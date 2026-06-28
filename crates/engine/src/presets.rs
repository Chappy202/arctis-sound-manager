//! Read-only factory preset catalog. Channel EQ = oratory1990-measured Nova Pro Wireless
//! curves; mic presets = verbatim Sonar voice presets. See the design spec for provenance.
use arctis_config::{EqBandConfig, EqPreset, MicCompressorStage, MicGainStage, MicGateStage,
    MicHighpassStage, MicPreset, MicSuppressionStage};

// ── EQ band builders (band 0 = lowshelf@31, band 9 = highshelf@16k, rest peaking) ──
fn pk(freq: f32, gain: f32) -> EqBandConfig { EqBandConfig { kind: "peaking".into(), freq_hz: freq, q: 1.0, gain_db: gain } }
fn pkq(freq: f32, q: f32, gain: f32) -> EqBandConfig { EqBandConfig { kind: "peaking".into(), freq_hz: freq, q, gain_db: gain } }
fn ls(gain: f32) -> EqBandConfig { EqBandConfig { kind: "lowshelf".into(), freq_hz: 31.0, q: 0.7, gain_db: gain } }
fn hs(gain: f32) -> EqBandConfig { EqBandConfig { kind: "highshelf".into(), freq_hz: 16000.0, q: 0.7, gain_db: gain } }

fn eqp(name: &str, hint: &str, bands: Vec<EqBandConfig>) -> EqPreset {
    EqPreset { name: name.into(), kind_hint: Some(hint.into()), bands }
}

pub fn factory_eq_presets() -> Vec<EqPreset> {
    vec![
        eqp("Flat", "Reference · no EQ",
            vec![ls(0.0), pk(62.0,0.0), pk(125.0,0.0), pk(250.0,0.0), pk(500.0,0.0), pk(1000.0,0.0), pk(2000.0,0.0), pk(4000.0,0.0), pk(8000.0,0.0), hs(0.0)]),
        eqp("Reference (Calibrated)", "Reference · preamp -3.2 dB",
            vec![ls(-1.5), pk(62.0,0.3), pkq(125.0,1.41,-9.5), pkq(250.0,1.41,-1.6), pk(500.0,1.0), pk(1000.0,2.2), pk(2000.0,1.6), pk(4000.0,2.8), pk(8000.0,0.2), hs(2.2)]),
        eqp("Bass Boost", "Music · preamp -4 dB",
            vec![ls(4.0), pk(62.0,2.0), pkq(125.0,1.41,-5.0), pk(250.0,-0.5), pk(500.0,0.5), pk(1000.0,1.5), pk(2000.0,1.0), pk(4000.0,2.5), pk(8000.0,-1.0), hs(1.0)]),
        eqp("FPS / Footsteps", "Gaming · preamp -5 dB",
            vec![ls(-2.0), pk(62.0,-1.5), pkq(125.0,1.41,-7.0), pk(250.0,-2.5), pk(500.0,0.0), pk(1000.0,2.0), pk(2000.0,3.5), pk(4000.0,5.0), pk(8000.0,2.5), hs(1.0)]),
        eqp("Immersive", "Movies · preamp -4.5 dB",
            vec![ls(4.5), pk(62.0,2.5), pkq(125.0,1.41,-5.5), pk(250.0,-1.0), pk(500.0,0.0), pk(1000.0,0.5), pk(2000.0,1.0), pk(4000.0,2.0), pk(8000.0,-2.0), hs(3.5)]),
        eqp("Vocal Clarity", "Voice · preamp -4 dB",
            vec![ls(-3.0), pk(62.0,-1.5), pkq(125.0,1.41,-8.5), pk(250.0,0.0), pk(500.0,1.0), pk(1000.0,3.0), pk(2000.0,3.0), pk(4000.0,4.0), pk(8000.0,-1.5), hs(1.5)]),
        eqp("Warm", "Music · preamp -3 dB",
            vec![ls(2.5), pk(62.0,3.0), pkq(125.0,1.41,-3.5), pk(250.0,1.5), pk(500.0,0.5), pk(1000.0,0.0), pk(2000.0,0.0), pk(4000.0,1.5), pk(8000.0,-3.5), hs(-1.5)]),
        eqp("Treble Smooth", "Fatigue · preamp -3 dB",
            vec![ls(-1.0), pk(62.0,0.5), pkq(125.0,1.41,-8.0), pk(250.0,-1.5), pk(500.0,1.0), pk(1000.0,2.0), pk(2000.0,2.0), pk(4000.0,3.0), pkq(8000.0,1.4,-4.0), hs(-2.0)]),
    ]
}

// ── Mic EQ builders (all Q 0.7071) ──
fn mls(gain: f32) -> EqBandConfig { EqBandConfig { kind: "lowshelf".into(), freq_hz: 31.0, q: 0.7071, gain_db: gain } }
fn mpk(freq: f32, gain: f32) -> EqBandConfig { EqBandConfig { kind: "peaking".into(), freq_hz: freq, q: 0.7071, gain_db: gain } }
fn mhs(gain: f32) -> EqBandConfig { EqBandConfig { kind: "highshelf".into(), freq_hz: 16000.0, q: 0.7071, gain_db: gain } }

