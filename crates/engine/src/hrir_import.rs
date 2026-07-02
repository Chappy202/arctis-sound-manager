//! HRIR WAV validation + import. Parses the RIFF header without external deps;
//! import copies HeSuVi 14-channel WAVs into the profiles dir, resampling 44.1→48
//! via an external tool subprocess (never in the audio path).
use crate::error::EngineError;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    pub channels: u16,
    pub sample_rate: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportVerdict {
    Ready,
    NeedsResample,
    RejectChannels(u16),
}

pub fn read_wav_info(path: &Path) -> Result<WavInfo, EngineError> {
    let bytes = std::fs::read(path)
        .map_err(|e| EngineError::BadRequest(format!("cannot read {}: {e}", path.display())))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(EngineError::BadRequest(format!(
            "not a RIFF/WAVE file: {}",
            path.display()
        )));
    }
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let sz =
            u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        if id == b"fmt " && sz >= 16 && i + 8 + sz <= bytes.len() {
            let channels = u16::from_le_bytes([bytes[i + 10], bytes[i + 11]]);
            let sample_rate =
                u32::from_le_bytes([bytes[i + 12], bytes[i + 13], bytes[i + 14], bytes[i + 15]]);
            return Ok(WavInfo {
                channels,
                sample_rate,
            });
        }
        i += 8 + sz + (sz & 1); // chunks are word-aligned
    }
    Err(EngineError::BadRequest(format!(
        "no fmt chunk in {}",
        path.display()
    )))
}

// ─── HRIR insertion-gain measurement ─────────────────────────────────────────

/// Target peak for the direct-path (FL_L / FR_R) impulse response after the
/// convolver `gain` option is applied. Normalizing every HRIR's direct-path
/// peak to this common level removes the per-HRIR insertion gain, so switching
/// HRIRs (or A/B-ing HRIR vs bypass) compares TONE, not loudness.
pub const HRIR_DIRECT_TARGET_PEAK: f32 = 1.0;

/// Cap the normalization correction at ±12 dB (0.25×..=4×): a wildly quiet or
/// hot file is more likely mis-mastered than in need of that much correction.
const HRIR_GAIN_MIN: f32 = 0.25;
const HRIR_GAIN_MAX: f32 = 4.0;

/// Measure the peak |sample| of the direct-path channels of a HeSuVi 14-channel
/// HRIR WAV: channel 0 (FL→left ear) and channel 7 (FR→right ear) — the same
/// channels the renderer wires as `convFL_L` / `convFR_R`. Supports 16-bit PCM
/// (format 1) and 32-bit float (format 3).
pub fn measure_direct_peak(path: &Path) -> Result<f32, EngineError> {
    let bytes = std::fs::read(path)
        .map_err(|e| EngineError::BadRequest(format!("cannot read {}: {e}", path.display())))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(EngineError::BadRequest(format!(
            "not a RIFF/WAVE file: {}",
            path.display()
        )));
    }
    /// (format code, channels, bits per sample) from the fmt chunk.
    type FmtInfo = (u16, u16, u16);
    let mut fmt: Option<FmtInfo> = None;
    let mut data: Option<(usize, usize)> = None;
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let sz =
            u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        if id == b"fmt " && sz >= 16 && i + 8 + sz <= bytes.len() {
            let format = u16::from_le_bytes([bytes[i + 8], bytes[i + 9]]);
            let channels = u16::from_le_bytes([bytes[i + 10], bytes[i + 11]]);
            let bits = u16::from_le_bytes([bytes[i + 22], bytes[i + 23]]);
            fmt = Some((format, channels, bits));
        }
        if id == b"data" {
            let end = (i + 8 + sz).min(bytes.len());
            data = Some((i + 8, end));
        }
        i += 8 + sz + (sz & 1); // chunks are word-aligned
    }
    let (format, channels, bits) = fmt
        .ok_or_else(|| EngineError::BadRequest(format!("no fmt chunk in {}", path.display())))?;
    let (start, end) = data
        .ok_or_else(|| EngineError::BadRequest(format!("no data chunk in {}", path.display())))?;
    if channels < 8 {
        return Err(EngineError::BadRequest(format!(
            "{}-channel WAV has no channel 7 (FR_R direct path)",
            channels
        )));
    }
    let ch = channels as usize;
    let mut peak = 0.0f32;
    match (format, bits) {
        (1, 16) => {
            let frame = ch * 2;
            let mut p = start;
            while p + frame <= end {
                for &c in &[0usize, 7usize] {
                    let o = p + c * 2;
                    let v = i16::from_le_bytes([bytes[o], bytes[o + 1]]) as f32 / 32768.0;
                    peak = peak.max(v.abs());
                }
                p += frame;
            }
        }
        (3, 32) => {
            let frame = ch * 4;
            let mut p = start;
            while p + frame <= end {
                for &c in &[0usize, 7usize] {
                    let o = p + c * 4;
                    let v = f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
                    peak = peak.max(v.abs());
                }
                p += frame;
            }
        }
        (f, b) => {
            return Err(EngineError::BadRequest(format!(
                "unsupported WAV sample format {f}/{b}-bit in {}",
                path.display()
            )))
        }
    }
    Ok(peak)
}

