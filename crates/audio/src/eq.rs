use crate::error::AudioError;

/// Biquad band type. Labels are the PipeWire builtin filter labels.
/// Confirmed: https://docs.pipewire.org/page_module_filter_chain.html
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandKind {
    Peaking,
    LowShelf,
    HighShelf,
}

impl BandKind {
    /// The PipeWire builtin node `label` for this band type.
    pub fn label(&self) -> &'static str {
        match self {
            BandKind::Peaking => "bq_peaking",
            BandKind::LowShelf => "bq_lowshelf",
            BandKind::HighShelf => "bq_highshelf",
        }
    }
}

/// Engine-wide audio constants (ARCHITECTURE G3 / spec §3).
pub const SAMPLE_RATE_HZ: u32 = 48_000;
pub const MAX_BANDS: usize = 10;
pub const GAIN_MIN_DB: f32 = -12.0;
pub const GAIN_MAX_DB: f32 = 12.0;
pub const Q_MIN: f32 = 0.3;
pub const Q_MAX: f32 = 10.0;
pub const FREQ_MIN_HZ: f32 = 20.0;
pub const FREQ_MAX_HZ: f32 = 20_000.0;

/// One parametric EQ band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqBand {
    pub kind: BandKind,
    pub freq_hz: f32,
    pub q: f32,
    pub gain_db: f32,
}

impl EqBand {
    pub fn new(kind: BandKind, freq_hz: f32, q: f32, gain_db: f32) -> Self {
        Self {
            kind,
            freq_hz,
            q,
            gain_db,
        }
    }

    /// Validate ranges. Our chosen defaults (spec §6 — SteelSeries' exact
    /// ranges are unpublished).
    pub fn validate(&self) -> Result<(), AudioError> {
        if !(FREQ_MIN_HZ..=FREQ_MAX_HZ).contains(&self.freq_hz) {
            return Err(AudioError::Invalid(format!(
                "freq {} Hz out of range {}..={}",
                self.freq_hz, FREQ_MIN_HZ, FREQ_MAX_HZ
            )));
        }
        if !(Q_MIN..=Q_MAX).contains(&self.q) {
            return Err(AudioError::Invalid(format!(
                "Q {} out of range {}..={}",
                self.q, Q_MIN, Q_MAX
            )));
        }
        if !(GAIN_MIN_DB..=GAIN_MAX_DB).contains(&self.gain_db) {
            return Err(AudioError::Invalid(format!(
                "gain {} dB out of range {}..={}",
                self.gain_db, GAIN_MIN_DB, GAIN_MAX_DB
            )));
        }
        Ok(())
    }
}

/// A full per-sink EQ: an ordered list of bands.
#[derive(Debug, Clone, PartialEq)]
pub struct EqModel {
    pub bands: Vec<EqBand>,
}

impl EqModel {
    /// 10 flat peaking bands at standard ISO-ish centers; gain 0 dB, Q 1.0.
    pub fn default_10band() -> Self {
        const CENTERS: [f32; MAX_BANDS] = [
            31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
        ];
        let bands = CENTERS
            .iter()
            .map(|&f| EqBand::new(BandKind::Peaking, f, 1.0, 0.0))
            .collect();
        Self { bands }
    }