/// g = the 8 inner peaking-band gains for 62..8000 Hz; b0 = 31 Hz lowshelf gain; b9 = 16 kHz highshelf gain.
fn miceq(b0: f32, g: [f32; 8], b9: f32) -> Vec<EqBandConfig> {
    vec![mls(b0), mpk(62.0,g[0]), mpk(125.0,g[1]), mpk(250.0,g[2]), mpk(500.0,g[3]),
         mpk(1000.0,g[4]), mpk(2000.0,g[5]), mpk(4000.0,g[6]), mpk(8000.0,g[7]), mhs(b9)]
}

fn micp(name: &str, desc: &str, supp: bool, gate: bool, comp: bool, eq: Vec<EqBandConfig>) -> MicPreset {
    MicPreset {
        name: name.into(), description: desc.into(),
        gain: MicGainStage::default(),
        highpass: MicHighpassStage::default(),
        suppression: MicSuppressionStage { enabled: supp, ..Default::default() },
        compressor: MicCompressorStage { enabled: comp, ..Default::default() },
        gate: MicGateStage { enabled: gate, ..Default::default() },
        eq_enabled: true, eq,
    }
}

pub fn factory_mic_presets() -> Vec<MicPreset> {
    vec![
        micp("Flat", "No processing — a clean reset", false, false, false,
            miceq(0.0, [0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0], 0.0)),
        micp("Less Nasal", "Tame the nasal/honky 1 kHz zone", true, false, false,
            miceq(-12.0, [0.0,0.0,0.0,-2.0,-4.0,-2.0,2.0,4.0], 0.0)),
        micp("Deep Voice", "Warmth and body for thin voices", true, false, false,
            miceq(-12.0, [4.0,4.0,3.0,0.0,-3.0,-1.0,0.0,0.0], 0.0)),
        micp("Balanced", "General-purpose clarity", true, false, false,
            miceq(-12.0, [0.0,0.0,-3.0,-2.0,0.0,2.0,3.0,1.0], 0.0)),
        micp("Clarity High Pitch", "Presence/air for high voices", true, false, false,
            miceq(-12.0, [-12.0,-4.0,-4.0,0.0,0.0,3.0,4.0,4.0], 4.0)),
        micp("Clarity Low Pitch", "Presence/air for deep voices", true, false, false,
            miceq(-12.0, [-3.0,-2.5,0.0,0.0,0.0,3.0,4.0,4.0], 4.0)),
        micp("Broadcast High Pitch", "Radio voice for high voices", true, false, false,
            miceq(-12.0, [-12.0,-3.0,6.0,3.0,-2.5,0.0,3.0,4.0], 4.0)),
        micp("Broadcast Low Pitch", "Radio voice for deep voices", true, false, false,
            miceq(-12.0, [2.0,5.0,2.0,-3.0,-2.0,0.0,3.0,4.0], 4.0)),
        micp("Walkie Talkie", "Two-way radio bandpass effect", true, false, false,
            miceq(-12.0, [-12.0,-12.0,-12.0,5.0,6.0,6.0,5.0,-12.0], -12.0)),
        micp("Streamer", "Full chain: gate + compressor + presence", true, true, true,
            miceq(-12.0, [0.0,0.0,-2.0,-1.0,0.0,2.0,3.0,1.0], 0.0)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    const FREQS: [f32; 10] = [31.0,62.0,125.0,250.0,500.0,1000.0,2000.0,4000.0,8000.0,16000.0];

    #[test]
    fn every_factory_preset_is_well_formed() {
        let eq = factory_eq_presets();
        assert!(eq.iter().any(|p| p.name == "Reference (Calibrated)"));
        let mic = factory_mic_presets();
        for p in &mic { assert!(["Flat","Less Nasal","Deep Voice","Balanced","Clarity High Pitch",
            "Clarity Low Pitch","Broadcast High Pitch","Broadcast Low Pitch","Walkie Talkie","Streamer"]
            .contains(&p.name.as_str()), "unexpected mic preset {}", p.name); }
        // Channel + mic EQ vectors: 10 dense bands at canonical freqs, gains within ±12.
        let all_band_sets: Vec<&Vec<EqBandConfig>> =
            eq.iter().map(|p| &p.bands).chain(mic.iter().map(|p| &p.eq)).collect();
        for bands in all_band_sets {
            assert_eq!(bands.len(), 10, "preset must have 10 bands");
            for (i, b) in bands.iter().enumerate() {
                assert_eq!(b.freq_hz, FREQS[i], "band {i} freq mismatch");
                assert!((-12.0..=12.0).contains(&b.gain_db), "gain {} out of ±12", b.gain_db);
                assert!(["peaking","lowshelf","highshelf"].contains(&b.kind.as_str()));
            }
            assert_eq!(bands[0].kind, "lowshelf");
            assert_eq!(bands[9].kind, "highshelf");
        }
        // Names unique within each catalog.
        let mut eqn: Vec<_> = eq.iter().map(|p| &p.name).collect(); eqn.sort(); eqn.dedup();
        assert_eq!(eqn.len(), eq.len());
    }
}
