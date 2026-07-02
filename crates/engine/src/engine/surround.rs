//! Virtual surround / HRIR: mode resolution, convolver sizing, live apply.
use super::*;

/// Map a configured surround mode + negotiated channel count to the effective render mode.
/// `Auto` adapts to the available channel count; explicit modes pass through unchanged.
/// Never produces a mode that upmixes (no "upmix" mode exists by construction).
pub fn resolve_effective_mode(cfg_mode: SurroundMode, negotiated: Option<u8>) -> SurroundMode {
    use SurroundMode::*;
    match cfg_mode {
        Auto => match negotiated {
            Some(8) => Hrir71,
            Some(6) => Hrir51,
            Some(2) => StereoBypass,
            Some(_) => Hrir71, // any other count (incl. unusual) → treat as full HRIR
            None => Hrir71,    // not yet probed → optimistic
        },
        explicit => explicit, // explicit non-Auto choice passes through unchanged
    }
}

/// Convert a `SurroundMode` to its canonical snake_case wire-format string.
pub(crate) fn surround_mode_str(m: SurroundMode) -> &'static str {
    use SurroundMode::*;
    match m {
        Auto => "auto",
        Hrir71 => "hrir71",
        Hrir51 => "hrir51",
        StereoBypass => "stereo_bypass",
    }
}

/// Build `Vec<HrirEntrySnapshot>` from the HRIR profiles directory under `base_dir`.
///
/// For each `.wav` stem found by `convert::available_hrirs`, looks up the catalog entry
/// (if any) to populate `display`, `group`, and `tonality`. Unknown stems get a
/// humanised display name (via `hrir_catalog::display_name`), an empty group, and
/// `"Neutral"` tonality.
pub(crate) fn hrir_entries_for(base_dir: &std::path::Path) -> Vec<crate::state::HrirEntrySnapshot> {
    convert::available_hrirs(base_dir)
        .into_iter()
        .map(|stem| {
            let tonality = crate::hrir_catalog::entry_for(&stem)
                .map(|e| format!("{:?}", e.tonality))
                .unwrap_or_else(|| "Neutral".into());
            let group = crate::hrir_catalog::entry_for(&stem)
                .map(|e| e.group.to_string())
                .unwrap_or_default();
            crate::state::HrirEntrySnapshot {
                display: crate::hrir_catalog::display_name(&stem),
                group,
                tonality,
                stem,
            }
        })
        .collect()
}