    pub fn validate(&self) -> Result<(), AudioError> {
        if self.bands.is_empty() {
            return Err(AudioError::Invalid("EQ has no bands".into()));
        }
        if self.bands.len() > MAX_BANDS {
            return Err(AudioError::Invalid(format!(
                "{} bands exceeds max {}",
                self.bands.len(),
                MAX_BANDS
            )));
        }
        for (i, b) in self.bands.iter().enumerate() {
            b.validate()
                .map_err(|e| AudioError::Invalid(format!("band {i}: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_pipewire_builtins() {
        assert_eq!(BandKind::Peaking.label(), "bq_peaking");
        assert_eq!(BandKind::LowShelf.label(), "bq_lowshelf");
        assert_eq!(BandKind::HighShelf.label(), "bq_highshelf");
    }

    #[test]
    fn default_is_ten_flat_bands_and_validates() {
        let m = EqModel::default_10band();
        assert_eq!(m.bands.len(), 10);
        assert!(m.bands.iter().all(|b| b.gain_db == 0.0 && b.q == 1.0));
        assert!(m.validate().is_ok());
    }

    #[test]
    fn rejects_out_of_range_gain() {
        let b = EqBand::new(BandKind::Peaking, 1000.0, 1.0, 99.0);
        assert!(b.validate().is_err());
    }

    #[test]
    fn rejects_out_of_range_freq_and_q() {
        assert!(EqBand::new(BandKind::Peaking, 5.0, 1.0, 0.0)
            .validate()
            .is_err());
        assert!(EqBand::new(BandKind::Peaking, 1000.0, 0.01, 0.0)
            .validate()
            .is_err());
    }

    #[test]
    fn rejects_too_many_bands() {
        let m = EqModel {
            bands: vec![EqBand::new(BandKind::Peaking, 1000.0, 1.0, 0.0); MAX_BANDS + 1],
        };
        assert!(m.validate().is_err());
    }

    // --- boundary: exact valid edges must pass ---

    #[test]
    fn gain_max_edge_passes() {
        assert!(EqBand::new(BandKind::Peaking, 1000.0, 1.0, 12.0)
            .validate()
            .is_ok());
    }

    #[test]
    fn gain_min_edge_passes() {
        assert!(EqBand::new(BandKind::Peaking, 1000.0, 1.0, -12.0)
            .validate()
            .is_ok());
    }

    #[test]
    fn q_min_edge_passes() {
        assert!(EqBand::new(BandKind::Peaking, 1000.0, 0.3, 0.0)
            .validate()
            .is_ok());
    }

    #[test]
    fn q_max_edge_passes() {
        assert!(EqBand::new(BandKind::Peaking, 1000.0, 10.0, 0.0)
            .validate()
            .is_ok());
    }

    #[test]
    fn freq_min_edge_passes() {
        assert!(EqBand::new(BandKind::Peaking, 20.0, 1.0, 0.0)
            .validate()
            .is_ok());
    }

    #[test]
    fn freq_max_edge_passes() {
        assert!(EqBand::new(BandKind::Peaking, 20_000.0, 1.0, 0.0)
            .validate()
            .is_ok());
    }

    // --- boundary: just-past edges must fail ---

    #[test]
    fn gain_just_above_max_fails() {
        let b = EqBand::new(BandKind::Peaking, 1000.0, 1.0, 12.1);
        assert!(matches!(b.validate(), Err(AudioError::Invalid(_))));
    }

    #[test]
    fn gain_just_below_min_fails() {
        let b = EqBand::new(BandKind::Peaking, 1000.0, 1.0, -12.1);
        assert!(matches!(b.validate(), Err(AudioError::Invalid(_))));
    }

    #[test]
    fn q_just_below_min_fails() {
        let b = EqBand::new(BandKind::Peaking, 1000.0, 0.29, 0.0);
        assert!(matches!(b.validate(), Err(AudioError::Invalid(_))));
    }

    #[test]
    fn q_just_above_max_fails() {
        let b = EqBand::new(BandKind::Peaking, 1000.0, 10.1, 0.0);
        assert!(matches!(b.validate(), Err(AudioError::Invalid(_))));
    }

    #[test]
    fn freq_just_below_min_fails() {
        let b = EqBand::new(BandKind::Peaking, 19.9, 1.0, 0.0);
        assert!(matches!(b.validate(), Err(AudioError::Invalid(_))));
    }

    #[test]
    fn freq_just_above_max_fails() {
        let b = EqBand::new(BandKind::Peaking, 20_000.1, 1.0, 0.0);
        assert!(matches!(b.validate(), Err(AudioError::Invalid(_))));
    }

    // --- EqModel with no bands must fail ---

    #[test]
    fn empty_bands_rejected() {
        let m = EqModel { bands: vec![] };
        assert!(matches!(m.validate(), Err(AudioError::Invalid(_))));
    }
}
