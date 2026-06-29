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
        if id == b"fmt " && i + 8 + 16 <= bytes.len() {
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

pub fn is_importable(info: &WavInfo) -> ImportVerdict {
    match (info.channels, info.sample_rate) {
        (14, 48000) => ImportVerdict::Ready,
        (14, _) => ImportVerdict::NeedsResample,
        (c, _) => ImportVerdict::RejectChannels(c),
    }
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
}
