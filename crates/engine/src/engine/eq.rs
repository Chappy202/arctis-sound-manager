//! Per-channel parametric EQ: band edits, bulk sets, named presets.
use super::*;

impl<R: CommandRunner> Engine<R> {
    /// Mutate one EQ band in the active profile's channel, persist config, apply live via audio.
    pub fn set_eq_band(
        &mut self,
        channel_id: &str,
        band: usize,
        cfg: EqBandConfig,
    ) -> Result<(), EngineError> {
        // Update in-memory config. Capture whether the band's filter TYPE (kind)
        // changed BEFORE overwriting it: a biquad's type is the filter-chain node
        // label (bq_peaking/bq_lowshelf/bq_highshelf), fixed at chain-build time,
        // and CANNOT be switched by a live `pw-cli s … Props` set — only
        // Freq/Q/Gain are live-updatable. A kind change therefore needs a sink
        // rebuild; a value-only change keeps the fast live path (G3).
        let kind_changed;
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            let channel = profile
                .channels
                .iter_mut()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found: {channel_id}"))
                })?;
            if band >= arctis_audio::MAX_BANDS {
                return Err(EngineError::BadRequest(format!(
                    "band index {band} out of range (0..{})",
                    arctis_audio::MAX_BANDS
                )));
            }
            // Seed the dense canonical defaults (correct freqs, NOT 1000 Hz)
            // while preserving any existing overrides, so unedited lower bands
            // keep their real default frequencies.
            if channel.eq.len() < arctis_audio::MAX_BANDS {
                channel.eq = convert::dense_eq_bands(channel);
            }
            kind_changed = channel.eq[band].kind != cfg.kind;
            channel.eq[band] = cfg.clone();
        }
        // Persist
        self.save_config()?;
        // Apply live via AudioBackend
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.active()?.clone();
            let channel = profile
                .channels
                .iter()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found: {channel_id}"))
                })?;
            let def = convert::channel_def_from_cfg(channel);
            let spec = def.sink_spec();
            let mut be = AudioBackend::new(&mut self.runner, spec);
            if kind_changed {
                // Rebuild ONLY this channel's sink so the new bq_* label takes
                // effect (reuses AudioBackend::recreate, the same teardown+respawn
                // ChannelManager::set_output uses). The new conf is rendered from
                // the full dense band model, so all live values are preserved.
                let eq_model = convert::eq_model_for(channel)?;
                let handle = be.recreate(&eq_model)?;
                if let Some(t) = handle.child {
                    self.children.track(t);
                }
            } else {
                // Value-only edit (Freq/Q/Gain): keep the low-latency live path.
                // The auto-preamp is recomputed from the FULL dense model so the
                // headroom always compensates the largest boost (live, G3).
                let eq_band = convert::eq_band_from_cfg(&cfg)?;
                let eq_model = convert::eq_model_for(channel)?;
                be.apply_band_with_preamp(band, &eq_band, &eq_model)?;
            }
            let _ = active_name; // suppress unused warning
        }
        // Emit event
        self.emit(Event::EqBandSet {
            channel_id: channel_id.to_string(),
            band,
        });
        Ok(())
    }

    /// Save the current EQ bands of `channel_id` in the active profile as a named preset.
    /// Overwrites if a preset with that name already exists.
    pub fn save_eq_preset(&mut self, name: &str, channel_id: &str) -> Result<(), EngineError> {
        // Find channel in active profile
        let bands = {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            profile
                .channels
                .iter()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| EngineError::BadRequest(format!("channel not found: {channel_id}")))?
                .eq
                .clone()
        };

        let preset = arctis_config::EqPreset {
            name: name.to_string(),
            kind_hint: None,
            bands,
        };

        // Overwrite if name exists, otherwise push
        if let Some(existing) = self.config.eq_presets.iter_mut().find(|p| p.name == name) {
            *existing = preset;
        } else {
            self.config.eq_presets.push(preset);
        }

        self.save_config()?;
        self.emit(Event::EqPresetSaved {
            name: name.to_string(),
        });
        Ok(())
    }

    /// Apply a named preset's bands to `channel_id` in the active profile. Live-applies via AudioBackend.
    pub fn apply_eq_preset(&mut self, preset: &str, channel_id: &str) -> Result<(), EngineError> {
        // Find preset — user library first, then factory catalog (user always wins on name conflict)
        let preset_bands = self
            .config
            .eq_presets
            .iter()
            .find(|p| p.name == preset)
            .map(|p| p.bands.clone())
            .or_else(|| {
                crate::presets::factory_eq_presets()
                    .into_iter()
                    .find(|p| p.name == preset)
                    .map(|p| p.bands)
            })
            .ok_or_else(|| EngineError::BadRequest(format!("EQ preset not found: {preset}")))?;

        // Mutate channel EQ in active profile
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            let channel = profile
                .channels
                .iter_mut()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found: {channel_id}"))
                })?;
            channel.eq = preset_bands;
        }

        self.save_config()?;

        // Live-apply all bands (same pattern as reconcile step 2)
        {
            let profile = self.config.active()?.clone();
            let channel = profile
                .channels
                .iter()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found: {channel_id}"))
                })?;
            let eq_model = convert::eq_model_for(channel)?;
            let def = convert::channel_def_from_cfg(channel);
            let spec = def.sink_spec();
            let mut be = arctis_audio::AudioBackend::new(&mut self.runner, spec);
            if let Err(e) = be.apply_all(&eq_model) {
                eprintln!(
                    "warning: apply_eq_preset apply_all for channel '{channel_id}' failed (ignoring): {e}"
                );
            }
        }

        self.emit(Event::EqPresetApplied {
            name: preset.to_string(),
            channel_id: channel_id.to_string(),
        });
        Ok(())
    }

    /// Set the FULL EQ band set for `channel_id` in the active profile in ONE shot.
    ///
    /// Unlike `set_eq_band` (one band per IPC round-trip, each resolving the sink
    /// node via a full `pw-cli ls Node` enumeration), this stores all bands, saves
    /// the config ONCE, and live-applies every band through `AudioBackend::apply_all`
    /// — a single `find_node_id` followed by N `pw-cli s` sets. Used by bulk UI edits
    /// (Flatten / tone curves) so the curve settles instantly instead of band-by-band.
    /// Mirrors `apply_eq_preset` minus the preset lookup (G3: live, no restart).
    pub fn set_channel_eq(
        &mut self,
        channel_id: &str,
        bands: Vec<EqBandConfig>,
    ) -> Result<crate::state::EngineState, EngineError> {
        if bands.len() > arctis_audio::MAX_BANDS {
            return Err(EngineError::BadRequest(format!(
                "too many EQ bands: {} (max {})",
                bands.len(),
                arctis_audio::MAX_BANDS
            )));
        }

        // Mutate channel EQ in active profile (store densely, same as apply_eq_preset).
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            let channel = profile
                .channels
                .iter_mut()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found: {channel_id}"))
                })?;
            channel.eq = bands;
        }

        self.save_config()?;

        // Live-apply all bands in one shot: one find_node_id + N `pw-cli s`.
        {
            let profile = self.config.active()?.clone();
            let channel = profile
                .channels
                .iter()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found: {channel_id}"))
                })?;
            let eq_model = convert::eq_model_for(channel)?;
            let def = convert::channel_def_from_cfg(channel);
            let spec = def.sink_spec();
            let mut be = arctis_audio::AudioBackend::new(&mut self.runner, spec);
            if let Err(e) = be.apply_all(&eq_model) {
                eprintln!(
                    "warning: set_channel_eq apply_all for channel '{channel_id}' failed (ignoring): {e}"
                );
            }
        }

        self.emit(Event::EqBandSet {
            channel_id: channel_id.to_string(),
            band: 0,
        });
        Ok(self.state())
    }

    /// Delete a named EQ preset. Errors if not found.
    pub fn delete_eq_preset(&mut self, name: &str) -> Result<(), EngineError> {
        let pos = self
            .config
            .eq_presets
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| EngineError::BadRequest(format!("EQ preset not found: {name}")))?;
        self.config.eq_presets.remove(pos);
        self.save_config()?;
        self.emit(Event::EqPresetDeleted {
            name: name.to_string(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
