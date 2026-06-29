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
