//! Read-only factory preset catalog. Channel EQ = oratory1990-measured Nova Pro Wireless
//! curves; mic presets = verbatim Sonar voice presets. See the design spec for provenance.
// q: 0.7071 throughout is a deliberate Butterworth Q literal; keep exact values (not 1/√2 from consts).
#![allow(clippy::approx_constant)]
use arctis_config::{EqBandConfig, EqPreset, MicCompressorStage, MicGainStage, MicGateStage,
    MicHighpassStage, MicPreset, MicSuppressionStage};

// ── EQ band builders (band 0 = lowshelf@31, band 9 = highshelf@16k, rest peaking) ──
fn pk(freq: f32, gain: f32) -> EqBandConfig { EqBandConfig { kind: "peaking".into(), freq_hz: freq, q: 1.0, gain_db: gain } }
fn pkq(freq: f32, q: f32, gain: f32) -> EqBandConfig { EqBandConfig { kind: "peaking".into(), freq_hz: freq, q, gain_db: gain } }
fn ls(gain: f32) -> EqBandConfig { EqBandConfig { kind: "lowshelf".into(), freq_hz: 31.0, q: 0.7, gain_db: gain } }
fn hs(gain: f32) -> EqBandConfig { EqBandConfig { kind: "highshelf".into(), freq_hz: 16000.0, q: 0.7, gain_db: gain } }

/// Format a dB value compactly: whole numbers without the decimal ("-4 dB"),
/// fractional ones with one decimal ("-4.5 dB").
fn fmt_db(v: f32) -> String {
    if (v - v.round()).abs() < 1e-6 { format!("{}", v.round() as i32) } else { format!("{v:.1}") }
}

/// Build the kind hint FROM the band data (G4): the engine now renders a real
/// auto-preamp node compensating the largest boost, so the advertised preamp is
/// derived from the same rule (`−max boost`), never hand-written.
fn eqp(name: &str, desc: &str, bands: Vec<EqBandConfig>) -> EqPreset {
    let max_boost = bands.iter().map(|b| b.gain_db).fold(0.0f32, f32::max);
    let hint = if max_boost > 0.0 {
        format!("{desc} · preamp {} dB", fmt_db(-max_boost))
    } else {
        format!("{desc} · no boost")
    };
    EqPreset { name: name.into(), kind_hint: Some(hint), bands }
}

