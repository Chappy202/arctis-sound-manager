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
mod tests {
    use super::*;
    use crate::engine::test_support::*;

    // ─────────────────────────────────────────────
    // Surround mode fallback tests
    // ─────────────────────────────────────────────

    #[test]
    fn auto_mode_maps_negotiated_channels_to_path() {
        use SurroundMode::*;
        assert!(matches!(resolve_effective_mode(Auto, Some(8)), Hrir71));
        assert!(matches!(resolve_effective_mode(Auto, Some(6)), Hrir51));
        assert!(matches!(resolve_effective_mode(Auto, Some(2)), StereoBypass));
        assert!(matches!(resolve_effective_mode(StereoBypass, Some(8)), StereoBypass));
        assert!(matches!(resolve_effective_mode(Auto, None), Hrir71));
    }

    // ─────────────────────────────────────────────
    // F1.3 TDD: resolve_hrir_path + available_hrirs
    // ─────────────────────────────────────────────

    use crate::convert;

    // These tests inject the base dir directly — NO env mutation, NO ENV_MUTEX needed.
    // This is the canonical pattern: resolve_hrir_path / available_hrirs take base_dir
    // as a parameter so tests never race on process-global HOME.

    #[test]
    fn resolve_hrir_path_with_named_stem_returns_abs_path() {
        let tmp = unique_cfg_tmp("hrir_named");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        let wav = profiles_dir.join("00-default.wav");
        std::fs::write(&wav, b"").unwrap();

        let cfg = arctis_config::SurroundConfig {
            hrir: Some("00-default".into()),
            ..Default::default()
        };
        let path = convert::resolve_hrir_path(&cfg, &base).expect("should resolve named stem");
        assert_eq!(path, wav);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_hrir_path_named_stem_missing_returns_err() {
        let tmp = unique_cfg_tmp("hrir_missing");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();

        let cfg = arctis_config::SurroundConfig {
            hrir: Some("nonexistent".into()),
            ..Default::default()
        };
        let result = convert::resolve_hrir_path(&cfg, &base);
        assert!(matches!(result, Err(EngineError::BadRequest(_))));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_hrir_path_no_stem_picks_first_lexicographic() {
        let tmp = unique_cfg_tmp("hrir_lex");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("beta.wav"), b"").unwrap();
        std::fs::write(profiles_dir.join("alpha.wav"), b"").unwrap();

        let cfg = arctis_config::SurroundConfig::default(); // hrir = None
        let path =
            convert::resolve_hrir_path(&cfg, &base).expect("should pick first lexicographic");
        assert!(
            path.ends_with("alpha.wav"),
            "expected alpha.wav, got {:?}",
            path
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_hrir_path_no_stem_no_profiles_fallback_to_hrir_wav() {
        let tmp = unique_cfg_tmp("hrir_fallback");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("hrir.wav"), b"").unwrap();

        let cfg = arctis_config::SurroundConfig::default();
        let path = convert::resolve_hrir_path(&cfg, &base).expect("should fall back to hrir.wav");
        assert!(
            path.ends_with("hrir.wav"),
            "expected hrir.wav, got {:?}",
            path
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_hrir_path_no_hrir_at_all_returns_err() {
        let tmp = unique_cfg_tmp("hrir_none");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        std::fs::create_dir_all(&base).unwrap();

        let cfg = arctis_config::SurroundConfig::default();
        let result = convert::resolve_hrir_path(&cfg, &base);
        assert!(matches!(result, Err(EngineError::BadRequest(_))));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_or_fallback_present_stem_no_missing_flag() {
        let tmp = unique_cfg_tmp("hrir_or_fb_present");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("04-gsx-sennheiser-gsx.wav"), b"").unwrap();
        let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
        let (path, missing) = convert::resolve_hrir_path_or_fallback(&cfg, &base).expect("resolves");
        assert!(path.ends_with("04-gsx-sennheiser-gsx.wav"));
        assert_eq!(missing, None, "no fallback used → no missing flag");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_or_fallback_missing_pinned_uses_bundled_and_reports_missing() {
        let tmp = unique_cfg_tmp("hrir_or_fb_missing");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        // Pinned stem absent; bundled dry fallback present.
        std::fs::write(profiles_dir.join(format!("{}.wav", convert::FALLBACK_HRIR_STEM)), b"").unwrap();
        let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
        let (path, missing) = convert::resolve_hrir_path_or_fallback(&cfg, &base).expect("falls back");
        assert!(path.ends_with(format!("{}.wav", convert::FALLBACK_HRIR_STEM)));
        assert_eq!(missing, Some("04-gsx-sennheiser-gsx".to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_or_fallback_missing_pinned_falls_back_to_any_available() {
        let tmp = unique_cfg_tmp("hrir_or_fb_any");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        // Neither pinned nor bundled present, but another HRIR exists.
        std::fs::write(profiles_dir.join("99-other.wav"), b"").unwrap();
        let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
        let (path, missing) = convert::resolve_hrir_path_or_fallback(&cfg, &base).expect("falls back to any");
        assert!(path.ends_with("99-other.wav"));
        assert_eq!(missing, Some("04-gsx-sennheiser-gsx".to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_or_fallback_no_hrir_at_all_errors() {
        let tmp = unique_cfg_tmp("hrir_or_fb_none");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        std::fs::create_dir_all(&base).unwrap();
        let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
        let result = convert::resolve_hrir_path_or_fallback(&cfg, &base);
        assert!(matches!(result, Err(EngineError::BadRequest(_))));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn available_hrirs_returns_sorted_stems() {
        let tmp = unique_cfg_tmp("avail_hrirs");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("zz-last.wav"), b"").unwrap();
        std::fs::write(profiles_dir.join("aa-first.wav"), b"").unwrap();
        std::fs::write(profiles_dir.join("mm-middle.wav"), b"").unwrap();

        let hrirs = convert::available_hrirs(&base);
        assert_eq!(hrirs, vec!["aa-first", "mm-middle", "zz-last"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn available_hrirs_empty_when_dir_missing() {
        let tmp = unique_cfg_tmp("avail_hrirs_empty");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        // Intentionally don't create the profiles dir.
        std::fs::create_dir_all(&tmp).unwrap();

        let hrirs = convert::available_hrirs(&base);
        assert!(hrirs.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn reconcile_step6_disabled_calls_source_exists_noop() {
        // Surround disabled → remove() is called → source_exists() fires → 1 ls
        // queue_reconcile_present already adds the surround step6 ls output
        let runner = queue_reconcile_present(MockRunner::new());
        let cfg = make_config_no_eq_no_routes(); // surround disabled by default
        let mut engine = Engine::new(runner, cfg);
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        // The last call should be the surround source_exists check (ls Node, step6)
        let calls = &engine.runner.calls;
        let last = calls.last().expect("at least one call");
        assert_eq!(
            last,
            &vec!["pw-cli", "ls", "Node"],
            "step6 must call source_exists"
        );
        // No surround spawn (disabled → remove path, source absent → no-op)
        assert!(
            !engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("arctis_surround"))
                .unwrap_or(false)),
            "no surround spawn when disabled"
        );
        // Total 60 calls: 2 (detect) + 4 (phase1) + 4*(1+10+1+1) (EQ+vol per channel) + 1 (mic) + 1 (surround)
        assert_eq!(calls.len(), 60, "expected 60 total pw-cli calls");
    }

    /// Remove stale on-disk confs from previous tests/runs so the
    /// diff-before-recreate guards never skip a scripted teardown+respawn.
    fn scrub_stale_confs() {
        let t = std::env::temp_dir();
        for f in [
            "arctis_arctis_surround.conf",
            "arctis_eq.Arctis_Game.conf",
            "arctis_eq.Arctis_Chat.conf",
            "arctis_eq.Arctis_Media.conf",
            "arctis_eq.Arctis_Aux.conf",
        ] {
            let _ = std::fs::remove_file(t.join(f));
        }
    }

    /// Queue outputs for reconcile with surround ENABLED and surround node absent.
    /// Engine::new seeds "aux" → 4 channels (game/chat/media/aux).
    /// Step6 enabled (apply_surround uses recreate_ex in enabled path):
    ///   1. recreate_ex() = remove() + spawn_conf() (no second source_exists):
    ///      remove: source_exists() → 1 ls (absent, no destroy)
    ///      spawn_conf: writes conf + spawns (no runner call)
    ///   2. reroute "game": set_output → ls (source_exists: present) → find_node_id → destroy → pkill + ls (source absent → spawn)
    ///   3. reroute "media": same
    fn queue_reconcile_surround_enabled_absent(runner: MockRunner) -> MockRunner {
        scrub_stale_confs();
        let ls_channels = ls_all_present();
        let ls_surround_absent = ls_all_absent(); // no surround node
        let mut r = runner;
        // detect_headset_sink: pw-metadata 0 + pw-dump (no SteelSeries → detect returns None)
        r = r.with_output(0, "", ""); // pw-metadata 0
        r = r.with_output(0, "[]", ""); // pw-dump []
        // Phase 1: 4 ls (all channels present, including aux seeded by Engine::new)
        for _ in 0..4 {
            r = r.with_output(0, &ls_channels, "");
        }
        // Phase 2 + 2b interleaved: per channel (4), EQ apply then volume/mute apply
        for _ in 0..4 {
            // Phase 2: EQ apply (1 ls + 10 band sets + 1 preamp set)
            r = r.with_output(0, &ls_channels, "");
            for _ in 0..11 {
                r = r.with_output(0, "", "");
            }
            // Phase 2b: volume/mute apply (1 ls + 1 Props set)
            r = r.with_output(0, &ls_channels, ""); // find_node_id
            r = r.with_output(0, "", ""); // Props set
        }
        // Phase 5 (mic disabled): source_exists → 1 ls (no mic)
        r = r.with_output(0, &ls_surround_absent, "");
        // Phase 6 start: apply_surround (mode = Auto) probes the negotiated input
        // layout first → 1 pw-dump (no streams → None → HRIR 7.1).
        r = r.with_output(0, "[]", ""); // apply_surround Auto probe: pw-dump
        // Then it re-detects the headset to default the convolver output sink
        // (hw_sink unset) → pw-metadata 0 + pw-dump (no SteelSeries → None,
        // so the convolver output is left untargeted in this fixture).
        r = r.with_output(0, "", ""); // apply_surround detect: pw-metadata 0
        r = r.with_output(0, "[]", ""); // apply_surround detect: pw-dump []
        // Phase 6 (surround enabled, absent) — recreate_ex():
        //   remove: source_exists() → 1 ls (absent, no destroy needed)
        //   spawn_conf: writes conf + spawns directly (no second runner call)
        r = r.with_output(0, &ls_surround_absent, "");
        // reroute "game": set_output → ChannelManager::set_output →
        //   find existing present node: ls (source_exists present) + find_node_id ls + destroy + pkill
        //   create absent: ls absent → spawn
        r = r.with_output(0, &ls_channels, ""); // set_output: source_exists (game present)
        r = r.with_output(0, &ls_channels, ""); // set_output: find_node_id
        r = r.with_output(0, "", ""); // set_output: destroy
        r = r.with_output(0, "", ""); // set_output: pkill
        r = r.with_output(0, &ls_surround_absent, ""); // set_output: create source_exists (absent → spawn)
                                                       // reroute "media": same
        r = r.with_output(0, &ls_channels, "");
        r = r.with_output(0, &ls_channels, "");
        r = r.with_output(0, "", "");
        r = r.with_output(0, "", "");
        r = r.with_output(0, &ls_surround_absent, "");
        r
    }

    #[test]
    fn reconcile_step6_enabled_spawns_surround_and_reroutes_channels() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_step6_enabled");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        let runner = queue_reconcile_surround_enabled_absent(MockRunner::new());
        let cfg = make_config_surround_enabled("test-hrir");
        let mut engine = Engine::new(runner, cfg);
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        // Surround sink was spawned
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("arctis_surround"))
                .unwrap_or(false)),
            "surround sink must be spawned: {:?}",
            engine.runner.spawned
        );
        // Channel reroutes spawned new instances for game + media
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("Arctis_Game"))
                .unwrap_or(false)),
            "game channel must be respawned for surround routing"
        );
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("Arctis_Media"))
                .unwrap_or(false)),
            "media channel must be respawned for surround routing"
        );
        // Children tracked: surround + game + media
        assert!(engine.children.len() >= 3, "at least 3 children tracked");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    /// pw-dump fixture: a STEREO stream linked to Arctis_Game (the surround sink).
    const STEREO_STREAM_DUMP: &str = r#"[
      { "id": 50, "type": "PipeWire:Interface:Node",
        "info": { "props": { "media.class": "Audio/Sink", "node.name": "Arctis_Game" } } },
      { "id": 51, "type": "PipeWire:Interface:Node",
        "info": { "props": { "media.class": "Stream/Output/Audio",
            "application.name": "Spotify", "application.process.binary": "spotify" },
          "params": { "Format": [ { "channels": 2, "position": ["FL","FR"] } ] } } },
      { "id": 99, "type": "PipeWire:Interface:Link",
        "info": { "output-node-id": 51, "input-node-id": 50 } }
    ]"#;

    /// Auto + a stereo-only feeding stream → state() must report the REAL
    /// effective mode (stereo_bypass), not the old hardcoded hrir71.
    #[test]
    fn state_effective_mode_auto_reports_bypass_for_stereo_input() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let runner = MockRunner::new().with_output(0, STEREO_STREAM_DUMP, ""); // state(): pw-dump
        let cfg = make_config_surround_single_game_no_eq("test-hrir"); // mode = Auto
        let mut engine = Engine::new(runner, cfg);
        let st = engine.state();
        assert_eq!(st.surround.mode, "auto");
        assert_eq!(st.surround.negotiated_channels, Some(2));
        assert_eq!(
            st.surround.effective_mode, "stereo_bypass",
            "Auto with a stereo input must report stereo_bypass"
        );
    }

    /// Auto + a stereo-only feeding stream → apply_surround must build the
    /// stereo-bypass graph (2-ch, no convolver), not the 7.1 HRIR graph.
    #[test]
    fn apply_surround_auto_with_stereo_input_builds_bypass() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let runner = queue_apply_surround_both_absent(
            MockRunner::new()
                .with_output(0, STEREO_STREAM_DUMP, "") // Auto probe: pw-dump (stereo stream)
                .with_output(0, "", "") // detect: pw-metadata 0
                .with_output(0, "[]", ""), // detect: pw-dump []
        );
        let cfg = make_config_surround_single_game_no_eq("test-hrir"); // mode = Auto
        let mut engine = Engine::new(runner, cfg);
        let profile = engine.config.active().unwrap().clone();
        engine.apply_surround(&profile).expect("apply_surround must succeed");

        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| argv.get(2).map(|s| s.contains("arctis_surround")).unwrap_or(false))
            .expect("surround conf must have been spawned");
        let conf = std::fs::read_to_string(&surround_argv[2]).expect("conf exists");
        assert!(
            conf.contains("audio.channels = 2"),
            "Auto+stereo must spawn the 2-ch bypass graph, got:\n{conf}"
        );
        assert!(
            !conf.contains("convolver"),
            "Auto+stereo must NOT build the HRIR convolver graph"
        );
    }

    // ─────────────────────────────────────────────
    // F1.3 TDD: surround_set_* methods
    // ─────────────────────────────────────────────

    #[test]
    fn surround_set_enabled_true_without_hrir_returns_err() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_on_no_hrir");
        // Don't create any HRIR file → resolve_hrir_path must fail
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("HOME", &tmp);

        let cfg = make_config_no_eq_no_routes(); // surround.enabled = false
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let result = engine.surround_set_enabled(true);
        assert!(
            matches!(result, Err(EngineError::BadRequest(_))),
            "must error when no HRIR exists: {result:?}"
        );
        // Config must NOT have been mutated (error before mutation)
        assert!(
            !engine.config().active().unwrap().surround.enabled,
            "surround.enabled must remain false on error"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    #[test]
    fn surround_set_hrir_persists_and_emits_event() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp_home = unique_cfg_tmp("surr_set_hrir_home");
        let tmp_cfg = unique_cfg_tmp("surr_set_hrir_cfg");
        let profiles_dir = tmp_home.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("my-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp_home);
        std::env::set_var("ASM_CONFIG_HOME", &tmp_cfg);

        // surround disabled → surround_set_hrir just persists, no recreate
        let cfg = make_config_no_eq_no_routes(); // surround disabled
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        engine.set_event_sink(tx);

        engine
            .surround_set_hrir("my-hrir".into())
            .expect("surround_set_hrir should succeed");

        // Config persisted
        let saved_path = tmp_cfg.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("my-hrir"),
            "persisted config must contain hrir stem"
        );

        // In-memory updated
        assert_eq!(
            engine.config().active().unwrap().surround.hrir.as_deref(),
            Some("my-hrir")
        );

        // Event emitted
        let event = rx.try_recv().expect("SurroundHrirSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::SurroundHrirSet {
                hrir: Some("my-hrir".into())
            }
        );

        let _ = std::fs::remove_dir_all(&tmp_home);
        let _ = std::fs::remove_dir_all(&tmp_cfg);
        std::env::remove_var("HOME");
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn surround_set_hrir_nonexistent_returns_err() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_hrir_err");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        // Don't write the file.
        std::env::set_var("HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let result = engine.surround_set_hrir("nonexistent".into());
        assert!(matches!(result, Err(EngineError::BadRequest(_))));
        // hrir must remain None (rollback)
        assert!(engine.config().active().unwrap().surround.hrir.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    #[test]
    fn surround_set_channels_persists_and_emits_event() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_ch");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        engine.set_event_sink(tx);

        engine
            .surround_set_channels(vec!["game".into(), "chat".into(), "media".into()])
            .expect("surround_set_channels should succeed");

        // Config persisted
        assert!(tmp.join("config.toml").exists());

        // Event emitted
        let event = rx
            .try_recv()
            .expect("SurroundChannelsSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::SurroundChannelsSet {
                channels: vec!["game".into(), "chat".into(), "media".into()]
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn surround_set_blocksize_rejects_zero() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_bs_zero");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let before = engine.config.active().unwrap().surround.blocksize;

        let res = engine.surround_set_blocksize(Some(0));
        assert!(matches!(res, Err(crate::error::EngineError::BadRequest(_))));
        assert_eq!(
            engine.config.active().unwrap().surround.blocksize,
            before,
            "config must be unchanged when blocksize is rejected"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn surround_set_blocksize_persists() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_bs");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        engine.surround_set_blocksize(Some(128)).unwrap();
        assert_eq!(engine.config.active().unwrap().surround.blocksize, Some(128));
        engine.surround_set_blocksize(None).unwrap();
        assert_eq!(engine.config.active().unwrap().surround.blocksize, None);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn surround_set_blocksize_rejects_non_power_of_two_and_out_of_range() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_bs_invalid");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        for bad in [100u32, 32, 16384, 65] {
            let res = engine.surround_set_blocksize(Some(bad));
            assert!(
                matches!(res, Err(crate::error::EngineError::BadRequest(_))),
                "blocksize {bad} must be rejected (power of two in 64..=8192)"
            );
        }
        // Valid values pass (64 and 8192 are the range edges).
        engine.surround_set_blocksize(Some(64)).unwrap();
        engine.surround_set_blocksize(Some(8192)).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn surround_set_tailsize_validates_and_persists() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_ts");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        // Same shape validation as blocksize.
        assert!(matches!(
            engine.surround_set_tailsize(Some(100)),
            Err(crate::error::EngineError::BadRequest(_))
        ));
        // tailsize must be >= a pinned blocksize.
        engine.surround_set_blocksize(Some(256)).unwrap();
        assert!(
            matches!(
                engine.surround_set_tailsize(Some(128)),
                Err(crate::error::EngineError::BadRequest(_))
            ),
            "tailsize below the pinned blocksize must be rejected"
        );
        engine.surround_set_tailsize(Some(4096)).unwrap();
        assert_eq!(engine.config.active().unwrap().surround.tailsize, Some(4096));
        // And blocksize may not be raised past the pinned tailsize.
        assert!(matches!(
            engine.surround_set_blocksize(Some(8192)),
            Err(crate::error::EngineError::BadRequest(_))
        ));
        // Clearing works.
        engine.surround_set_tailsize(None).unwrap();
        assert_eq!(engine.config.active().unwrap().surround.tailsize, None);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn state_surround_snapshot_reflects_config() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_state");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("aa-first.wav"), b"").unwrap();
        std::fs::write(profiles_dir.join("zz-last.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround = arctis_config::SurroundConfig {
            enabled: false,
            hrir: Some("aa-first".into()),
            channels: vec!["game".into()],
            hw_sink: Some("alsa_output.pci".into()),
            ..Default::default()
        };
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let s = engine.state();

        assert!(!s.surround.enabled);
        assert_eq!(s.surround.hrir.as_deref(), Some("aa-first"));
        assert_eq!(s.surround.channels, vec!["game"]);
        assert_eq!(s.surround.hw_sink.as_deref(), Some("alsa_output.pci"));
        assert_eq!(s.surround.available_hrirs, vec!["aa-first", "zz-last"]);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    #[test]
    fn surround_snapshot_includes_display_entries_for_available_hrirs() {
        let base = unique_cfg_tmp("hrir_entries");
        let profiles = base.join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(profiles.join("04-gsx-sennheiser-gsx.wav"), b"RIFF").unwrap();
        let entries = crate::engine::hrir_entries_for(&base);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].stem, "04-gsx-sennheiser-gsx");
        assert_eq!(entries[0].display, "Sennheiser GSX");
        assert_eq!(entries[0].group, "Sennheiser");
        let _ = std::fs::remove_dir_all(&base);
    }

    // ─────────────────────────────────────────────
    // F1.3 Bug Fix tests: C1, C2, I3
    // ─────────────────────────────────────────────

    /// C1 fix: apply_surround restores a channel removed from the surround list.
    ///
    /// Setup: `surround_routed = {"game", "media"}`, config has `channels = ["game"]`.
    /// After apply_surround: media must be restored to its output_device; surround_routed = {"game"}.
    #[test]
    fn apply_surround_restores_removed_channel() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp_home = unique_cfg_tmp("c1_restore_home");
        let profiles_dir = tmp_home.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp_home);

        // Config: surround enabled with only "game" in channels.
        // "media" was previously routed to surround but is now removed.
        let cfg = make_config_surround_enabled("test-hrir"); // channels = ["game", "media"]
                                                             // We'll override channels to just ["game"] below.

        // Runner calls for apply_surround with surround_routed = {"game", "media"},
        // desired = {"game"}, to_restore = ["media"], to_route = [] (game already tracked):
        //
        // recreate surround (absent) — recreate_ex():
        //   remove: source_exists → 1 ls (absent, no destroy)
        //   spawn_conf: writes conf + spawns directly (no second runner call)
        //
        // restore media (Arctis_Media present → destroy + pkill, then create absent → spawn):
        //   remove: sink_exists → 1 ls (present), find_node_id → 1 ls, destroy, pkill
        //   create: sink_exists → 1 ls (absent) → spawn
        let ls_channels = ls_all_present();
        let ls_absent = ls_all_absent();

        let runner = MockRunner::new()
            // recreate surround: remove source_exists (absent)
            .with_output(0, &ls_absent, "")
            // restore media: remove sink_exists (present)
            .with_output(0, &ls_channels, "")
            // restore media: remove find_node_id
            .with_output(0, &ls_channels, "")
            // restore media: destroy
            .with_output(0, "", "")
            // restore media: pkill
            .with_output(0, "", "")
            // restore media: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "");

        let mut engine = Engine::new(runner, cfg);
        // Pre-populate surround_routed to simulate prior state.
        engine.surround_routed.insert("game".into());
        engine.surround_routed.insert("media".into());

        // Mutate config so surround only has "game".
        {
            let p = engine.config.profile_mut("default").unwrap();
            p.surround.channels = vec!["game".into()];
        }

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        // surround_routed must now only contain "game" (media was removed).
        assert!(
            engine.surround_routed.contains("game"),
            "game must remain in surround_routed"
        );
        assert!(
            !engine.surround_routed.contains("media"),
            "media must be removed from surround_routed after restore"
        );
        assert_eq!(
            engine.surround_routed.len(),
            1,
            "exactly 1 channel in surround_routed"
        );

        // media was respawned (set_output → recreate → create → spawn)
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("Arctis_Media"))
                .unwrap_or(false)),
            "Arctis_Media must be respawned to restore from surround routing: {:?}",
            engine.runner.spawned
        );

        // surround sink was recreated (spawned)
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("arctis_surround"))
                .unwrap_or(false)),
            "surround sink must be respawned: {:?}",
            engine.runner.spawned
        );

        let _ = std::fs::remove_dir_all(&tmp_home);
        std::env::remove_var("HOME");
    }

    /// C2 fix: apply_surround disabled path restores channels with output_device=None.
    ///
    /// Setup: `surround_routed = {"game"}` (simulating prior enable), config has
    /// `surround.enabled = false`. After apply_surround: game must be restored
    /// (even though output_device = None), surround_routed = {}.
    #[test]
    fn apply_surround_disabled_restores_stale_channel_with_no_output_device() {
        scrub_stale_confs();
        // Config: surround disabled (default). Game channel has output_device = None.
        let cfg = make_config_no_eq_no_routes(); // surround.enabled = false

        // Runner calls for apply_surround with surround_routed = {"game"},
        // surround disabled:
        //
        // remove surround (absent → no destroy):
        //   source_exists → 1 ls (absent)
        //
        // restore game (Arctis_Game present, then recreate):
        //   remove: sink_exists → 1 ls (present), find_node_id → 1 ls, destroy, pkill
        //   create: sink_exists → 1 ls (absent) → spawn
        let ls_channels = ls_all_present();
        let ls_absent = ls_all_absent();

        let runner = MockRunner::new()
            // remove surround: source_exists (absent)
            .with_output(0, &ls_absent, "")
            // restore game: remove sink_exists (present)
            .with_output(0, &ls_channels, "")
            // restore game: remove find_node_id
            .with_output(0, &ls_channels, "")
            // restore game: destroy
            .with_output(0, "", "")
            // restore game: pkill
            .with_output(0, "", "")
            // restore game: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "");

        let mut engine = Engine::new(runner, cfg);
        // Simulate prior enable: game was routed to surround.
        engine.surround_routed.insert("game".into());

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        // surround_routed must be empty after disable.
        assert!(
            engine.surround_routed.is_empty(),
            "surround_routed must be empty after disable: {:?}",
            engine.surround_routed
        );

        // game channel was respawned (restored even with output_device = None).
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("Arctis_Game"))
                .unwrap_or(false)),
            "Arctis_Game must be respawned after disable even with output_device=None: {:?}",
            engine.runner.spawned
        );

        // Verify destroy was called (channel was present, then destroyed and recreated).
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c.len() >= 3 && c[1] == "destroy"),
            "destroy must be called when restoring the channel"
        );
    }

    /// Enable → disable flow: all listed channels are restored after disable.
    ///
    /// This tests the full round-trip:
    /// 1. surround_set_enabled(true) → populates surround_routed
    /// 2. surround_set_enabled(false) → drains surround_routed + restores channels
    #[test]
    fn surround_set_enabled_disable_restores_all_channels() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp_home = unique_cfg_tmp("surr_roundtrip_home");
        let tmp_cfg = unique_cfg_tmp("surr_roundtrip_cfg");
        let profiles_dir = tmp_home.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp_home);
        std::env::set_var("ASM_CONFIG_HOME", &tmp_cfg);

        // Config: surround disabled, channels = ["game", "media"] (will be enabled).
        // Start with surround disabled so surround_set_enabled(true) is meaningful.
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround = arctis_config::SurroundConfig {
            enabled: false,
            hrir: Some("test-hrir".into()),
            channels: vec!["game".into(), "media".into()],
            hw_sink: None,
            ..Default::default()
        };

        // Phase 1: surround_set_enabled(true) → apply_surround(enabled):
        //   recreate surround (recreate_ex): remove (absent, 1 ls) + spawn_conf (no ls) = 1 ls + spawn
        //   to_route = ["game", "media"] (both not yet tracked):
        //     route game: remove (absent) + create (absent → spawn) = 2 ls + spawn
        //     route media: remove (absent) + create (absent → spawn) = 2 ls + spawn
        //
        // Phase 2: surround_set_enabled(false) → apply_surround(disabled):
        //   remove surround: source_exists (absent) = 1 ls
        //   restore game (was present from spawn above — but MockRunner can't distinguish;
        //     we'll report game as absent for simplicity → remove is no-op, create spawns)
        //   restore media: same
        //
        // For simplicity, we report all channels/surround as absent throughout.
        let ls_absent = ls_all_absent();

        let runner = MockRunner::new()
            // --- Phase 1: surround_set_enabled(true) ---
            // recreate surround (recreate_ex): remove source_exists (absent) — spawn_conf has no ls
            .with_output(0, &ls_absent, "")
            // route game: remove sink_exists (absent)
            .with_output(0, &ls_absent, "")
            // route game: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "")
            // route media: remove sink_exists (absent)
            .with_output(0, &ls_absent, "")
            // route media: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "")
            // --- Phase 2: surround_set_enabled(false) ---
            // remove surround: source_exists (absent)
            .with_output(0, &ls_absent, "")
            // restore game: remove sink_exists (absent)
            .with_output(0, &ls_absent, "")
            // restore game: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "")
            // restore media: remove sink_exists (absent)
            .with_output(0, &ls_absent, "")
            // restore media: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "");

        let mut engine = Engine::new(runner, cfg);

        // Enable surround.
        engine
            .surround_set_enabled(true)
            .expect("enable must succeed");

        // After enable: surround_routed must contain both channels.
        assert!(
            engine.surround_routed.contains("game"),
            "game must be in surround_routed after enable"
        );
        assert!(
            engine.surround_routed.contains("media"),
            "media must be in surround_routed after enable"
        );

        // Disable surround.
        engine
            .surround_set_enabled(false)
            .expect("disable must succeed");

        // After disable: surround_routed must be empty.
        assert!(
            engine.surround_routed.is_empty(),
            "surround_routed must be empty after disable: {:?}",
            engine.surround_routed
        );

        // game and media channels must have been respawned (restored).
        let game_spawns = engine
            .runner
            .spawned
            .iter()
            .filter(|argv| {
                argv.get(2)
                    .map(|s| s.contains("Arctis_Game"))
                    .unwrap_or(false)
            })
            .count();
        let media_spawns = engine
            .runner
            .spawned
            .iter()
            .filter(|argv| {
                argv.get(2)
                    .map(|s| s.contains("Arctis_Media"))
                    .unwrap_or(false)
            })
            .count();

        // game: spawned on route (enable) + spawned on restore (disable) = 2
        assert!(
            game_spawns >= 2,
            "game must be spawned at least twice (route + restore): {game_spawns}"
        );
        // media: same
        assert!(
            media_spawns >= 2,
            "media must be spawned at least twice (route + restore): {media_spawns}"
        );

        let _ = std::fs::remove_dir_all(&tmp_home);
        let _ = std::fs::remove_dir_all(&tmp_cfg);
        std::env::remove_var("HOME");
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// I3 fix: surround_set_hrir routes through apply_surround (not inlined recreate).
    ///
    /// When surround is ENABLED and HRIR changes, apply_surround is called.
    /// Since channels are already in surround_routed, to_route and to_restore are both empty
    /// → only the sink is recreated (no channel thrash).
    #[test]
    fn surround_set_hrir_enabled_uses_apply_surround_no_channel_thrash() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp_home = unique_cfg_tmp("surr_hrir_i3_home");
        let tmp_cfg = unique_cfg_tmp("surr_hrir_i3_cfg");
        let profiles_dir = tmp_home.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("old-hrir.wav"), b"").unwrap();
        std::fs::write(profiles_dir.join("new-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp_home);
        std::env::set_var("ASM_CONFIG_HOME", &tmp_cfg);

        // Config: surround enabled, channels = ["game"].
        let cfg = make_config_surround_enabled("old-hrir");

        // Pre-populate surround_routed to simulate that "game" is already routed.
        // apply_surround will find to_route = [], to_restore = [] → only sink recreated.
        //
        // recreate surround (recreate_ex, absent):
        //   remove: source_exists → 1 ls (absent)
        //   spawn_conf: writes conf + spawns directly (no second runner call)
        // No channel operations.
        let ls_absent = ls_all_absent();

        let runner = MockRunner::new()
            // recreate surround: remove source_exists (absent)
            .with_output(0, &ls_absent, "");

        let mut engine = Engine::new(runner, cfg);
        // Simulate that channels are already tracked (prior enable).
        engine.surround_routed.insert("game".into());
        engine.surround_routed.insert("media".into());

        engine
            .surround_set_hrir("new-hrir".into())
            .expect("surround_set_hrir must succeed");

        // Surround sink was respawned (HRIR change took effect).
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("arctis_surround"))
                .unwrap_or(false)),
            "surround sink must be respawned after HRIR change: {:?}",
            engine.runner.spawned
        );

        // No channel thrash: game and media were NOT respawned (already tracked).
        assert!(
            !engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("Arctis_Game") || s.contains("Arctis_Media"))
                .unwrap_or(false)),
            "channels must NOT be thrashed when HRIR changes (already in surround_routed): {:?}",
            engine.runner.spawned
        );

        // surround_routed unchanged.
        assert!(engine.surround_routed.contains("game"));
        assert!(engine.surround_routed.contains("media"));

        let _ = std::fs::remove_dir_all(&tmp_home);
        let _ = std::fs::remove_dir_all(&tmp_cfg);
        std::env::remove_var("HOME");
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    // ─────────────────────────────────────────────
    // Task B6b: surround mode-selection + EQ-on-binaural-output
    // ─────────────────────────────────────────────

    /// Helper: make a config where "game" is the single surround channel with empty EQ.
    fn make_config_surround_single_game_no_eq(hrir_stem: &str) -> Config {
        Config {
            version: arctis_config::CURRENT_VERSION,
            active_profile: "default".into(),
            profiles: vec![Profile {
                name: "default".into(),
                channels: vec![ChannelConfig {
                    id: "game".into(),
                    node_name: "Arctis_Game".into(),
                    description: "Game".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                }],
                routes: vec![],
                mic: MicChainConfig::default(),
                surround: arctis_config::SurroundConfig {
                    enabled: true,
                    hrir: Some(hrir_stem.into()),
                    channels: vec!["game".into()],
                    hw_sink: None,
                    ..Default::default()
                },
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

    /// Queue 3 MockRunner outputs for a direct `apply_surround` call where surround and
    /// the channel sink are both absent:
    ///   recreate_ex: remove source_exists (absent, 1 ls) + spawn_conf (no ls) = 1 ls
    ///   set_output game: remove sink_exists (absent) + create sink_exists (absent → spawn) = 2 ls
    fn queue_apply_surround_both_absent(runner: MockRunner) -> MockRunner {
        scrub_stale_confs();
        let ls_absent = ls_all_absent();
        runner
            .with_output(0, &ls_absent, "") // recreate_ex: remove source_exists (absent)
            .with_output(0, &ls_absent, "") // set_output game: remove sink_exists (absent)
            .with_output(0, &ls_absent, "") // set_output game: create sink_exists (absent → spawn)
    }

    /// Back-compat: a default profile (game channel, NO custom EQ) with surround enabled
    /// must produce an 8-channel HRIR conf WITHOUT an EQ tail — exactly as before this task.
    #[test]
    fn apply_surround_default_profile_renders_8ch_no_eq_conf() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("b6b_no_eq");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        let runner = queue_apply_surround_both_absent(MockRunner::new());
        let cfg = make_config_surround_single_game_no_eq("test-hrir");
        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        // Find the surround conf spawn (path contains "arctis_surround").
        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| {
                argv.get(2)
                    .map(|s| s.contains("arctis_surround"))
                    .unwrap_or(false)
            })
            .expect("surround conf must have been spawned");
        let conf = std::fs::read_to_string(&surround_argv[2])
            .expect("surround conf file must exist");

        // 8-channel HRIR (default Auto → hrir71).
        assert!(
            conf.contains("audio.channels = 8"),
            "default profile must produce an 8-channel HRIR conf, got:\n{conf}"
        );
        // No EQ tail: game channel has empty eq → output_eq=None → no eq nodes in conf.
        assert!(
            !conf.contains("\"eq_l_0\""),
            "no EQ tail expected when game channel has no custom EQ, got:\n{conf}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    /// The HRIR convolver's OUTPUT must reach the headset. When `surround.hw_sink` is
    /// unset (the common case — profiles store None = "auto → headset"), apply_surround
    /// must default the convolver output to the DETECTED headset sink, mirroring how
    /// reconcile defaults each channel's output via `overlay_default_output`. Otherwise
    /// the convolver output falls to the system default sink (e.g. onboard speakers) and
    /// the HRIR-processed audio never reaches the headphones.
    #[test]
    fn apply_surround_defaults_convolver_output_to_detected_headset_when_hw_sink_unset() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surround_hwsink_default");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        // Runner: detect_headset_sink probes first (pw-metadata 0, then a pw-dump of real
        // sinks that includes the SteelSeries Arctis hw sink), then the recreate/set_output
        // existence checks (absent → spawn).
        let sinks_dump = include_str!("../../../audio/tests/fixtures/pw_dump_sinks.json");
        let runner = queue_apply_surround_both_absent(
            MockRunner::new()
                .with_output(0, "[]", "") // Auto probe: pw-dump (no streams)
                .with_output(0, PW_METADATA_SINK, "") // detect: pw-metadata 0 (default sink)
                .with_output(0, sinks_dump, ""), // detect: pw-dump (real sinks)
        );
        let cfg = make_config_surround_single_game_no_eq("test-hrir"); // surround.hw_sink = None
        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| {
                argv.get(2)
                    .map(|s| s.contains("arctis_surround"))
                    .unwrap_or(false)
            })
            .expect("surround conf must have been spawned");
        let conf =
            std::fs::read_to_string(&surround_argv[2]).expect("surround conf file must exist");

        assert!(
            conf.contains("target.object")
                && conf.contains(
                    "alsa_output.usb-SteelSeries_Arctis_Nova_Pro_Wireless-00.analog-stereo"
                ),
            "convolver output must target the detected headset, got:\n{conf}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    /// One-EQ-per-channel: a profile whose "game" channel has a non-empty EQ must
    /// (a) produce a surround conf with NO eq_l_0 / eq_r_0 tail nodes (no config-driven
    ///     post-convolution EQ), AND
    /// (b) route the game channel sink WITH its own EQ (custom band gain present in the
    ///     channel conf) — i.e. the channel EQ is applied pre-convolution.
    #[test]
    fn apply_surround_game_eq_stays_on_channel_sink_no_tail() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("b6b_eq_tail");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        // Use a distinctive gain value (6.0 dB) to identify whether the custom EQ ends up
        // in the surround conf tail vs. the channel sink conf.  The conf renderer uses
        // `fmt_num` which formats integral-valued floats as "{:.1}" → "6.0", NOT "6".
        // Using "6.0" is safe: it does not appear in band-index names like "eq_band_6".
        let custom_gain: f32 = 6.0;
        let gain_str_in_conf = format!("{custom_gain:.1}"); // "6.0"
        let mut cfg = make_config_surround_single_game_no_eq("test-hrir");
        cfg.profiles[0].channels[0].eq = vec![arctis_config::EqBandConfig {
            kind: "peaking".into(),
            freq_hz: 1000.0,
            q: 1.0,
            gain_db: custom_gain,
        }];

        let runner = queue_apply_surround_both_absent(MockRunner::new());
        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        // (a) Surround conf must have NO EQ tail nodes (no config-driven post-conv EQ).
        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| {
                argv.get(2)
                    .map(|s| s.contains("arctis_surround"))
                    .unwrap_or(false)
            })
            .expect("surround conf must have been spawned");
        let surround_conf = std::fs::read_to_string(&surround_argv[2])
            .expect("surround conf file must exist");
        assert!(
            !surround_conf.contains("\"eq_l_0\""),
            "surround conf must NOT contain eq_l_0 tail node (channel EQ stays pre-convolution)"
        );

        // (b) Game channel sink conf must carry its own custom gain (EQ applied pre-conv).
        // The channel conf is at {tmp_dir}/arctis_eq.Arctis_Game.conf.
        let game_conf_path = std::env::temp_dir().join("arctis_eq.Arctis_Game.conf");
        let game_conf = std::fs::read_to_string(&game_conf_path)
            .expect("game channel conf file must exist");
        assert!(
            game_conf.contains(&gain_str_in_conf),
            "game channel sink conf MUST contain custom gain {gain_str_in_conf} (EQ pre-convolution), conf:\n{game_conf}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    /// A pinned `blocksize` must be carried into the spawned surround conf, and — since the
    /// pinned HRIR is present — `state().surround.hrir_missing` must stay `None`. There is
    /// no config-driven post-convolution EQ tail.
    #[test]
    fn apply_surround_carries_blocksize_and_clears_missing_no_tail() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("t6_explicit_eq");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("g.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        let mut cfg = make_config_surround_single_game_no_eq("g");
        cfg.profiles[0].surround.blocksize = Some(128);

        let runner = queue_apply_surround_both_absent(MockRunner::new());
        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        // HRIR present → no missing flag.
        assert_eq!(engine.state().surround.hrir_missing, None);

        // Inspect the spawned surround conf (established mechanism: find the "arctis_surround"
        // spawn argv and read the conf file at argv[2]).
        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| {
                argv.get(2)
                    .map(|s| s.contains("arctis_surround"))
                    .unwrap_or(false)
            })
            .expect("surround conf must have been spawned");
        let conf = std::fs::read_to_string(&surround_argv[2])
            .expect("surround conf file must exist");
        assert!(
            conf.contains("blocksize = 128"),
            "surround conf must carry pinned blocksize, got:\n{conf}"
        );
        assert!(
            !conf.contains("\"eq_l_0\""),
            "surround conf must NOT carry an EQ tail (no config-driven post-conv EQ), got:\n{conf}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    /// A pinned HRIR stem that is not installed must fall back to the bundled dry HRIR
    /// (`07-oal+++-openal-max`) and record the missing stem in `state().surround.hrir_missing`.
    #[test]
    fn apply_surround_missing_pinned_hrir_falls_back_and_sets_flag() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("t6_missing_hrir");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        // Only the bundled fallback is present; the pinned stem is absent.
        std::fs::write(profiles_dir.join("07-oal+++-openal-max.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        let cfg = make_config_surround_single_game_no_eq("04-gsx-sennheiser-gsx");
        let runner = queue_apply_surround_both_absent(MockRunner::new());
        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        assert_eq!(
            engine.state().surround.hrir_missing,
            Some("04-gsx-sennheiser-gsx".to_string())
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    /// StereoBypass mode: spawns a 2-channel conf with no convolver, no HRIR path needed.
    #[test]
    fn apply_surround_stereo_bypass_mode_spawns_2ch_conf() {
        // Hold ENV_MUTEX to serialize with other tests that write to /tmp/arctis_arctis_surround.conf
        // even though StereoBypass doesn't need HOME (no HRIR resolution).
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let runner = queue_apply_surround_both_absent(MockRunner::new());
        let mut cfg = make_config_surround_single_game_no_eq("unused-hrir");
        cfg.profiles[0].surround.mode = SurroundMode::StereoBypass;

        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround with StereoBypass must succeed");

        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| {
                argv.get(2)
                    .map(|s| s.contains("arctis_surround"))
                    .unwrap_or(false)
            })
            .expect("surround conf must have been spawned");
        let conf = std::fs::read_to_string(&surround_argv[2])
            .expect("surround conf file must exist");

        assert!(
            conf.contains("audio.channels = 2"),
            "StereoBypass mode must produce a 2-channel conf, got:\n{conf}"
        );
        assert!(
            !conf.contains("convolver"),
            "StereoBypass mode conf must NOT contain any convolver node, got:\n{conf}"
        );
    }

    /// state() surround snapshot must expose `mode` and `effective_mode` strings.
    /// Default config has mode=Auto → effective_mode="hrir71" (no negotiated channels).
    #[test]
    fn surround_snapshot_exposes_mode_and_effective_mode() {
        // Use a config with surround enabled and default mode (Auto).
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround = arctis_config::SurroundConfig {
            enabled: true,
            hrir: None,
            channels: vec!["game".into()],
            hw_sink: None,
            ..Default::default() // mode = Auto, crossfeed = 0
        };

        let mut engine = Engine::new(MockRunner::new(), cfg);
        let st = engine.state();

        // mode must reflect the configured value.
        assert_eq!(
            st.surround.mode, "auto",
            "mode must be 'auto' for default SurroundConfig, got: {:?}",
            st.surround.mode
        );
        // effective_mode for Auto + no negotiated channels → Hrir71 → "hrir71".
        assert_eq!(
            st.surround.effective_mode, "hrir71",
            "effective_mode must be 'hrir71' when mode=Auto and no negotiated count, got: {:?}",
            st.surround.effective_mode
        );
        // negotiated_channels not yet probed → None.
        assert_eq!(
            st.surround.negotiated_channels, None,
            "negotiated_channels must be None before any pw-dump probe"
        );
    }

    #[test]
    fn surround_snapshot_stereo_bypass_mode_string() {
        // Set mode explicitly to StereoBypass and verify the snapshot produces "stereo_bypass".
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround = arctis_config::SurroundConfig {
            enabled: true,
            mode: arctis_config::SurroundMode::StereoBypass,
            hrir: None,
            channels: vec!["game".into()],
            hw_sink: None,
            ..Default::default()
        };

        let mut engine = Engine::new(MockRunner::new(), cfg);
        let st = engine.state();

        // mode must reflect StereoBypass as "stereo_bypass" (snake_case, not "stereobypass").
        assert_eq!(
            st.surround.mode, "stereo_bypass",
            "mode must be 'stereo_bypass' for SurroundMode::StereoBypass, got: {:?}",
            st.surround.mode
        );
        // effective_mode for explicit StereoBypass → "stereo_bypass".
        assert_eq!(
            st.surround.effective_mode, "stereo_bypass",
            "effective_mode must be 'stereo_bypass' when mode=StereoBypass, got: {:?}",
            st.surround.effective_mode
        );
    }

    #[test]
    fn state_reports_negotiated_surround_input_for_game_channel() {
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround.enabled = true;
        cfg.profiles[0].surround.channels = vec!["game".into()];

        // DayZ 7.1 stream linked to the Arctis_Game sink.
        let dump = r#"[
          { "id": 50, "type": "PipeWire:Interface:Node",
            "info": { "props": { "media.class": "Audio/Sink", "node.name": "Arctis_Game" } } },
          { "id": 51, "type": "PipeWire:Interface:Node",
            "info": { "props": { "media.class": "Stream/Output/Audio",
                "application.name": "DayZ", "application.process.binary": "DayZ" },
              "params": { "Format": [ { "channels": 8,
                "position": ["FL","FR","FC","LFE","RL","RR","SL","SR"] } ] } } },
          { "id": 99, "type": "PipeWire:Interface:Link",
            "info": { "output-node-id": 51, "input-node-id": 50 } }
        ]"#;
        let runner = MockRunner::new().with_output(0, dump, "");
        let mut engine = Engine::new(runner, cfg);
        let st = engine.state();
        assert_eq!(st.surround.negotiated_channels, Some(8));
        assert_eq!(st.surround.negotiated_surround, Some(true));
    }

    #[test]
    fn state_reports_none_surround_input_when_no_game_stream() {
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround.enabled = true;
        cfg.profiles[0].surround.channels = vec!["game".into()];
        // pw-dump with no app streams.
        let runner = MockRunner::new().with_output(0, "[]", "");
        let mut engine = Engine::new(runner, cfg);
        let st = engine.state();
        assert_eq!(st.surround.negotiated_channels, None);
        assert_eq!(st.surround.negotiated_surround, None);
    }
}