/// Best-effort normalization gain for the convolver `gain` config option:
/// `HRIR_DIRECT_TARGET_PEAK / direct-path peak`, clamped to ±12 dB. `None`
/// when the file cannot be measured (unreadable, silent, exotic format) — the
/// convolver then runs at unity, exactly the pre-normalization behaviour.
pub fn normalization_gain(path: &Path) -> Option<f32> {
    match measure_direct_peak(path) {
        Ok(peak) if peak > 0.0 && peak.is_finite() => {
            Some((HRIR_DIRECT_TARGET_PEAK / peak).clamp(HRIR_GAIN_MIN, HRIR_GAIN_MAX))
        }
        Ok(_) => None, // silent direct path — leave at unity
        Err(e) => {
            eprintln!(
                "asm: warning — cannot measure HRIR insertion gain for {} ({e}); using unity",
                path.display()
            );
            None
        }
    }
}

// ─── Bundled HRIR install ─────────────────────────────────────────────────────

const BUNDLED_HRIR: &[(&str, &[u8])] = &[(
    "07-oal+++-openal-max",
    include_bytes!("../../../assets/hrir/07-oal+++-openal-max.wav"),
)];

/// On first daemon boot, write each bundled HRIR into `<base_dir>/profiles/<stem>.wav`
/// if it is not already present.  Returns the stems newly installed (so
/// `["07-oal+++-openal-max"]` on the first run, `[]` on every subsequent run).
///
/// Callers should treat errors as non-fatal: surround simply reports "no HRIR"
/// when no profiles are present.
pub fn ensure_bundled(base_dir: &Path) -> Result<Vec<String>, EngineError> {
    let profiles = base_dir.join("profiles");
    std::fs::create_dir_all(&profiles).map_err(|e| {
        EngineError::BadRequest(format!("cannot create profiles dir: {e}"))
    })?;
    let mut installed = Vec::new();
    for (stem, data) in BUNDLED_HRIR {
        let dst = profiles.join(format!("{stem}.wav"));
        if !dst.exists() {
            std::fs::write(&dst, data).map_err(|e| {
                EngineError::BadRequest(format!("write bundled HRIR {stem}: {e}"))
            })?;
            installed.push((*stem).to_string());
        }
    }
    Ok(installed)
}

// ─────────────────────────────────────────────────────────────────────────────

/// Summary of a `import_dir` run: which files were imported and which were skipped (with reasons).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    pub imported: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

pub fn is_importable(info: &WavInfo) -> ImportVerdict {
    match (info.channels, info.sample_rate) {
        (14, 48000) => ImportVerdict::Ready,
        (14, _) => ImportVerdict::NeedsResample,
        (c, _) => ImportVerdict::RejectChannels(c),
    }
}

