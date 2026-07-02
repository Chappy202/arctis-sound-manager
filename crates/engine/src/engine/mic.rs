//! Microphone DSP chain: stages, params, mic EQ, suppression backend, presets.
use super::*;

impl<R: CommandRunner> Engine<R> {
    // ── Mic engine methods ───────────────────────────────────────────────────

    /// Enable or disable a specific mic DSP stage.
    /// Topology change → full recreate (remove + respawn with new node list).
    pub fn mic_set_stage_enabled(&mut self, stage: StageKind, on: bool) -> Result<(), EngineError> {
        // Mutate config
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            match stage {
                StageKind::Gain => profile.mic.gain.enabled = on,
                StageKind::Highpass => profile.mic.highpass.enabled = on,
                StageKind::Suppression => profile.mic.suppression.enabled = on,
                StageKind::Compressor => profile.mic.compressor.enabled = on,
                StageKind::Gate => profile.mic.gate.enabled = on,
                StageKind::MicEq => profile.mic.eq_enabled = on,
            }
        }
        self.save_config()?;

        // Only perform live graph I/O when the master switch is on.
        // When off, the persisted config change takes effect the next time
        // mic_set_enabled(true) or reconcile() builds the chain.
        if self.config.active()?.mic.enabled {
            self.ensure_pw_version();
            let profile = self.config.active()?.clone();
            let (nodes, availability) =
                convert::mic_chain_nodes(&profile.mic, self.probe.as_ref(), self.builtin_noisegate);
            self.mic_availability = availability;
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            let handle = mic_be.recreate(&nodes)?;
            if let Some(token) = handle.child {
                self.children.track(token);
            }
        }

