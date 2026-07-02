//! Reconcile: pure planning (plan_reconcile) and bringing the live graph to config.
use super::*;

/// A reconcile-step descriptor used for pure planning + test assertions before any I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileStep {
    ChannelsUp,
    ApplyEq {
        channel_id: String,
    },
    SetOutput {
        channel_id: String,
        device: Option<String>,
    },
    RouteSet {
        app_binary: String,
        target_sink: String,
    },
}

/// Pure planner: compute the ordered step list for a profile (no I/O). Lets us unit-test the
/// reconcile PLAN independently of execution (G8).
pub fn plan_reconcile(cfg: &Config) -> Result<Vec<ReconcileStep>, EngineError> {
    let profile = cfg.active()?;
    let mut steps = Vec::new();

    // Step 1: bring channels up
    steps.push(ReconcileStep::ChannelsUp);

    // Step 2: per-channel EQ apply
    for ch in &profile.channels {
        steps.push(ReconcileStep::ApplyEq {
            channel_id: ch.id.clone(),
        });
    }

    // Step 3: per-channel output device overrides
    for ch in &profile.channels {
        if ch.output_device.is_some() {
            steps.push(ReconcileStep::SetOutput {
                channel_id: ch.id.clone(),
                device: ch.output_device.clone(),
            });
        }
    }

    // Step 4: routing rules
    for route in &profile.routes {
        steps.push(ReconcileStep::RouteSet {
            app_binary: route.app_binary.clone(),
            target_sink: route.target_sink.clone(),
        });
    }

    Ok(steps)
}

impl<R: CommandRunner> Engine<R> {
    /// Bring the live graph to match the active profile. Idempotent. Order:
    ///   1. ChannelManager::up(default flat eq) — creates sinks, tracking spawn_owned tokens
    ///   2. per channel: AudioBackend/ChannelManager apply_all(eq_model_for(channel))
    ///   3. per channel with output_device: ChannelManager::set_output(...)
    ///   4. Router: set_rule for each route, save_persistent, then apply_live best-effort
    ///
    /// Reuses ChannelManager/Router/AudioBackend — does NOT reimplement.
    pub fn reconcile(&mut self) -> Result<(), EngineError> {
        let mut profile = self.config.active()?.clone();
        if let Some(headset) = self.detect_headset_sink() {
            convert::overlay_default_output(&mut profile.channels, &headset);
        }
        let channel_set = convert::channel_set_from_profile(&profile);
        let route_rules = convert::route_rules_from_profile(&profile);

        // Step 1: channels up — track any freshly-spawned pipewire instances.
        // Record which channel IDs were freshly spawned (token.is_some()) so Step 2
        // can treat the post-spawn apply_all as non-fatal for those channels.
        let freshly_spawned: std::collections::HashSet<String> = {
            let mut mgr = ChannelManager::new(&mut self.runner, channel_set.clone());
            let flat_eq = EqModel::default_10band();
            let pairs = mgr.up(&flat_eq)?;
            let mut fresh = std::collections::HashSet::new();
            for (i, (_handle, token)) in pairs.into_iter().enumerate() {
                if let Some(t) = token {
                    // Record the channel id as freshly spawned
                    if let Some(ch) = profile.channels.get(i) {
                        fresh.insert(ch.id.clone());
                    }
                    self.children.track(t);
                }
            }
            fresh
        };

        // Step 2: per-channel EQ apply.
        // For freshly-spawned channels the EQ is already baked into the filter-chain
        // conf written at spawn time, so this live apply_all is a redundant re-apply
        // for the initial state. A transient "node not yet registered" race (PipeWire
        // hasn't published the node to the graph yet) must not fail daemon startup —
        // log a warning and continue. For channels that were already present we treat
        // the error the same way (non-fatal warn) for consistency; those are idempotent
        // re-applies and a transient error there is equally recoverable.
        for ch in &profile.channels {
            let eq_model = convert::eq_model_for(ch)?;
            let def = convert::channel_def_from_cfg(ch);
            let spec = def.sink_spec();
            {
                let mut be = arctis_audio::AudioBackend::new(&mut self.runner, spec.clone());
                if let Err(e) = be.apply_all(&eq_model) {
                    let freshness = if freshly_spawned.contains(&ch.id) {
                        "freshly-spawned"
                    } else {
                        "already-present"
                    };
                    eprintln!(
                        "warning: reconcile apply_all for channel '{}' ({freshness}) failed \
                         (EQ is conf-baked; ignoring): {e}",
                        ch.id
                    );
                }
            }
            // Volume/mute apply
            {
                let mut be2 = arctis_audio::AudioBackend::new(&mut self.runner, spec);
                if let Err(e) = be2.apply_volume_mute_pct(ch.volume_pct, ch.muted) {
                    eprintln!(
                        "warning: reconcile apply_volume_mute_pct for channel '{}' failed (ignoring): {e}",
                        ch.id
                    );
                }
            }
        }

        // Step 3: per-channel output device overrides.
        // SKIP surround-routed channels: their live output is owned by Step 6
        // (apply_surround routes them to effect_input.arctis_surround). Without this
        // skip, Step 3 re-points the channel to its output_device (the headset, via
        // overlay_default_output) on every reconcile, while apply_surround's no-thrash
        // guard then declines to re-route it — silently bypassing the HRIR after the
        // first reconcile (bug C1). One writer per channel = no fight.
        for ch in &profile.channels {
            if ch.output_device.is_some()
                && !convert::surround_routes_channel(&profile.surround, &ch.id)
            {
                let eq_model = convert::eq_model_for(ch)?;
                let mut mgr = ChannelManager::new(&mut self.runner, channel_set.clone());
                let handle = mgr.set_output(&ch.id, ch.output_device.clone(), &eq_model)?;
                if let Some(t) = handle.child {
                    self.children.track(t);
                }
            }
        }

        // Step 4: routing rules — persistent projection only (apply_live is
        // best-effort and needs live streams). Always save, even when empty:
        // routes.json and the conf fragments are pure projections of
        // profile.routes (G4), so an empty profile must clear them rather than
        // leave stale rules behind. save_persistent is disk-only (no runner calls).
        {
            let router = Router::with_rules(&mut self.runner, route_rules);
            router.save_persistent()?;
        }

        // Step 5: mic source build/teardown (Clean Mic virtual Audio/Source).
        // Query PW version once (cached) then build availability from the probe.
        self.ensure_pw_version();
        let (nodes, availability) =
            convert::mic_chain_nodes(&profile.mic, self.probe.as_ref(), self.builtin_noisegate);
        self.mic_availability = availability;

        if !profile.mic.enabled {
            // Master switch off: remove the source (idempotent).
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            if let Err(e) = mic_be.remove() {
                eprintln!("warning: reconcile mic remove failed (ignoring): {e}");
            }
        } else {
            // Master switch on: create (idempotent — no-op if already present).
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            match mic_be.create(&nodes) {
                Ok(handle) => {
                    if let Some(token) = handle.child {
                        self.children.track(token);
                    }
                }
                Err(e) => {
                    // Transient post-spawn find_node_id race is non-fatal (mirror channel pattern).
                    eprintln!("warning: reconcile mic create failed (post-spawn race?): {e}");
                }
            }
        }

        // Step 6: surround sink create/teardown + channel re-routing (surround channels → effect_input.arctis_surround).
        // AFTER step 3/4 so that surround re-routing overrides any output-device set in step 3.
        if let Err(e) = self.apply_surround(&profile) {
            eprintln!("warning: reconcile surround step failed (ignoring): {e}");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;
