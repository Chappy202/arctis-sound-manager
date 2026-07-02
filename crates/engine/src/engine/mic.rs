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
mod tests;