pub fn factory_eq_presets() -> Vec<EqPreset> {
    vec![
        eqp("Flat", "Reference",
            vec![ls(0.0), pk(62.0,0.0), pk(125.0,0.0), pk(250.0,0.0), pk(500.0,0.0), pk(1000.0,0.0), pk(2000.0,0.0), pk(4000.0,0.0), pk(8000.0,0.0), hs(0.0)]),
        eqp("Reference (Calibrated)", "Reference",
            vec![ls(-1.5), pk(62.0,0.3), pkq(125.0,1.41,-9.5), pkq(250.0,1.41,-1.6), pk(500.0,1.0), pk(1000.0,2.2), pk(2000.0,1.6), pk(4000.0,2.8), pk(8000.0,0.2), hs(2.2)]),
        eqp("Bass Boost", "Music",
            vec![ls(4.0), pk(62.0,2.0), pkq(125.0,1.41,-5.0), pk(250.0,-0.5), pk(500.0,0.5), pk(1000.0,1.5), pk(2000.0,1.0), pk(4000.0,2.5), pk(8000.0,-1.0), hs(1.0)]),
        eqp("FPS / Footsteps", "Gaming",
            vec![ls(-2.0), pk(62.0,-1.5), pkq(125.0,1.41,-7.0), pk(250.0,-2.5), pk(500.0,0.0), pk(1000.0,2.0), pk(2000.0,3.5), pk(4000.0,5.0), pk(8000.0,2.5), hs(1.0)]),
        eqp("FPS / Footsteps (Competitive)", "Gaming",
            vec![ls(0.0), pk(62.0,-3.0), pkq(125.0,1.41,-2.0), pk(250.0,3.0), pk(500.0,0.0), pk(1000.0,0.0), pk(2000.0,3.0), pk(4000.0,2.0), pk(8000.0,0.0), hs(0.0)]),
        // Applied PRE-convolution on the game channel sink (factory_profiles.rs);
        // the old "post-HRIR" wording contradicted the code.
        eqp("DayZ Spatial", "Gaming · pre-HRIR footsteps + air",
            vec![ls(-1.0), pk(62.0,-3.0), pkq(125.0,1.41,-2.0), pk(250.0,3.0), pk(500.0,0.0), pk(1000.0,0.0), pk(2000.0,3.0), pk(4000.0,2.0), pk(8000.0,1.5), hs(1.5)]),
        eqp("Immersive", "Movies",
            vec![ls(4.5), pk(62.0,2.5), pkq(125.0,1.41,-5.5), pk(250.0,-1.0), pk(500.0,0.0), pk(1000.0,0.5), pk(2000.0,1.0), pk(4000.0,2.0), pk(8000.0,-2.0), hs(3.5)]),
        eqp("Vocal Clarity", "Voice",
            vec![ls(-3.0), pk(62.0,-1.5), pkq(125.0,1.41,-8.5), pk(250.0,0.0), pk(500.0,1.0), pk(1000.0,3.0), pk(2000.0,3.0), pk(4000.0,4.0), pk(8000.0,-1.5), hs(1.5)]),
        eqp("Warm", "Music",
            vec![ls(2.5), pk(62.0,3.0), pkq(125.0,1.41,-3.5), pk(250.0,1.5), pk(500.0,0.5), pk(1000.0,0.0), pk(2000.0,0.0), pk(4000.0,1.5), pk(8000.0,-3.5), hs(-1.5)]),
        eqp("Treble Smooth", "Fatigue",
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

    #[test]
    fn kind_hints_derive_preamp_from_band_data() {
        let presets = factory_eq_presets();
        let hint = |name: &str| {
            presets.iter().find(|p| p.name == name).expect(name).kind_hint.clone().unwrap()
        };
        // Preamp text = −(largest boosted band), the same rule the rendered
        // eq_preamp node uses — never a hand-written number.
        assert_eq!(hint("Flat"), "Reference · no boost");
        assert_eq!(hint("Bass Boost"), "Music · preamp -4 dB");
        assert_eq!(hint("Immersive"), "Movies · preamp -4.5 dB");
        assert_eq!(hint("Reference (Calibrated)"), "Reference · preamp -2.8 dB");
        // DayZ Spatial is applied PRE-convolution (factory_profiles.rs seeds the
        // game CHANNEL sink EQ) — the hint must say so.
        let dz = hint("DayZ Spatial");
        assert!(dz.contains("pre-HRIR"), "DayZ hint must match the code (pre-convolution): {dz}");
        assert!(dz.contains("preamp -3 dB"), "DayZ hint must carry the derived preamp: {dz}");
    }

    #[test]
    fn competitive_footsteps_preset_present_and_gentle() {
        let p = factory_eq_presets();
        let fp = p.iter().find(|p| p.name == "FPS / Footsteps (Competitive)").expect("preset present");
        // gentle: no band exceeds +4 dB, sub-bass cut present
        assert!(fp.bands.iter().all(|b| b.gain_db <= 4.0));
        assert!(fp.bands.iter().any(|b| b.freq_hz == 62.0 && b.gain_db < 0.0));
    }

    #[test]
    fn dayz_spatial_preset_present_and_capped_at_plus_3() {
        let p = factory_eq_presets();
        let dz = p.iter().find(|p| p.name == "DayZ Spatial").expect("DayZ Spatial present");
        assert_eq!(dz.bands.len(), 10, "DayZ Spatial must have 10 bands");
        assert!(dz.bands.iter().all(|b| b.gain_db <= 3.0), "no band may boost beyond +3 dB");
        // 250 Hz footstep-weight boost and 62 Hz rumble cut are the signature moves.
        assert!(dz.bands.iter().any(|b| b.freq_hz == 250.0 && b.gain_db == 3.0));
        assert!(dz.bands.iter().any(|b| b.freq_hz == 62.0 && b.gain_db < 0.0));
    }
}