/// Copy HeSuVi 14-channel WAVs from `src` into `<base_dir>/profiles/`, resampling
/// 44.1 kHz files via ffmpeg. Returns an `ImportReport` on success; hard failures
/// (can't create profiles dir, can't read src dir) return `Err`.
///
/// Per-file problems (wrong channels, unreadable file, ffmpeg failure) are recorded
/// as `skipped` entries — they are never returned as `Err`. Idempotent: re-importing
/// the same stem overwrites the previous copy.
pub fn import_dir<R: arctis_audio::CommandRunner>(
    runner: &mut R,
    src: &Path,
    base_dir: &Path,
) -> Result<ImportReport, crate::error::EngineError> {
    let profiles = base_dir.join("profiles");
    std::fs::create_dir_all(&profiles).map_err(|e| {
        crate::error::EngineError::BadRequest(format!("cannot create profiles dir: {e}"))
    })?;

    let mut report = ImportReport::default();

    let entries = std::fs::read_dir(src).map_err(|e| {
        crate::error::EngineError::BadRequest(format!("cannot read import dir: {e}"))
    })?;

    for ent in entries.filter_map(|e| e.ok()) {
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("wav") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let info = match read_wav_info(&path) {
            Ok(i) => i,
            Err(e) => {
                report.skipped.push((stem, e.to_string()));
                continue;
            }
        };
        let dst = profiles.join(format!("{stem}.wav"));
        match is_importable(&info) {
            ImportVerdict::Ready => {
                std::fs::copy(&path, &dst).map_err(|e| {
                    crate::error::EngineError::BadRequest(format!("copy failed: {e}"))
                })?;
                report.imported.push(stem);
            }
            ImportVerdict::NeedsResample => {
                let src_str = match path.to_str() {
                    Some(s) => s.to_string(),
                    None => {
                        report.skipped.push((stem, "non-UTF8 source path".into()));
                        continue;
                    }
                };
                let dst_str = match dst.to_str() {
                    Some(s) => s.to_string(),
                    None => {
                        report.skipped.push((stem, "non-UTF8 destination path".into()));
                        continue;
                    }
                };
                let out = runner.run("ffmpeg", &["-y", "-i", &src_str, "-ar", "48000", &dst_str]);
                match out {
                    Ok(o) if o.status == 0 => report.imported.push(stem),
                    Ok(o) => report.skipped.push((
                        stem,
                        format!("ffmpeg failed (exit {}): {}", o.status, o.stderr),
                    )),
                    Err(e) => report
                        .skipped
                        .push((stem, format!("44.1kHz and ffmpeg resample unavailable: {e}"))),
                }
            }
            ImportVerdict::RejectChannels(c) => {
                report
                    .skipped
                    .push((stem, format!("{c}-channel WAV is not HeSuVi 14-channel")));
            }
        }
    }

    report.imported.sort();
    Ok(report)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn write_wav(path: &Path, channels: u16, rate: u32) {
        // Minimal canonical 16-bit PCM WAV header + zero data frames.
        let byte_rate = rate * channels as u32 * 2;
        let block_align = channels * 2;
        let data_len: u32 = 0;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&byte_rate.to_le_bytes());
        b.extend_from_slice(&block_align.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        std::fs::write(path, b).unwrap();
    }

    /// PCM16 WAV with explicit interleaved frames (each frame = `channels` samples).
    pub(crate) fn write_wav_pcm16_frames(path: &Path, channels: u16, rate: u32, frames: &[Vec<i16>]) {
        let data_len = (frames.len() * channels as usize * 2) as u32;
        let byte_rate = rate * channels as u32 * 2;
        let block_align = channels * 2;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&byte_rate.to_le_bytes());
        b.extend_from_slice(&block_align.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for f in frames {
            assert_eq!(f.len(), channels as usize);
            for s in f {
                b.extend_from_slice(&s.to_le_bytes());
            }
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn measures_direct_path_peak_on_channels_0_and_7() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.wav");
        // Frame 0: ch0 = 0.5 FS (direct L); frame 1: ch7 = 0.25 FS (direct R);
        // ch3 carries a HUGE value that must NOT count (not a direct channel).
        let mut f0 = vec![0i16; 14];
        f0[0] = 16384; // 0.5
        f0[3] = 32000; // ignored
        let mut f1 = vec![0i16; 14];
        f1[7] = 8192; // 0.25
        write_wav_pcm16_frames(&p, 14, 48_000, &[f0, f1]);
        let peak = measure_direct_peak(&p).unwrap();
        assert!((peak - 0.5).abs() < 1e-3, "direct peak must be 0.5, got {peak}");
    }

    #[test]
    fn normalization_gain_hits_target_and_clamps() {
        let d = tempfile::tempdir().unwrap();
        // peak 0.5 → gain 2.0 (10^(6/20) toward the 1.0 target)
        let p = d.path().join("half.wav");
        let mut f = vec![0i16; 14];
        f[0] = 16384;
        write_wav_pcm16_frames(&p, 14, 48_000, &[f]);
        let g = normalization_gain(&p).unwrap();
        assert!((g - 2.0).abs() < 1e-3, "0.5 peak → 2.0 gain, got {g}");

        // Tiny peak → clamped at +12 dB (4.0), not a huge boost.
        let p2 = d.path().join("tiny.wav");
        let mut f2 = vec![0i16; 14];
        f2[7] = 300; // ~0.009 FS
        write_wav_pcm16_frames(&p2, 14, 48_000, &[f2]);
        assert_eq!(normalization_gain(&p2), Some(4.0), "gain must clamp at 4.0");

        // Silent / empty data → None (unity).
        let p3 = d.path().join("silent.wav");
        write_wav(&p3, 14, 48_000);
        assert_eq!(normalization_gain(&p3), None, "unmeasurable → None (unity)");
    }

    #[test]
    fn reads_channels_and_rate() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let d = std::env::temp_dir().join(format!(
            "asm_hrir_test_a_{pid}_{nanos}",
            pid = std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("a.wav");
        write_wav(&p, 14, 48000);
        let info = read_wav_info(&p).unwrap();
        assert_eq!(info.channels, 14);
        assert_eq!(info.sample_rate, 48000);
        assert!(matches!(is_importable(&info), ImportVerdict::Ready));
    }

    #[test]
    fn flags_44k_as_needs_resample_and_7ch_as_rejected() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let d = std::env::temp_dir().join(format!(
            "asm_hrir_test_bc_{pid}_{nanos}",
            pid = std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        let p1 = d.join("b.wav");
        write_wav(&p1, 14, 44100);
        let p2 = d.join("c.wav");
        write_wav(&p2, 7, 48000);
        assert!(matches!(
            is_importable(&read_wav_info(&p1).unwrap()),
            ImportVerdict::NeedsResample
        ));
        assert!(matches!(
            is_importable(&read_wav_info(&p2).unwrap()),
            ImportVerdict::RejectChannels(7)
        ));
    }

    #[test]
    fn import_copies_ready_skips_wrong_channels() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let base = d.path().join("base");
        // a Ready 14ch/48k and a 7ch reject
        write_wav(&src.join("04-gsx.wav"), 14, 48000);
        write_wav(&src.join("none.wav"), 7, 48000);
        let mut runner = arctis_audio::MockRunner::new();
        let report = import_dir(&mut runner, &src, &base).unwrap();
        assert!(report.imported.contains(&"04-gsx".to_string()));
        assert!(base.join("profiles/04-gsx.wav").exists());
        assert!(report.skipped.iter().any(|(s, _)| s == "none"));
    }

    #[test]
    fn ensure_bundled_installs_when_absent_and_is_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let installed = ensure_bundled(d.path()).unwrap();
        assert!(
            installed.contains(&"07-oal+++-openal-max".to_string()),
            "first run must install 07-oal+++-openal-max; got: {installed:?}"
        );
        assert!(
            d.path().join("profiles/07-oal+++-openal-max.wav").exists(),
            "profiles/07-oal+++-openal-max.wav must exist after install"
        );
        // Second run: idempotent — nothing newly installed.
        let again = ensure_bundled(d.path()).unwrap();
        assert!(
            again.is_empty(),
            "second run must return empty (idempotent); got: {again:?}"
        );
    }

    #[test]
    fn rejects_fmt_chunk_with_undersized_declared_size() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let d = std::env::temp_dir().join(format!(
            "asm_hrir_test_malformed_{pid}_{nanos}",
            pid = std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("malformed.wav");

        // Craft a malformed WAV: fmt chunk declares size 4 instead of 16.
        // Parser should reject it and return an error, not read garbage.
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&28u32.to_le_bytes()); // file size - 8 = 36 - 8 = 28
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&4u32.to_le_bytes()); // WRONG: declares only 4 bytes instead of 16
        b.extend_from_slice(&[0u8, 0u8, 0u8, 0u8]); // 4 bytes junk
        b.extend_from_slice(b"data");
        b.extend_from_slice(&0u32.to_le_bytes()); // data chunk size 0
        std::fs::write(&p, b).unwrap();

        // The parser should reject this file (no valid fmt chunk), not silently
        // read garbage from overlapping chunk data.
        let result = read_wav_info(&p);
        assert!(result.is_err(), "Parser should reject fmt chunk with undersized declared size");
        assert!(result.unwrap_err().to_string().contains("no fmt chunk"));
    }
}