impl<R: CommandRunner> Engine<R> {
    /// Apply the surround config to the live graph (self-correcting, non-thrashing).
    ///
    /// Tracks which channels are currently routed to the surround node via
    /// `self.surround_routed`. On each call:
    ///
    /// **Disabled path**: removes the surround sink (idempotent), then restores ALL
    ///   channels in `surround_routed` to their configured `output_device` (fixes C2:
    ///   channels with `output_device=None` were left with a stale pointer to the
    ///   destroyed node). Drains `surround_routed` on completion.
    ///
    /// **Enabled path**: recreates the surround sink (so HRIR/hw_sink changes take
    ///   effect), then computes:
    ///   - `to_restore`: channels previously routed to surround but no longer in the
    ///     desired set → restore to their configured `output_device` (fixes C1).
    ///   - `to_route`: channels in the desired set not yet tracked → route to surround.
    ///
    ///   Channels already in `surround_routed` ∩ `desired` are left untouched (no thrash).
    ///
    /// Warn-and-continue on transient errors (mirrors reconcile step pattern).
    pub(super) fn apply_surround(
        &mut self,
        profile: &arctis_config::Profile,
    ) -> Result<(), crate::error::EngineError> {
        let sc = &profile.surround;
        let mut spec = convert::surround_spec(sc);

        if !sc.enabled {
            self.surround_hrir_missing = None;
            // Remove surround sink (idempotent).
            let mut surround_be = SurroundBackend::new(&mut self.runner, spec);
            if let Err(e) = surround_be.remove() {
                eprintln!("warning: apply_surround remove failed (ignoring): {e}");
            }
            // Restore ALL channels that were surround-routed (fixes C2).
            // Drain first so the set is empty regardless of errors below.
            let previously_routed: Vec<String> = self.surround_routed.drain().collect();
            if !previously_routed.is_empty() {
                let channel_set = convert::channel_set_from_profile(profile);
                for ch_id in &previously_routed {
                    if let Some(ch) = profile.channels.iter().find(|c| &c.id == ch_id) {
                        let eq_model = convert::eq_model_for(ch)?;
                        // M1: create ChannelManager per iteration; drop before track to release borrow.
                        let handle = {
                            let mut mgr =
                                ChannelManager::new(&mut self.runner, channel_set.clone());
                            mgr.set_output(ch_id, ch.output_device.clone(), &eq_model)
                        };
                        match handle {
                            Ok(h) => {
                                if let Some(t) = h.child {
                                    self.children.track(t);
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "warning: apply_surround restore channel '{ch_id}' failed (ignoring): {e}"
                                );
                            }
                        }
                    }
                }
            }
            return Ok(());
        }

        // Each surround-routed channel keeps its own EQ on its channel sink, applied
        // BEFORE convolution. There is no config-driven post-convolution tail EQ.
        //
        // Auto adapts to what the feeding app actually negotiated: probe the richest
        // input among streams on the surround-routed channel sinks (read-only, reuses
        // the TTL-cached pw-dump). Stereo-only input → StereoBypass; true surround →
        // HRIR. Explicit modes skip the probe entirely. Probe failure → None →
        // optimistic HRIR 7.1, the pre-probe behaviour.
        //
        // TODO(pw-watcher): the layout is only re-probed when apply_surround runs
        // (reconcile / surround setters). A live re-apply when the richest negotiated
        // layout CHANGES would need an engine hook from a stream watcher;
        // crates/cli/src/route_watcher.rs only rewrites route metadata and has no
        // engine handle, so wiring that is not contained — documented follow-up.
        let negotiated: Option<u8> = if sc.mode == SurroundMode::Auto {
            let surround_sinks: Vec<String> = profile
                .channels
                .iter()
                .filter(|c| sc.channels.iter().any(|id| id == &c.id))
                .map(|c| c.node_name.clone())
                .collect();
            let dump = self.volume_dump();
            parse_app_streams(&dump)
                .ok()
                .and_then(|streams| richest_surround_input(&streams, &surround_sinks))
                .map(|si| si.channels)
        } else {
            None
        };
        // Enabled path: resolve HRIR first (only needed for HRIR modes).
        // StereoBypass does not use an HRIR file — resolve only when needed.
        let effective = resolve_effective_mode(sc.mode, negotiated);
        let hrir_path_opt: Option<std::path::PathBuf> = match effective {
            SurroundMode::StereoBypass => {
                self.surround_hrir_missing = None;
                None
            }
            _ => match convert::hrir_base_dir()
                .and_then(|base| convert::resolve_hrir_path_or_fallback(sc, &base))
            {
                Ok((p, missing)) => {
                    self.surround_hrir_missing = missing;
                    Some(p)
                }
                Err(e) => {
                    eprintln!(
                        "warning: apply_surround HRIR resolve failed (skipping surround): {e}"
                    );
                    self.surround_hrir_missing = None;
                    return Ok(());
                }
            },
        };

        // Default the convolver's OUTPUT sink to the detected headset when hw_sink is
        // unset. Profiles store hw_sink=None meaning "auto → headset"; without this the
        // convolver's playback node has no target.object and PipeWire routes the
        // HRIR-processed audio to the SYSTEM DEFAULT sink (e.g. onboard speakers) instead
        // of the headphones. Mirrors reconcile's overlay_default_output for channels.
        if spec.hw_sink.is_none() {
            spec.hw_sink = self.detect_headset_sink();
        }

        // Insertion-gain normalization: measure the HRIR's direct-path peak and
        // emit the convolver `gain` option so every HRIR meets the same target
        // level (unmeasurable file → None → unity, the old behaviour).
        let conv_gain = hrir_path_opt
            .as_deref()
            .and_then(crate::hrir_import::normalization_gain);

        // The surround route target derives from the spec (G4), not a literal.
        let surround_target = spec.capture_node_name();

        // Recreate surround sink — pick the variant that matches the effective mode.
        {
            let mut surround_be = SurroundBackend::new(&mut self.runner, spec);
            let result = match effective {
                SurroundMode::Hrir71 | SurroundMode::Auto => {
                    // Auto is resolved above; this arm is unreachable in practice but
                    // kept for exhaustiveness.
                    let hrir_path = match hrir_path_opt.as_deref() {
                        Some(p) => p,
                        None => {
                            eprintln!("warning: apply_surround internal: HRIR path unexpectedly None (skipping)");
                            return Ok(());
                        }
                    };
                    surround_be.recreate_ex(hrir_path, 8, None, sc.blocksize, conv_gain, sc.tailsize)
                }
                SurroundMode::Hrir51 => {
                    let hrir_path = match hrir_path_opt.as_deref() {
                        Some(p) => p,
                        None => {
                            eprintln!("warning: apply_surround internal: HRIR path unexpectedly None (skipping)");
                            return Ok(());
                        }
                    };
                    surround_be.recreate_ex(hrir_path, 6, None, sc.blocksize, conv_gain, sc.tailsize)
                }
                SurroundMode::StereoBypass => {
                    surround_be.recreate_stereo_bypass(sc.crossfeed, None)
                }
            };
            match result {
                Ok(handle) => {
                    if let Some(t) = handle.child {
                        self.children.track(t);
                    }
                }
                Err(e) => {
                    eprintln!("warning: apply_surround recreate failed (ignoring): {e}");
                }
            }
        }

        // Compute desired surround-channel set (from new config).
        let desired: std::collections::HashSet<String> = sc.channels.iter().cloned().collect();

        // Channels to restore: were surround-routed, no longer in desired set (fixes C1).
        let to_restore: Vec<String> = self
            .surround_routed
            .iter()
            .filter(|id| !desired.contains(*id))
            .cloned()
            .collect();

        // Channels to route to surround: in desired set, not already tracked (avoid thrash).
        let to_route: Vec<String> = desired
            .iter()
            .filter(|id| !self.surround_routed.contains(*id))
            .cloned()
            .collect();

        let channel_set = convert::channel_set_from_profile(profile);

        // Restore removed channels to their configured output_device.
        for ch_id in &to_restore {
            if let Some(ch) = profile.channels.iter().find(|c| &c.id == ch_id) {
                let eq_model = convert::eq_model_for(ch)?;
                let handle = {
                    let mut mgr = ChannelManager::new(&mut self.runner, channel_set.clone());
                    mgr.set_output(ch_id, ch.output_device.clone(), &eq_model)
                };
                match handle {
                    Ok(h) => {
                        if let Some(t) = h.child {
                            self.children.track(t);
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "warning: apply_surround restore channel '{ch_id}' failed (ignoring): {e}"
                        );
                    }
                }
            }
            self.surround_routed.remove(ch_id);
        }

        // Route new channels to surround.
        for ch_id in &to_route {
            if let Some(ch) = profile.channels.iter().find(|c| &c.id == ch_id) {
                // Each channel keeps its own EQ on its channel sink (applied pre-convolution).
                let eq_model = convert::eq_model_for(ch)?;
                let handle = {
                    let mut mgr = ChannelManager::new(&mut self.runner, channel_set.clone());
                    mgr.set_output(ch_id, Some(surround_target.clone()), &eq_model)
                };
                match handle {
                    Ok(h) => {
                        if let Some(t) = h.child {
                            self.children.track(t);
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "warning: apply_surround reroute channel '{ch_id}' failed (ignoring): {e}"
                        );
                    }
                }
                self.surround_routed.insert(ch_id.clone());
            }
        }

        Ok(())
    }