        self.emit(Event::MicStageSet {
            stage: crate::state::StageName::from(stage),
            enabled: on,
        });
        Ok(())
    }

    /// Set a single mic DSP parameter live (in-place via `apply_control`).
    /// Validates range against domain bounds; errors on out-of-range.
    /// `GateThreshold` triggers a full recreate (different control format between builtin/LADSPA).
    pub fn mic_set_param(&mut self, param: MicParam, value: f32) -> Result<(), EngineError> {
        let mut needs_recreate = false;
        let mut control_name_opt: Option<&'static str> = None;
        let mut node_name_opt: Option<&'static str> = None;

        {
            let engine_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&engine_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    engine_name.clone(),
                ))
            })?;
            let mic = &mut profile.mic;
            match param {
                MicParam::GainDb => {
                    if !(MIC_GAIN_MIN_DB..=MIC_GAIN_MAX_DB).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "gain_db {value} out of range {MIC_GAIN_MIN_DB}..={MIC_GAIN_MAX_DB}"
                        )));
                    }
                    mic.gain.gain_db = value;
                    node_name_opt = Some("mic_gain");
                    control_name_opt = Some("Mult");
                }
                MicParam::HighpassFreq => {
                    if !(MIC_HIGHPASS_MIN_HZ..=MIC_HIGHPASS_MAX_HZ).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "highpass_freq {value} Hz out of range {MIC_HIGHPASS_MIN_HZ}..={MIC_HIGHPASS_MAX_HZ}"
                        )));
                    }
                    mic.highpass.freq_hz = value;
                    node_name_opt = Some("mic_highpass");
                    control_name_opt = Some("Freq");
                }
                MicParam::AttenuationLimitDb => {
                    if !(MIC_ATTEN_LIMIT_MIN_DB..=MIC_ATTEN_LIMIT_MAX_DB).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "attenuation_limit_db {value} out of range {MIC_ATTEN_LIMIT_MIN_DB}..={MIC_ATTEN_LIMIT_MAX_DB}"
                        )));
                    }
                    mic.suppression.attenuation_limit_db = value;
                    node_name_opt = Some("mic_suppression");
                    control_name_opt = Some("Attenuation Limit (dB)");
                }
                MicParam::VadThreshold => {
                    if !(MIC_VAD_THRESHOLD_MIN..=MIC_VAD_THRESHOLD_MAX).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "vad_threshold {value} out of range {MIC_VAD_THRESHOLD_MIN}..={MIC_VAD_THRESHOLD_MAX}"
                        )));
                    }
                    mic.suppression.vad_threshold = value;
                    node_name_opt = Some("mic_suppression");
                    control_name_opt = Some("VAD Threshold (%)");
                }
                MicParam::VadGraceMs => {
                    if !(MIC_VAD_GRACE_MIN_MS..=MIC_VAD_GRACE_MAX_MS).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "vad_grace_ms {value} ms out of range {MIC_VAD_GRACE_MIN_MS}..={MIC_VAD_GRACE_MAX_MS}"
                        )));
                    }
                    mic.suppression.vad_grace_ms = value;
                    node_name_opt = Some("mic_suppression");
                    control_name_opt = Some("VAD Grace Period (ms)");
                }
                MicParam::VadRetroGraceMs => {
                    if !(MIC_VAD_RETRO_GRACE_MIN_MS..=MIC_VAD_RETRO_GRACE_MAX_MS).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "vad_retro_grace_ms {value} ms out of range {MIC_VAD_RETRO_GRACE_MIN_MS}..={MIC_VAD_RETRO_GRACE_MAX_MS}"
                        )));
                    }
                    mic.suppression.vad_retro_grace_ms = value;
                    node_name_opt = Some("mic_suppression");
                    control_name_opt = Some("Retroactive VAD Grace (ms)");
                }
                MicParam::GateThreshold => {
                    if !(MIC_GATE_THRESHOLD_MIN..=MIC_GATE_THRESHOLD_MAX).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "gate_threshold {value} out of range {MIC_GATE_THRESHOLD_MIN}..={MIC_GATE_THRESHOLD_MAX}"
                        )));
                    }
                    mic.gate.threshold = value;
                    // Gate threshold changes the gate topology (different control name/units
                    // between builtin noisegate and LADSPA gate_1410) → recreate, not live Props.
                    needs_recreate = true;
                }
                MicParam::CompThresholdDb => {
                    if !(MIC_COMP_THRESHOLD_MIN_DB..=MIC_COMP_THRESHOLD_MAX_DB).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "comp_threshold_db {value} dB out of range {MIC_COMP_THRESHOLD_MIN_DB}..={MIC_COMP_THRESHOLD_MAX_DB}"
                        )));
                    }
                    mic.compressor.threshold_db = value;
                    node_name_opt = Some("mic_compressor");
                    control_name_opt = Some("Threshold level (dB)");
                }
                MicParam::CompRatio => {
                    if !(MIC_COMP_RATIO_MIN..=MIC_COMP_RATIO_MAX).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "comp_ratio {value} out of range {MIC_COMP_RATIO_MIN}..={MIC_COMP_RATIO_MAX}"
                        )));
                    }
                    mic.compressor.ratio = value;
                    node_name_opt = Some("mic_compressor");
                    control_name_opt = Some("Ratio (1:n)");
                }
                MicParam::CompMakeupDb => {
                    if !(MIC_COMP_MAKEUP_MIN_DB..=MIC_COMP_MAKEUP_MAX_DB).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "comp_makeup_db {value} dB out of range {MIC_COMP_MAKEUP_MIN_DB}..={MIC_COMP_MAKEUP_MAX_DB}"
                        )));
                    }
                    mic.compressor.makeup_db = value;
                    node_name_opt = Some("mic_compressor");
                    control_name_opt = Some("Makeup gain (dB)");
                }
            }
        }
        self.save_config()?;

        // Only perform live I/O when the master switch is on.
        if self.config.active()?.mic.enabled {
            if needs_recreate {
                self.ensure_pw_version();
                let profile = self.config.active()?.clone();
                let (nodes, availability) = convert::mic_chain_nodes(
                    &profile.mic,
                    self.probe.as_ref(),
                    self.builtin_noisegate,
                );
                self.mic_availability = availability;
                let spec = convert::mic_chain_spec(&profile.mic);
                let mut mic_be = MicBackend::new(&mut self.runner, spec);
                let handle = mic_be.recreate(&nodes)?;
                if let Some(token) = handle.child {
                    self.children.track(token);
                }
            } else if let (Some(node_name), Some(control_name)) = (node_name_opt, control_name_opt)
            {
                // For gain, convert to linear; all others pass raw value.
                let apply_value = if param == MicParam::GainDb {
                    convert::db_to_linear(value)
                } else {
                    value
                };
                let profile = self.config.active()?.clone();
                let spec = convert::mic_chain_spec(&profile.mic);
                let mut mic_be = MicBackend::new(&mut self.runner, spec);
                if let Err(e) = mic_be.apply_control(node_name, control_name, apply_value) {
                    eprintln!(
                        "warning: mic_set_param apply_control failed (post-spawn race?): {e}"
                    );
                }
            }
        }

        self.emit(Event::MicParamSet { param, value });
        Ok(())
    }

    /// Set a single mic EQ band live via `apply_control` (no restart).
    pub fn mic_set_eq_band(&mut self, band: usize, cfg: EqBandConfig) -> Result<(), EngineError> {
        // Bounds guard: reject out-of-range band indices.
        if band >= arctis_audio::MAX_BANDS {
            return Err(EngineError::BadRequest(format!(
                "band index {band} out of range (0..{})",
                arctis_audio::MAX_BANDS
            )));
        }
        // Validate band
        let eq_band = convert::eq_band_from_cfg(&cfg)?;
        // Mutate config. Capture whether the band's filter TYPE (kind) changed
        // BEFORE overwriting: like the channel EQ, a mic biquad's type is the
        // filter-chain node label and can only change via a chain rebuild — live
        // apply_control only updates Freq/Q/Gain.
        let kind_changed;
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            // Seed the dense canonical defaults (correct freqs, NOT 1000 Hz)
            // while preserving any existing overrides, so unedited lower bands
            // keep their real default frequencies.
            if profile.mic.eq.len() < arctis_audio::MAX_BANDS {
                let mut dense = convert::default_eq_band_configs();
                for (i, b) in profile.mic.eq.iter().enumerate().take(dense.len()) {
                    dense[i] = b.clone();
                }
                profile.mic.eq = dense;
            }
            kind_changed = profile.mic.eq[band].kind != cfg.kind;
            profile.mic.eq[band] = cfg.clone();
        }
        self.save_config()?;

        // Only perform live I/O when the master switch is on.
        // When off, the persisted config change takes effect the next time
        // mic_set_enabled(true) or reconcile() builds the chain.
        if self.config.active()?.mic.enabled {
            if kind_changed {
                // Filter-type change → rebuild the mic chain so the new bq_*
                // label takes effect (mirrors mic_set_param's recreate branch).
                self.ensure_pw_version();
                let profile = self.config.active()?.clone();
                let (nodes, availability) = convert::mic_chain_nodes(
                    &profile.mic,
                    self.probe.as_ref(),
                    self.builtin_noisegate,
                );
                self.mic_availability = availability;
                let spec = convert::mic_chain_spec(&profile.mic);
                let mut mic_be = MicBackend::new(&mut self.runner, spec);
                let handle = mic_be.recreate(&nodes)?;
                if let Some(token) = handle.child {
                    self.children.track(token);
                }
            } else {
                // Value-only edit (Freq/Q/Gain): keep the live apply_control path.
                let profile = self.config.active()?.clone();
                let spec = convert::mic_chain_spec(&profile.mic);
                let node_name = convert::mic_eq_band_node_name(band);
                let mut mic_be = MicBackend::new(&mut self.runner, spec);
                if let Err(e) = mic_be.apply_control(&node_name, "Freq", eq_band.freq_hz) {
                    eprintln!("warning: mic_set_eq_band Freq apply_control failed: {e}");
                }
                if let Err(e) = mic_be.apply_control(&node_name, "Q", eq_band.q) {
                    eprintln!("warning: mic_set_eq_band Q apply_control failed: {e}");
                }
                if let Err(e) = mic_be.apply_control(&node_name, "Gain", eq_band.gain_db) {
                    eprintln!("warning: mic_set_eq_band Gain apply_control failed: {e}");
                }
            }
        }

        self.emit(Event::MicEqBandSet { band });
        Ok(())
    }

    /// Enable or disable the whole mic chain (master switch). Builds the Clean Mic
    /// source when enabling, removes it when disabling. Persists + emits.
    pub fn mic_set_enabled(&mut self, on: bool) -> Result<(), EngineError> {
        // Mutate config
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            profile.mic.enabled = on;
        }
        self.save_config()?;

        // Apply: mirror reconcile step5 create/remove logic exactly.
        {
            self.ensure_pw_version();
            let profile = self.config.active()?.clone();
            let (nodes, availability) =
                convert::mic_chain_nodes(&profile.mic, self.probe.as_ref(), self.builtin_noisegate);
            self.mic_availability = availability;
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            if on {
                match mic_be.create(&nodes) {
                    Ok(handle) => {
                        if let Some(token) = handle.child {
                            self.children.track(token);
                        }
                    }
                    Err(e) => {
                        eprintln!("warning: mic_set_enabled create failed (post-spawn race?): {e}");
                    }
                }
            } else {
                if let Err(e) = mic_be.remove() {
                    eprintln!("warning: mic_set_enabled remove failed (ignoring): {e}");
                }
            }
        }

        self.emit(Event::MicEnabledSet { enabled: on });
        Ok(())
    }

    /// Set (or clear) the hardware mic capture target.
    /// Capture target change → full recreate.
    pub fn mic_set_hw_mic(&mut self, hw_mic: Option<String>) -> Result<(), EngineError> {
        // Mutate config
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            profile.mic.hw_mic = hw_mic;
        }
        self.save_config()?;

        // Only perform live graph I/O (recreate) when the master switch is on.
        // When off, the persisted config change takes effect the next time
        // mic_set_enabled(true) or reconcile() builds the chain.
        let hw_mic_snapshot = self.config.active()?.mic.hw_mic.clone();
        if self.config.active()?.mic.enabled {
            self.ensure_pw_version();
            let profile = self.config.active()?.clone();
            let (nodes, availability) =
                convert::mic_chain_nodes(&profile.mic, self.probe.as_ref(), self.builtin_noisegate);
            self.mic_availability = availability;
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            let handle = mic_be.recreate(&nodes)?;
            if let Some(token) = handle.child {
                self.children.track(token);
            }
        }
        self.emit(Event::MicHwMicSet {
            hw_mic: hw_mic_snapshot,
        });
        Ok(())
    }

    /// Change the active noise-suppression backend. Triggers a full chain recreate when
    /// the master switch is on (topology change: different LADSPA plugin).
    pub fn mic_set_suppression_backend(
        &mut self,
        backend: crate::state::SuppressionBackend,
    ) -> Result<(), EngineError> {
        // Mutate config
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            profile.mic.suppression.backend = match backend {
                crate::state::SuppressionBackend::DeepFilter => {
                    arctis_config::SuppressionBackend::DeepFilter
                }
                crate::state::SuppressionBackend::Rnnoise => {
                    arctis_config::SuppressionBackend::Rnnoise
                }
            };
        }
        self.save_config()?;

        // Recreate the chain if the master switch is on (topology change).
        if self.config.active()?.mic.enabled {
            self.ensure_pw_version();
            let profile = self.config.active()?.clone();
            let (nodes, availability) =
                convert::mic_chain_nodes(&profile.mic, self.probe.as_ref(), self.builtin_noisegate);
            self.mic_availability = availability;
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            let handle = mic_be.recreate(&nodes)?;
            if let Some(token) = handle.child {
                self.children.track(token);
            }
        }

        self.emit(Event::MicSuppressionBackendSet { backend });
        Ok(())
    }

    /// Apply a named factory mic preset to the active profile.
    ///
    /// Overlays `gain`, `highpass`, `suppression`, `compressor`, `gate`, `eq_enabled`,
    /// and `eq` from the preset. `mic.enabled` and `mic.hw_mic` are **never** touched.
    /// Persists the config and, when the master switch is on, rebuilds the mic chain live.
    pub fn apply_mic_preset(&mut self, name: &str) -> Result<(), EngineError> {
        // Find the named preset in the factory catalog.
        let preset = crate::presets::factory_mic_presets()
            .into_iter()
            .find(|p| p.name == name)
            .ok_or_else(|| EngineError::BadRequest(format!("mic preset not found: {name}")))?;

        // Overlay preset fields onto the active profile's mic config.
        // Preserve `enabled` and `hw_mic`.
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            let mic = &mut profile.mic;
            mic.gain = preset.gain;
            mic.highpass = preset.highpass;
            mic.suppression = preset.suppression;
            mic.compressor = preset.compressor;
            mic.gate = preset.gate;
            mic.eq_enabled = preset.eq_enabled;
            mic.eq = preset.eq;
            // mic.enabled and mic.hw_mic are intentionally preserved.
        }

        self.save_config()?;

        // Rebuild the live mic chain only when the master switch is on.
        if self.config.active()?.mic.enabled {
            self.ensure_pw_version();
            let profile = self.config.active()?.clone();
            let (nodes, availability) =
                convert::mic_chain_nodes(&profile.mic, self.probe.as_ref(), self.builtin_noisegate);
            self.mic_availability = availability;
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            let handle = mic_be.recreate(&nodes)?;
            if let Some(token) = handle.child {
                self.children.track(token);
            }
        }

        self.emit(Event::MicPresetApplied {
            name: name.to_string(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;

    // ─────────────────────────────────────────────
    // Task 4 TDD: mic source reconcile + engine methods
    // ─────────────────────────────────────────────

    use crate::engine::MicParam;

    use arctis_audio::MockPluginProbe;

    /// LS Node output containing the arctis_clean_mic source.
    fn ls_with_mic() -> String {
        [
            "id 40, type PipeWire:Interface:Node/3\n    node.name = \"alsa_output.pci\"\n",
            "id 71, type PipeWire:Interface:Node/3\n    node.name = \"arctis_clean_mic\"\n",
            "id 72, type PipeWire:Interface:Node/3\n    node.name = \"arctis_clean_mic.capture\"\n",
        ]
        .concat()
    }

    /// LS Node output with NO mic node.
    fn ls_without_mic() -> String {
        "id 40, type PipeWire:Interface:Node/3\n    node.name = \"alsa_output.pci\"\n".to_string()
    }

    /// Bugfix (mic): changing a mic EQ band's filter TYPE must rebuild the mic
    /// chain (the bq_* label is fixed at build time); value-only edits keep the
    /// live apply_control path.
    #[test]
    fn mic_set_eq_band_kind_change_rebuilds_chain() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_eq_kind_change");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // mic.enabled = true, mic.eq empty → dense band 0 default kind = "lowshelf".
        let cfg = make_config_mic_enabled();

        // recreate(): remove() [source_exists ls + find_node_id ls + destroy + pkill]
        //           + create() [source_exists ls → default-empty → absent → spawn].
        let ls = ls_with_mic();
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // remove: source_exists (present)
            .with_output(0, &ls, ""); // remove: find_node_id (present)

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.seed_pw_version((1, 4, 11)); // keep ensure_pw_version() a no-op

        let band_cfg = EqBandConfig {
            kind: "peaking".to_string(), // CHANGED from default "lowshelf" at band 0
            freq_hz: 31.0,
            q: 1.0,
            gain_db: 2.0,
        };
        engine
            .mic_set_eq_band(0, band_cfg)
            .expect("mic_set_eq_band kind-change should succeed");

        // Rebuild: fresh chain spawned, old node destroyed, child tracked.
        assert_eq!(
            engine.runner.spawned.len(),
            1,
            "mic kind change must rebuild the chain"
        );
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c.len() >= 2 && c[0] == "pw-cli" && c[1] == "destroy"),
            "mic kind change must destroy the old node"
        );
        assert_eq!(
            engine.children.len(),
            1,
            "mic rebuild must track the new child"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Bugfix guard (mic): a value-only mic EQ edit keeps the live apply_control
    /// path — NO chain rebuild.
    #[test]
    fn mic_set_eq_band_value_only_keeps_live_apply() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_eq_value_only");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_enabled();

        // apply_control path: 3 sets (Freq/Q/Gain), each find_node_id ls + set.
        let ls = ls_with_mic();
        let runner = MockRunner::new()
            .with_output(0, &ls, "")
            .with_output(0, "", "")
            .with_output(0, &ls, "")
            .with_output(0, "", "")
            .with_output(0, &ls, "")
            .with_output(0, "", "");

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.seed_pw_version((1, 4, 11));

        let band_cfg = EqBandConfig {
            kind: "lowshelf".to_string(), // SAME as default band 0 kind
            freq_hz: 31.0,
            q: 1.0,
            gain_db: 4.0, // value-only change
        };
        engine
            .mic_set_eq_band(0, band_cfg)
            .expect("mic_set_eq_band value-only should succeed");

        // No rebuild.
        assert!(
            engine.runner.spawned.is_empty(),
            "value-only mic edit must NOT rebuild the chain"
        );
        assert!(
            !engine
                .runner
                .calls
                .iter()
                .any(|c| c.len() >= 2 && c[0] == "pw-cli" && c[1] == "destroy"),
            "value-only mic edit must NOT destroy the node"
        );
        assert_eq!(engine.children.len(), 0, "value-only mic edit tracks no child");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Queue outputs for a 3-channel reconcile with mic DISABLED.
    fn queue_reconcile_with_mic_disabled(runner: MockRunner) -> MockRunner {
        queue_reconcile_present(runner)
    }

    /// Queue outputs for a 4-channel reconcile with mic ENABLED and source absent.
    /// Engine::new seeds "aux" → 4 channels (game/chat/media/aux).
    /// Step5: create() → source_exists() (1 ls, absent) → spawn (goes to spawned)
    fn queue_reconcile_with_mic_enabled_absent(runner: MockRunner) -> MockRunner {
        let ls = ls_all_present();
        let ls_mic_absent = ls_without_mic();
        let mut r = runner;
        // detect_headset_sink: pw-metadata 0 + pw-dump (no SteelSeries → detect returns None)
        r = r.with_output(0, "", ""); // pw-metadata 0
        r = r.with_output(0, "[]", ""); // pw-dump []
        // Phase 1: 4 ls (all present, including aux seeded by Engine::new)
        for _ in 0..4 {
            r = r.with_output(0, &ls, "");
        }
        // Phase 2 + 2b interleaved: per channel (4), EQ apply then volume/mute apply
        for _ in 0..4 {
            // Phase 2: EQ apply (1 ls + 10 band sets)
            r = r.with_output(0, &ls, "");
            for _ in 0..10 {
                r = r.with_output(0, "", "");
            }
            // Phase 2b: volume/mute apply (1 ls + 1 Props set)
            r = r.with_output(0, &ls, ""); // find_node_id
            r = r.with_output(0, "", ""); // Props set
        }
        // Phase 5: mic enabled → create() → source_exists() (absent) → spawn
        r = r.with_output(0, &ls_mic_absent, "");
        // Phase 6: surround disabled → remove() → source_exists() → 1 ls (surround absent)
        r = r.with_output(0, &ls, "");
        r
    }

    /// Test 1: reconcile with mic disabled calls MicBackend::remove() (source_exists only, no spawn).
    #[test]
    fn reconcile_passthrough_when_mic_disabled_removes_source() {
        let ls = ls_all_present();
        let ls_no_mic = ls_without_mic();

        let runner = queue_reconcile_with_mic_disabled(MockRunner::new());
        let cfg = make_config_no_eq_no_routes(); // mic.enabled = false by default
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.reconcile().expect("reconcile should succeed");

        // Verify step5 ran: last call is ls Node (source_exists check by remove())
        let calls = &engine.runner.calls;
        let last = calls.last().expect("at least one call");
        assert_eq!(
            last,
            &vec!["pw-cli", "ls", "Node"],
            "last call must be mic source_exists ls"
        );

        // No mic spawn (mic disabled → remove path, source absent → no destroy either)
        // spawned is only channel sinks (none in this case since sinks are "present")
        assert!(
            engine.runner.spawned.is_empty(),
            "no spawns when mic is disabled and channels are present"
        );

        let _ = &ls;
        let _ = &ls_no_mic; // suppress warnings
    }

    /// Test 2: reconcile with mic enabled (passthrough) → spawns arctis_clean_mic.conf.
    #[test]
    fn reconcile_builds_clean_mic_when_enabled() {
        let ls_mic_absent = ls_without_mic();

        // Queue outputs for mic-enabled reconcile
        let runner = queue_reconcile_with_mic_enabled_absent(MockRunner::new());
        let cfg = make_config_mic_enabled();
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));

        // Count spawns before reconcile (channels absent → 3 channel spawns if absent)
        // Since channels ARE "present" (queue_reconcile_with_mic_enabled_absent uses ls_all_present),
        // only the mic spawn occurs.
        engine.reconcile().expect("reconcile should succeed");

        let spawned = &engine.runner.spawned;
        // The mic source should be spawned
        assert!(
            spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "expected a spawn of arctis_clean_mic.conf, got: {spawned:?}"
        );
        // children tracks exactly the mic token (channels were already present → no channel tokens)
        assert_eq!(engine.children.len(), 1, "only the mic source token");

        let _ = &ls_mic_absent;
    }

    /// Test 3: suppression enabled but probe returns none → spawn still fires (chain minus suppression);
    ///         state marks suppression unavailable.
    #[test]
    fn reconcile_drops_unavailable_rnnoise_but_still_builds() {
        let mut cfg = make_config_mic_enabled();
        // Enable suppression (default backend = DeepFilter) in config
        cfg.profiles[0].mic.suppression.enabled = true;

        // Queue for mic-enabled absent reconcile (deepfilter probe is none → dropped from chain)
        let runner = queue_reconcile_with_mic_enabled_absent(MockRunner::new());
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        // Spawn still fires (chain has passthrough fallback)
        let spawned = &engine.runner.spawned;
        assert!(
            spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "spawn must fire even when suppression is unavailable"
        );

        // state reports suppression unavailable
        let state = engine.state();
        let suppression_stage = state
            .mic
            .stages
            .iter()
            .find(|s| s.kind == crate::state::StageName::Suppression);
        let s = suppression_stage.expect("suppression must appear in stages");
        assert!(!s.available, "suppression must be marked unavailable");
        assert!(s.enabled, "suppression must show as enabled in config");
    }

    /// Test 4: mic_set_stage_enabled → persists config + recreates (remove + spawn observed).
    #[test]
    fn mic_set_stage_enabled_recreates_and_persists() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_stage_set");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_mic_enabled();
        cfg.profiles[0].mic.gain.enabled = false; // start with gain OFF

        // Queue for mic_set_stage_enabled(Gain, true):
        //   recreate() = remove() + create()
        //   remove(): source_exists() (1 ls, WITH mic) + find_node_id (1 ls) + destroy (1) + pkill (1) = 4 calls
        //   create(): source_exists() (1 ls, absent after remove) + spawn = 1 call
        let ls_with_mic = ls_with_mic();
        let ls_absent = ls_without_mic();

        let runner = MockRunner::new()
            // remove: source_exists (present)
            .with_output(0, &ls_with_mic, "")
            // remove: find_node_id
            .with_output(0, &ls_with_mic, "")
            // remove: pw-cli destroy 71
            .with_output(0, "", "")
            // remove: pkill -f <conf>
            .with_output(0, "", "")
            // create: source_exists (absent after remove)
            .with_output(0, &ls_absent, "");

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));
        engine
            .mic_set_stage_enabled(StageKind::Gain, true)
            .expect("mic_set_stage_enabled should succeed");

        // Config persisted to disk
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        // After reload, gain should be enabled (check raw TOML string)
        assert!(
            saved_str.contains("enabled = true"),
            "persisted config must show gain enabled (enabled = true present)"
        );

        // remove was called (destroy present) and create spawned new conf
        let calls = &engine.runner.calls;
        assert!(
            calls
                .iter()
                .any(|c| c.len() >= 3 && c[0] == "pw-cli" && c[1] == "destroy"),
            "destroy must be called during recreate"
        );
        let spawned = &engine.runner.spawned;
        assert!(
            spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "create must spawn arctis_clean_mic.conf after remove"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Test 5: mic_set_param emits exact pw-cli s <id> Props argv for VAD threshold.
    #[test]
    fn mic_set_param_emits_exact_props() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_param_set");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_mic_enabled();
        // Use Rnnoise backend so VadThreshold hits the VAD control path
        cfg.profiles[0].mic.suppression.enabled = true;
        cfg.profiles[0].mic.suppression.backend = arctis_config::SuppressionBackend::Rnnoise;
        cfg.profiles[0].mic.suppression.vad_threshold = 40.0;

        let ls_with_mic = ls_with_mic();

        // mic_set_param(VadThreshold, 55.0):
        //   apply_control("mic_suppression", "VAD Threshold (%)", 55.0)
        //   → find_node_id (1 ls) + pw-cli s 71 Props … (1 call)
        let runner = MockRunner::new()
            .with_output(0, &ls_with_mic, "") // find_node_id
            .with_output(0, "", ""); // set Props

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_param(MicParam::VadThreshold, 55.0)
            .expect("mic_set_param should succeed");

        let calls = &engine.runner.calls;
        // Last call should be: pw-cli s 71 Props { params = [ "mic_suppression:VAD Threshold (%)" 55.0 ] }
        let last = calls.last().expect("at least one call");
        assert_eq!(last[0], "pw-cli");
        assert_eq!(last[1], "s");
        assert_eq!(last[2], "71", "node id must be 71 (from ls fixture)");
        assert_eq!(last[3], "Props");
        assert_eq!(
            last[4], "{ params = [ \"mic_suppression:VAD Threshold (%)\" 55.0 ] }",
            "Props JSON must exactly match"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Test 6: shutdown kills the mic source token too.
    #[test]
    fn shutdown_kills_mic_source_too() {
        let ls_absent = ls_without_mic();

        // Reconcile with mic enabled and absent → spawn (mic token tracked in children)
        let runner = queue_reconcile_with_mic_enabled_absent(MockRunner::new());
        let cfg = make_config_mic_enabled();
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.reconcile().expect("reconcile should succeed");

        // channels are present so only the mic spawn occurs
        let initial_children = engine.children.len();
        assert!(initial_children >= 1, "at least mic child tracked");

        let initial_spawned = engine.runner.spawned.len();
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "mic must have been spawned"
        );

        // Shutdown kills all including the mic token
        engine.shutdown().expect("shutdown should succeed");

        assert_eq!(
            engine.runner.killed.len(),
            initial_spawned,
            "shutdown must kill all spawned processes including mic source"
        );
        assert_eq!(
            engine.children.len(),
            0,
            "children must be empty after shutdown"
        );

        let _ = &ls_absent;
    }

    /// Test 7: state().mic.stages reports suppression unavailable when probe is missing.
    #[test]
    fn state_reports_mic_stage_availability() {
        let mut cfg = make_config_mic_enabled();
        cfg.profiles[0].mic.suppression.enabled = true; // request suppression

        // Queue for reconcile with mic enabled, deepfilter probe absent → suppression dropped
        let runner = queue_reconcile_with_mic_enabled_absent(MockRunner::new());
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        let state = engine.state();
        assert!(state.mic.enabled, "mic must show enabled");

        let suppression = state
            .mic
            .stages
            .iter()
            .find(|s| s.kind == crate::state::StageName::Suppression)
            .expect("suppression stage must be in state");

        assert!(suppression.enabled, "suppression enabled in config");
        assert!(
            !suppression.available,
            "suppression must be unavailable (probe returns false)"
        );
    }

    /// Regression: disabling a mic stage must NOT report it unavailable. A disabled
    /// stage has no reconcile availability entry (the chain build skips probing it),
    /// so `state()` must default it to available — otherwise the UI greyed the card
    /// and locked the enable toggle, trapping the stage OFF with no way to re-enable
    /// it (the reported gate / suppression / compressor bug).
    #[test]
    fn disabled_mic_stages_default_available_so_toggle_stays_live() {
        // mic_enabled_passthrough() = chain on, every stage off.
        let cfg = make_config_mic_enabled();
        // No reconcile → mic_availability is empty → each stage hits its unwrap_or default.
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let state = engine.state();
        for kind in [
            crate::state::StageName::Gate,
            crate::state::StageName::Compressor,
            crate::state::StageName::Suppression,
        ] {
            let s = state
                .mic
                .stages
                .iter()
                .find(|s| s.kind == kind)
                .unwrap_or_else(|| panic!("{kind:?} stage must be present"));
            assert!(!s.enabled, "{kind:?} is disabled in this config");
            assert!(
                s.available,
                "disabled {kind:?} must default to available so its enable toggle stays live"
            );
        }
    }

    // ─────────────────────────────────────────────
    // Task 5b TDD: mic_set_enabled (master switch)
    // ─────────────────────────────────────────────

    /// Build a 3-channel config with mic master switch DISABLED (default).
    fn make_config_mic_disabled() -> Config {
        Config {
            version: arctis_config::CURRENT_VERSION,
            active_profile: "default".into(),
            profiles: vec![Profile {
                name: "default".into(),
                channels: vec![
                    ChannelConfig {
                        id: "game".into(),
                        node_name: "Arctis_Game".into(),
                        description: "Game".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        volume_pct: 100,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        volume_pct: 100,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        volume_pct: 100,
                        muted: false,
                    },
                ],
                routes: vec![],
                mic: arctis_config::MicChainConfig {
                    enabled: false,
                    hw_mic: Some("alsa_input.hw_mic".to_string()),
                    ..Default::default()
                },
                surround: arctis_config::SurroundConfig::default(),
                master_volume_db: 0.0,
                master_volume_pct: 100,
                master_mute: false,
                chatmix_position: 4,
                default_sink_channel: None,
            }],
            eq_presets: vec![],
            dial_controls_balance: true,
            knob_controls_master: true,
        }
    }

    /// Test 5b-1: mic_set_enabled(true) from master-off spawns the mic source and persists.
    #[test]
    fn mic_set_enabled_true_builds_source_and_persists() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_set_enabled_true");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_disabled(); // mic.enabled = false

        // mic_set_enabled(true) → create():
        //   source_exists() (1 ls, absent) → spawn
        let ls_absent = ls_without_mic();
        let runner = MockRunner::new().with_output(0, &ls_absent, "");

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_enabled(true)
            .expect("mic_set_enabled(true) should succeed");

        // Config persisted to disk with mic.enabled = true
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("enabled = true"),
            "persisted config must show mic.enabled = true"
        );

        // Reload and confirm
        let reloaded = arctis_config::store::load().expect("reload must succeed");
        assert!(
            reloaded
                .active()
                .expect("active profile must exist")
                .mic
                .enabled,
            "reloaded config must show mic.enabled = true"
        );

        // Mic source was spawned
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "mic source must be spawned when enabling"
        );
        // One child tracked
        assert_eq!(engine.children.len(), 1, "one mic child must be tracked");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Test 5b-2: mic_set_enabled(false) from master-on takes the remove path.
    #[test]
    fn mic_set_enabled_false_removes_source() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_set_enabled_false");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_enabled(); // mic.enabled = true

        // mic_set_enabled(false) → remove():
        //   source_exists() (1 ls, absent, so no destroy needed)
        let ls_absent = ls_without_mic();
        let runner = MockRunner::new().with_output(0, &ls_absent, "");

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_enabled(false)
            .expect("mic_set_enabled(false) should succeed");

        // Config persisted with mic.enabled = false
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");

        // No spawn (remove path, source already absent)
        assert!(
            engine.runner.spawned.is_empty(),
            "no spawn on disable when source already absent"
        );
        assert_eq!(engine.children.len(), 0, "no child tracked on disable");

        // Reload and confirm
        let reloaded = arctis_config::store::load().expect("reload must succeed");
        assert!(
            !reloaded
                .active()
                .expect("active profile must exist")
                .mic
                .enabled,
            "reloaded config must show mic.enabled = false"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Fix 2+3 test A: mic_set_stage_enabled while master is OFF → config persisted, no spawn,
    /// children unchanged, event emitted.
    #[test]
    fn mic_set_stage_enabled_when_master_off_persists_only_no_spawn() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("stage_master_off");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Master switch is OFF; gain stage starts OFF.
        let cfg = make_config_mic_disabled();

        // No runner outputs needed — master off path skips all graph I/O.
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine =
            Engine::with_probe(MockRunner::new(), cfg, Box::new(MockPluginProbe::none()));
        engine.set_event_sink(tx);

        engine
            .mic_set_stage_enabled(StageKind::Gain, true)
            .expect("mic_set_stage_enabled should succeed when master is off");

        // Config persisted with gain.enabled = true
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("enabled = true"),
            "persisted config must show gain enabled"
        );

        // No spawn: master off → no recreate
        assert!(
            engine.runner.spawned.is_empty(),
            "no spawn when master is off"
        );
        // No graph I/O calls (no pw-cli destroy / no create ls)
        assert!(
            engine.runner.calls.is_empty(),
            "no pw-cli calls when master is off"
        );
        // No child tokens tracked
        assert_eq!(engine.children.len(), 0, "no children when master is off");

        // Event must still be emitted
        let event = rx.try_recv().expect("MicStageSet event must be emitted");
        assert_eq!(
            event,
            crate::state::Event::MicStageSet {
                stage: crate::state::StageName::Gain,
                enabled: true,
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Fix 2+3 test B: mic_set_param while master is OFF → config persisted, no pw-cli s Props call,
    /// event emitted.
    #[test]
    fn mic_set_param_when_master_off_persists_only_no_props() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("param_master_off");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_mic_disabled(); // master off
        cfg.profiles[0].mic.highpass.enabled = true;
        cfg.profiles[0].mic.highpass.freq_hz = 80.0;

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine =
            Engine::with_probe(MockRunner::new(), cfg, Box::new(MockPluginProbe::none()));
        engine.set_event_sink(tx);

        engine
            .mic_set_param(MicParam::HighpassFreq, 120.0)
            .expect("mic_set_param should succeed when master is off");

        // Config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");

        // No pw-cli s ... Props call
        assert!(
            engine.runner.calls.is_empty(),
            "no pw-cli calls (no apply_control) when master is off"
        );

        // Event must still be emitted
        let event = rx.try_recv().expect("MicParamSet event must be emitted");
        assert_eq!(
            event,
            crate::state::Event::MicParamSet {
                param: MicParam::HighpassFreq,
                value: 120.0,
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    // ── Task 2 new engine tests ───────────────────────────────────────────────

    /// Task 2 test A: mic_set_suppression_backend persists, recreates, emits event.
    #[test]
    fn mic_set_suppression_backend_persists_recreates_emits() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("suppression_backend");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_enabled(); // mic.enabled = true
        let ls_with_mic = ls_with_mic();
        let ls_absent = ls_without_mic();

        // recreate() = remove() + create():
        // remove: source_exists (present), find_node_id, destroy, pkill = 4 calls
        // create: source_exists (absent) = 1 call + spawn
        let runner = MockRunner::new()
            .with_output(0, &ls_with_mic, "") // remove: source_exists
            .with_output(0, &ls_with_mic, "") // remove: find_node_id
            .with_output(0, "", "") // remove: destroy
            .with_output(0, "", "") // remove: pkill
            .with_output(0, &ls_absent, ""); // create: source_exists

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.set_event_sink(tx);
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));

        engine
            .mic_set_suppression_backend(crate::state::SuppressionBackend::Rnnoise)
            .expect("mic_set_suppression_backend should succeed");

        // Config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("rnnoise"),
            "persisted config must contain rnnoise backend"
        );

        // Recreate occurred: destroy was called
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c.len() >= 3 && c[1] == "destroy"),
            "destroy must be called during recreate"
        );

        // Event emitted
        let event = rx
            .try_recv()
            .expect("MicSuppressionBackendSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::MicSuppressionBackendSet {
                backend: crate::state::SuppressionBackend::Rnnoise,
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Task 2 test B: mic_set_param(AttenuationLimitDb) emits Props for mic_suppression.
    #[test]
    fn mic_set_param_attenuation_limit_emits_exact_props() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("atten_limit");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_mic_enabled();
        cfg.profiles[0].mic.suppression.enabled = true;
        cfg.profiles[0].mic.suppression.backend = arctis_config::SuppressionBackend::DeepFilter;
        cfg.profiles[0].mic.suppression.attenuation_limit_db = 100.0;

        let ls_with_mic = ls_with_mic();
        let runner = MockRunner::new()
            .with_output(0, &ls_with_mic, "") // find_node_id
            .with_output(0, "", ""); // set Props

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_param(MicParam::AttenuationLimitDb, 80.0)
            .expect("AttenuationLimitDb must succeed");

        let last = engine.runner.calls.last().expect("at least one call");
        assert_eq!(last[3], "Props");
        assert!(
            last[4].contains("mic_suppression:Attenuation Limit (dB)"),
            "Props must reference mic_suppression:Attenuation Limit (dB), got: {}",
            last[4]
        );
        assert!(
            last[4].contains("80"),
            "Props must contain the value 80, got: {}",
            last[4]
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Task 2 test C: state().mic.suppression_backend and available_suppression_backends.
    #[test]
    fn state_reports_suppression_backend_and_availability() {
        use crate::state::SuppressionBackend;
        use arctis_audio::MockPluginProbe;
        // Use a probe that reports DeepFilter available but not RNNoise
        let probe = MockPluginProbe::with([arctis_audio::DEEPFILTER_PLUGIN_BASENAME]);
        let mut cfg = make_config_mic_enabled();
        cfg.profiles[0].mic.suppression.backend = arctis_config::SuppressionBackend::DeepFilter;
        let mut engine = Engine::with_probe(MockRunner::new(), cfg, Box::new(probe));
        let state = engine.state();
        assert_eq!(
            state.mic.suppression_backend,
            SuppressionBackend::DeepFilter,
            "suppression_backend must match config"
        );
        assert!(
            state
                .mic
                .available_suppression_backends
                .contains(&SuppressionBackend::DeepFilter),
            "DeepFilter must appear in available backends when probe reports it present"
        );
        assert!(
            !state
                .mic
                .available_suppression_backends
                .contains(&SuppressionBackend::Rnnoise),
            "Rnnoise must not appear in available backends when probe reports it absent"
        );
    }

    /// Task 2 test D: gate threshold change recreates (not live Props).
    #[test]
    fn mic_set_param_gate_threshold_recreates_not_live_props() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("gate_threshold_recreate");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_enabled(); // gate disabled by default

        // recreate(): remove (source absent → no-op 1 ls) + create (absent → spawn)
        let ls_absent = ls_without_mic();
        let runner = MockRunner::new()
            .with_output(0, &ls_absent, "") // remove: source_exists (absent, no-op)
            .with_output(0, &ls_absent, ""); // create: source_exists (absent → spawn)

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_param(MicParam::GateThreshold, 0.005)
            .expect("GateThreshold must succeed");

        // Must have spawned (recreate happened), no direct Props set call
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "recreate must spawn arctis_clean_mic.conf for gate threshold change"
        );
        // No Props set call (no pw-cli s ... Props call)
        assert!(
            !engine
                .runner
                .calls
                .iter()
                .any(|c| c.len() >= 4 && c[1] == "s" && c[3] == "Props"),
            "gate threshold must NOT emit Props (must recreate)"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Task 2 test E: mic_set_enabled emits Event::MicEnabledSet.
    #[test]
    fn mic_set_enabled_emits_event() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_set_enabled_event");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_disabled();

        let ls_absent = ls_without_mic();
        let runner = MockRunner::new().with_output(0, &ls_absent, "");

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.set_event_sink(tx);

        engine
            .mic_set_enabled(true)
            .expect("mic_set_enabled should succeed");

        let event = rx.try_recv().expect("MicEnabledSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::MicEnabledSet { enabled: true },
            "event must be MicEnabledSet {{ enabled: true }}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    // ─────────────────────────────────────────────
    // Task 2: dense fixed-10-band mic EQ model
    // ─────────────────────────────────────────────

    #[test]
    fn state_returns_ten_dense_mic_eq_bands() {
        let mut engine = Engine::new(arctis_audio::MockRunner::new(), make_config_no_eq_no_routes());
        let st = engine.state();
        assert_eq!(st.mic.eq_bands.len(), 10, "mic EQ must report 10 dense bands");
        assert_eq!(st.mic.eq_bands[0].freq_hz, 31.0);
        assert_eq!(st.mic.eq_bands[9].freq_hz, 16000.0);
    }

    #[test]
    fn set_mic_eq_band_rejects_out_of_range_index() {
        let mut engine = Engine::new(arctis_audio::MockRunner::new(), make_config_no_eq_no_routes());
        let band = arctis_config::EqBandConfig {
            kind: "peaking".into(),
            freq_hz: 1000.0,
            q: 1.0,
            gain_db: 0.0,
        };
        assert!(engine.mic_set_eq_band(10, band).is_err());
    }

    // ─────────────────────────────────────────────
    // Task 4 TDD: apply_mic_preset
    // ─────────────────────────────────────────────

    /// apply_mic_preset overlays DSP fields + triggers live rebuild, while preserving
    /// mic.enabled and mic.hw_mic. Unknown preset name → BadRequest.
    ///
    /// The test uses `Engine::new(MockRunner::new(), cfg)` (empty queue).
    /// MockRunner returns (0,"","") for all calls — so the live rebuild path succeeds:
    ///   - ensure_pw_version(): pipewire --version → "" → version stays None (OK)
    ///   - recreate(): remove() source_exists → "" → absent (skip destroy);
    ///     create() source_exists → "" → absent → write conf → spawn
    #[test]
    fn apply_mic_preset_overlays_and_preserves_enabled_and_hwmic() {
        let _l = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_preset_apply");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Start with mic enabled + a pinned hw_mic that must survive the preset apply.
        let mut cfg = make_config_mic_disabled();
        cfg.profiles[0].mic.enabled = true;
        cfg.profiles[0].mic.hw_mic = Some("alsa_input.keepme".into());

        // MockRunner with no queued outputs → returns (0,"","") for every call,
        // which is "tolerant" for the mic rebuild (source seen as absent → spawn).
        let mut engine = Engine::new(MockRunner::new(), cfg);

        // Apply the "Less Nasal" preset.
        engine.apply_mic_preset("Less Nasal").expect("apply_mic_preset must succeed for a known preset");

        let st = engine.state();

        // Catalog is populated: all factory mic presets present.
        assert!(
            st.mic_presets.iter().any(|p| p.name == "Walkie Talkie"),
            "mic_presets must include Walkie Talkie from the factory catalog"
        );

        // EQ was overlaid: Less Nasal has 10 bands → state always returns 10 eq_bands.
        assert_eq!(st.mic.eq_bands.len(), 10, "mic eq_bands must be 10 after preset apply");

        // Suppression is enabled in Less Nasal preset.
        let supp = st
            .mic
            .stages
            .iter()
            .find(|s| s.kind == crate::state::StageName::Suppression)
            .expect("suppression stage must appear in mic.stages");
        assert!(supp.enabled, "suppression must be enabled after applying Less Nasal");

        // mic.enabled and mic.hw_mic must be preserved (not touched by the overlay).
        assert!(st.mic.enabled, "mic.enabled must be preserved after preset apply");
        assert_eq!(
            st.mic.hw_mic,
            Some("alsa_input.keepme".into()),
            "mic.hw_mic must be preserved after preset apply"
        );

        // Unknown preset name → BadRequest.
        assert!(
            matches!(
                engine.apply_mic_preset("Nope"),
                Err(EngineError::BadRequest(_))
            ),
            "unknown mic preset name must yield BadRequest"
        );

        // Config was persisted to disk.
        assert!(
            tmp.join("config.toml").exists(),
            "config.toml must be written after preset apply"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }
}
