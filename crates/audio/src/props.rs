use crate::config::band_node_name;
use crate::eq::EqBand;
use crate::error::AudioError;

/// Control names exposed by every builtin biquad node.
/// Confirmed: https://docs.pipewire.org/page_module_filter_chain.html
/// (The `<node-name>:<control>` addressing form is verified on-machine in
/// Task 6; if the daemon reports a different convention, change ONLY these
/// and the key format below.)
const CTL_FREQ: &str = "Freq";
const CTL_Q: &str = "Q";
const CTL_GAIN: &str = "Gain";

/// Format a value for the SPA Props payload: always emit a decimal so the
/// value is unambiguously a float to SPA (unlike conf `fmt_num` which drops `.0`).
fn fmt_f(v: f32) -> String {
    if v.fract() == 0.0 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

/// The `<param-json>` for `pw-cli s <id> Props <json>` that updates one band's
/// three controls in place (live; no conf rewrite, no restart — G3).
pub fn band_props_json(band_index: usize, band: &EqBand) -> Result<String, AudioError> {
    band.validate()?;
    let n = band_node_name(band_index);
    Ok(format!(
        "{{ params = [ \"{n}:{CTL_FREQ}\" {f} \"{n}:{CTL_Q}\" {q} \"{n}:{CTL_GAIN}\" {g} ] }}",
        f = fmt_f(band.freq_hz),
        q = fmt_f(band.q),
        g = fmt_f(band.gain_db),
    ))
}

/// Full argv (after the `pw-cli` program) to apply one band live.
pub fn set_band_props_argv(
    node_id: &str,
    band_index: usize,
    band: &EqBand,
) -> Result<Vec<String>, AudioError> {
    if node_id.trim().is_empty() {
        return Err(AudioError::Invalid("empty node id".into()));
    }
    Ok(vec![
        "s".to_string(),
        node_id.to_string(),
        "Props".to_string(),
        band_props_json(band_index, band)?,
    ])
}

/// The `<param-json>` for `pw-cli s <id> Props <json>` that updates a single
/// control by `<node_name>:<control>` address (live; no restart).
pub fn control_props_json(node_name: &str, control: &str, value: f32) -> String {
    format!(
        "{{ params = [ \"{node_name}:{control}\" {} ] }}",
        fmt_f(value)
    )
}

/// Full argv (after the `pw-cli` program) to apply one control live.
pub fn set_control_props_argv(
    node_id: &str,
    node_name: &str,
    control: &str,
    value: f32,
) -> Result<Vec<String>, AudioError> {
    if node_id.trim().is_empty() {
        return Err(AudioError::Invalid("empty node id".into()));
    }
    Ok(vec![
        "s".to_string(),
        node_id.to_string(),
        "Props".to_string(),
        control_props_json(node_name, control, value),
    ])
}

/// The `<param-json>` for `pw-cli s <id> Props <json>` that sets a node's
/// volume (channelVolumes array) and mute (bool) via SPA Props.
pub fn node_volume_props_json(channel_volumes: &[f32], mute: bool) -> String {
    let vols: Vec<String> = channel_volumes.iter().map(|v| fmt_f(*v)).collect();
    let vols_str = vols.join(" ");
    let mute_str = if mute { "true" } else { "false" };
    format!("{{ channelVolumes = [ {vols_str} ] mute = {mute_str} }}")
}

/// Full argv (after the `pw-cli` program) to set a node's volume+mute live.
pub fn set_node_volume_props_argv(
    node_id: &str,
    channel_volumes: &[f32],
    mute: bool,
) -> Result<Vec<String>, AudioError> {
    if node_id.trim().is_empty() {
        return Err(AudioError::Invalid("empty node id".into()));
    }
    Ok(vec![
        "s".to_string(),
        node_id.to_string(),
        "Props".to_string(),
        node_volume_props_json(channel_volumes, mute),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::BandKind;

    #[test]
    fn json_addresses_band_controls_by_node_name() {
        let b = EqBand::new(BandKind::Peaking, 1200.0, 1.0, -4.5);
        let json = band_props_json(3, &b).unwrap();
        assert_eq!(
            json,
            "{ params = [ \"eq_band_3:Freq\" 1200.0 \"eq_band_3:Q\" 1.0 \"eq_band_3:Gain\" -4.5 ] }"
        );
    }

    #[test]
    fn argv_is_s_id_props_json() {
        let b = EqBand::new(BandKind::Peaking, 1000.0, 1.0, 0.0);
        let argv = set_band_props_argv("57", 0, &b).unwrap();
        assert_eq!(argv[0], "s");
        assert_eq!(argv[1], "57");
        assert_eq!(argv[2], "Props");
        assert_eq!(
            argv[3],
            "{ params = [ \"eq_band_0:Freq\" 1000.0 \"eq_band_0:Q\" 1.0 \"eq_band_0:Gain\" 0.0 ] }"
        );
    }

    #[test]
    fn rejects_empty_node_id() {
        let b = EqBand::new(BandKind::Peaking, 1000.0, 1.0, 0.0);
        assert!(set_band_props_argv("  ", 0, &b).is_err());
    }

    #[test]
    fn rejects_invalid_band() {
        let b = EqBand::new(BandKind::Peaking, 1000.0, 1.0, 999.0);
        assert!(band_props_json(0, &b).is_err());
    }

    #[test]
    fn control_props_json_formats_single_control() {
        let json = control_props_json("mic_rnnoise", "VAD Threshold (%)", 40.0);
        assert_eq!(
            json,
            "{ params = [ \"mic_rnnoise:VAD Threshold (%)\" 40.0 ] }"
        );
    }

    #[test]
    fn set_control_props_argv_is_s_id_props_json() {
        let argv = set_control_props_argv("57", "mic_rnnoise", "VAD Threshold (%)", 40.0).unwrap();
        assert_eq!(
            argv,
            vec![
                "s".to_string(),
                "57".to_string(),
                "Props".to_string(),
                "{ params = [ \"mic_rnnoise:VAD Threshold (%)\" 40.0 ] }".to_string(),
            ]
        );
    }

    #[test]
    fn set_control_props_argv_rejects_empty_node_id() {
        assert!(set_control_props_argv("  ", "mic_gain", "Mult", 1.0).is_err());
    }

    #[test]
    fn node_volume_props_json_stereo_unmuted() {
        let json = node_volume_props_json(&[1.0, 1.0], false);
        assert_eq!(json, "{ channelVolumes = [ 1.0 1.0 ] mute = false }");
    }

    #[test]
    fn node_volume_props_json_stereo_muted() {
        let json = node_volume_props_json(&[0.5, 0.5], true);
        assert_eq!(json, "{ channelVolumes = [ 0.5 0.5 ] mute = true }");
    }

    #[test]
    fn set_node_volume_props_argv_is_s_id_props_json() {
        let argv = set_node_volume_props_argv("42", &[1.0, 1.0], false).unwrap();
        assert_eq!(
            argv,
            vec![
                "s".to_string(),
                "42".to_string(),
                "Props".to_string(),
                "{ channelVolumes = [ 1.0 1.0 ] mute = false }".to_string(),
            ]
        );
    }

    #[test]
    fn set_node_volume_props_argv_rejects_empty_node_id() {
        assert!(set_node_volume_props_argv("  ", &[1.0, 1.0], false).is_err());
    }
}