    // ── Surround engine methods ──────────────────────────────────────────────────

    /// Enable or disable virtual surround. When enabling, resolves HRIR first (errors if none found).
    pub fn surround_set_enabled(&mut self, on: bool) -> Result<(), crate::error::EngineError> {
        // When enabling, validate HRIR exists first (before mutating config).
        if on {
            let profile = self.config.active()?.clone();
            let base = convert::hrir_base_dir()?;
            convert::resolve_hrir_path(&profile.surround, &base)?;
        }
        // Mutate config
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    name.clone(),
                ))
            })?;
            profile.surround.enabled = on;
        }
        self.save_config()?;
        // Apply via the canonical path (handles sink create/remove AND channel routing).
        // When enabling: apply_surround recreates sink + routes desired channels → surround_routed.
        // When disabling: apply_surround removes sink + restores all surround_routed channels.
        {
            let profile = self.config.active()?.clone();
            self.apply_surround(&profile)?;
        }
        self.emit(crate::state::Event::SurroundEnabledSet { enabled: on });
        Ok(())
    }

    /// Set the HRIR profile stem. Validates file exists, persists, recreates if enabled.
    pub fn surround_set_hrir(&mut self, stem: String) -> Result<(), crate::error::EngineError> {
        // Validate: update config temporarily to check path existence.
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    name.clone(),
                ))
            })?;
            let old = profile.surround.hrir.clone();
            profile.surround.hrir = Some(stem.clone());
            // Validate the resolved path exists.
            let base = convert::hrir_base_dir()?;
            if let Err(e) = convert::resolve_hrir_path(&profile.surround, &base) {
                // Roll back mutation.
                profile.surround.hrir = old;
                return Err(e);
            }
        }
        self.save_config()?;
        // Apply via the canonical path (recreates sink with new HRIR if enabled;
        // no-op remove if disabled; channel routing is a no-op since channels are
        // already tracked in surround_routed).
        {
            let profile = self.config.active()?.clone();
            self.apply_surround(&profile)?;
        }
        let hrir_snapshot = self.config.active()?.surround.hrir.clone();
        self.emit(crate::state::Event::SurroundHrirSet {
            hrir: hrir_snapshot,
        });
        Ok(())
    }

    /// Set the channels routed through surround. Persists, recreates routing if enabled.
    pub fn surround_set_channels(
        &mut self,
        channels: Vec<String>,
    ) -> Result<(), crate::error::EngineError> {
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    name.clone(),
                ))
            })?;
            profile.surround.channels = channels;
        }
        self.save_config()?;
        // Reapply routing if enabled.
        if self.config.active()?.surround.enabled {
            let profile = self.config.active()?.clone();
            self.apply_surround(&profile)?;
        }
        let channels_snapshot = self.config.active()?.surround.channels.clone();
        self.emit(crate::state::Event::SurroundChannelsSet {
            channels: channels_snapshot,
        });
        Ok(())
    }

    /// Validate a convolver partition size: a power of two in 64..=8192.
    /// `None` (PipeWire default) is always valid. Typed error otherwise (G7).
    fn validate_convolver_size(
        name: &str,
        v: Option<u32>,
    ) -> Result<(), crate::error::EngineError> {
        if let Some(n) = v {
            if !(64..=8192).contains(&n) || !n.is_power_of_two() {
                return Err(crate::error::EngineError::BadRequest(format!(
                    "{name} must be a power of two in 64..=8192, got {n} (use None for the PipeWire default)"
                )));
            }
        }
        Ok(())
    }

    /// Pin (or clear) the convolver blocksize. Persists; re-applies when enabled.
    /// Validates BEFORE persisting (G7): power of two in 64..=8192, and never
    /// larger than a pinned tailsize.
    pub fn surround_set_blocksize(
        &mut self,
        blocksize: Option<u32>,
    ) -> Result<(), crate::error::EngineError> {
        Self::validate_convolver_size("blocksize", blocksize)?;
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    name.clone(),
                ))
            })?;
            if let (Some(b), Some(t)) = (blocksize, profile.surround.tailsize) {
                if b > t {
                    return Err(crate::error::EngineError::BadRequest(format!(
                        "blocksize {b} exceeds the pinned tailsize {t}"
                    )));
                }
            }
            profile.surround.blocksize = blocksize;
        }
        self.save_config()?;
        if self.config.active()?.surround.enabled {
            let profile = self.config.active()?.clone();
            self.apply_surround(&profile)?;
        }
        Ok(())
    }

    /// Pin (or clear) the convolver tailsize. Persists; re-applies when enabled.
    /// Same validation as blocksize, plus tailsize ≥ blocksize (a tail partition
    /// smaller than the head partition is invalid). Rationale: a bare small
    /// blocksize partitions the entire ~250 ms bundled IR at that size — the
    /// tail should run in larger, cheaper partitions.
    pub fn surround_set_tailsize(
        &mut self,
        tailsize: Option<u32>,
    ) -> Result<(), crate::error::EngineError> {
        Self::validate_convolver_size("tailsize", tailsize)?;
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    name.clone(),
                ))
            })?;
            if let (Some(t), Some(b)) = (tailsize, profile.surround.blocksize) {
                if t < b {
                    return Err(crate::error::EngineError::BadRequest(format!(
                        "tailsize {t} must be ≥ the pinned blocksize {b}"
                    )));
                }
            }
            profile.surround.tailsize = tailsize;
        }
        self.save_config()?;
        if self.config.active()?.surround.enabled {
            let profile = self.config.active()?.clone();
            self.apply_surround(&profile)?;
        }
        Ok(())
    }

    /// Import HeSuVi 14-channel WAVs from `dir` into the HRIR profiles directory.
    ///
    /// If `dir` is `None`, tries a priority-ordered list of well-known paths under `$HOME`:
    /// 1. `~/.local/share/pipewire/hrir_hesuvi/import`
    /// 2. `~/Dev/Personal/sound-manager/Arctis-Sound-Manager/hrir/HRIR_wav_files`
    /// 3. `~/src/Arctis-Sound-Manager/hrir/HRIR_wav_files`
    ///
    /// Returns an `ImportReport` describing which files were imported / skipped.
    pub fn surround_import_hrirs(
        &mut self,
        dir: Option<String>,
    ) -> Result<crate::hrir_import::ImportReport, crate::error::EngineError> {
        let src = match dir {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                let home = std::env::var("HOME").map(std::path::PathBuf::from).map_err(|_| {
                    crate::error::EngineError::BadRequest(
                        "no HRIR import directory found; pass an explicit path or create \
                         ~/.local/share/pipewire/hrir_hesuvi/import"
                            .into(),
                    )
                })?;
                let candidates = [
                    home.join(".local/share/pipewire/hrir_hesuvi/import"),
                    home.join("Dev/Personal/sound-manager/Arctis-Sound-Manager/hrir/HRIR_wav_files"),
                    home.join("src/Arctis-Sound-Manager/hrir/HRIR_wav_files"),
                ];
                candidates
                    .into_iter()
                    .find(|p| p.is_dir())
                    .ok_or_else(|| {
                        crate::error::EngineError::BadRequest(
                            "no HRIR import directory found; pass an explicit path or create \
                             ~/.local/share/pipewire/hrir_hesuvi/import"
                                .into(),
                        )
                    })?
            }
        };
        let base = crate::convert::hrir_base_dir()?;
        let report = crate::hrir_import::import_dir(&mut self.runner, &src, &base)?;
        eprintln!(
            "surround_import_hrirs: imported {}, skipped {}",
            report.imported.len(),
            report.skipped.len()
        );
        Ok(report)
    }

    /// Placeholder: automatic HeSuVi download is not yet implemented.
    ///
    /// This method is wired through the full stack so the surface exists. The actual
    /// download + import logic will be added in a future task.
    pub fn surround_fetch_hrirs(&mut self) -> Result<(), crate::error::EngineError> {
        Err(crate::error::EngineError::BadRequest(
            "automatic HeSuVi download is not yet available — use Import to add your local \
             HeSuVi collection"
                .into(),
        ))
    }

    /// Set (or clear) the hardware sink for the surround output tail. Recreates if enabled.
    pub fn surround_set_hw_sink(
        &mut self,
        hw_sink: Option<String>,
    ) -> Result<(), crate::error::EngineError> {
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    name.clone(),
                ))
            })?;
            profile.surround.hw_sink = hw_sink;
        }
        self.save_config()?;
        // Apply via the canonical path (recreates sink with new hw_sink baked into conf if
        // enabled; no-op remove if disabled; channel routing is a no-op since channels are
        // already tracked in surround_routed).
        {
            let profile = self.config.active()?.clone();
            self.apply_surround(&profile)?;
        }
        let hw_sink_snapshot = self.config.active()?.surround.hw_sink.clone();
        self.emit(crate::state::Event::SurroundHwSinkSet {
            hw_sink: hw_sink_snapshot,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
