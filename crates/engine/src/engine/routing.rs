//! Per-app routing, stream discovery/moves, output sinks, default-sink control.
use super::*;

impl<R: CommandRunner> Engine<R> {
    /// Project the active profile's routes — the single source of truth (G4) —
    /// into `routes.json` + the persistent conf fragments via Router. Building
    /// the Router from the FULL route set is what keeps sibling rules intact:
    /// constructing it empty and applying one rule used to clobber every other
    /// persisted route on each GUI route change.
    fn project_routes(&mut self) -> Result<(), EngineError> {
        let rules = convert::route_rules_from_profile(self.config.active()?);
        let router = Router::with_rules(&mut self.runner, rules);
        router.save_persistent()?;
        Ok(())
    }

    /// Persist a route rule without doing a live move.
    ///
    /// Upserts the route in the active profile's in-memory config, writes the unified
    /// config to disk, and re-projects ALL profile routes into the persistent
    /// fragments via Router. Emits `RouteSet`. Does NOT call `apply_live`.
    fn persist_route(&mut self, app_binary: &str, target_sink: &str) -> Result<(), EngineError> {
        // Update in-memory config
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            if let Some(existing) = profile
                .routes
                .iter_mut()
                .find(|r| r.app_binary == app_binary)
            {
                existing.target_sink = target_sink.to_string();
            } else {
                profile.routes.push(arctis_config::RouteConfig {
                    app_binary: app_binary.to_string(),
                    target_sink: target_sink.to_string(),
                });
            }
        }
        // Persist unified config
        self.save_config()?;
        // Re-project the FULL route set (no live move)
        self.project_routes()?;
        // Emit event
        self.emit(Event::RouteSet {
            app_binary: app_binary.to_string(),
            target_sink: target_sink.to_string(),
        });
        Ok(())
    }

    /// Add/upsert a route in the active profile, persist, set_rule + save_persistent + apply_live.
    pub fn set_route(&mut self, app_binary: &str, target_sink: &str) -> Result<(), EngineError> {
        self.persist_route(app_binary, target_sink)?;
        // Best-effort live move by binary (ignore error if app not running)
        {
            let mut router = Router::new(&mut self.runner);
            let _ = router.apply_live(&AppMatch::Binary(app_binary.to_string()), target_sink);
        }
        Ok(())
    }

    /// Remove the routing rule for `app_binary`.
    ///
    /// Drops the rule from in-memory config + persists, re-projects the remaining
    /// routes into the persistent fragments (siblings survive), and attempts a
    /// best-effort live clear (moves the stream back to the default sink by
    /// deleting its `target.object` metadata key).
    ///
    /// KNOWN LIMITATION: WirePlumber's restore-stream module
    /// (`node.stream.restore-target`, default true) remembers the app's last
    /// manual target in `~/.local/state/wireplumber/restore-stream`; there is no
    /// supported way to clear a single app's entry without restarting
    /// WirePlumber, so the app's NEXT stream may reappear on the old sink until
    /// the user moves it once. See KNOWN_ISSUES.md (KI-6).
    pub fn clear_route(&mut self, app_binary: &str) -> Result<(), EngineError> {
        // Update in-memory config
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            profile.routes.retain(|r| r.app_binary != app_binary);
        }
        // Persist unified config
        self.save_config()?;
        // Re-project the REMAINING routes (clear_route used to write an empty
        // rule set here, wiping every other persisted route).
        self.project_routes()?;
        // Best-effort live clear (ignore error if app not running)
        {
            let mut router = Router::new(&mut self.runner);
            let _ = router.clear_live(&AppMatch::Binary(app_binary.to_string()));
        }
        // Emit event
        self.emit(Event::RouteCleared {
            app_binary: app_binary.to_string(),
        });
        Ok(())
    }

    /// Per-node application output streams (one entry per PipeWire output node),
    /// each resolved to a channel id (via its linked sink node.name) and flagged
    /// with a persistent route. One `pw-dump` per call; pure mapping otherwise.
    /// Read-only. This is the pre-dedup view used by routing, which must act on
    /// EVERY node of an app — `list_streams` collapses it to one badge per app.
    fn list_app_streams_raw(&mut self) -> Result<Vec<crate::state::AppStream>, EngineError> {
        // node.name -> channel id, built from the active profile (never hard-coded).
        let (name_to_id, routed_bins): (
            std::collections::HashMap<String, String>,
            std::collections::HashSet<String>,
        ) = {
            let profile = self.config.active()?;
            let map = profile
                .channels
                .iter()
                .map(|c| (c.node_name.clone(), c.id.clone()))
                .collect();
            let routed = profile
                .routes
                .iter()
                .map(|r| r.app_binary.clone())
                .collect();
            (map, routed)
        };

        let out = self.runner.run("pw-dump", &[])?;
        if out.status != 0 {
            return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                program: "pw-dump".into(),
                status: out.status,
                stderr: out.stderr,
            }));
        }
        let parsed = arctis_audio::parse_app_streams(&out.stdout)?;
        let mut per_node: Vec<crate::state::AppStream> = parsed
            .into_iter()
            .map(|p| crate::state::AppStream {
                current_channel: p
                    .sink_node_name
                    .as_ref()
                    .and_then(|n| name_to_id.get(n).cloned()),
                routed: routed_bins.contains(&p.binary),
                id: p.id,
                binary: p.binary,
                app_name: p.app_name,
                pid: p.pid,
                icon_name: p.icon_name,
                media_name: p.media_name,
            })
            .collect();
        // Stable order by node id so the lowest-id node is the dedup representative.
        per_node.sort_by_key(|s| s.id);
        Ok(per_node)
    }

    /// Discover running application output streams for the mixer: ONE badge per
    /// app. Read-only (no graph mutation).
    pub fn list_streams(&mut self) -> Result<Vec<crate::state::AppStream>, EngineError> {
        let per_node = self.list_app_streams_raw()?;
        // Collapse multiple output nodes of the same app into ONE badge. A browser
        // (Vivaldi/Chrome) can hold several Stream/Output/Audio nodes at once — e.g.
        // a second node appears when a video starts — and one-per-node would show
        // duplicate badges. Keep the lowest-id node (per_node is id-sorted) as the
        // representative; if a sibling is linked to a channel, adopt that channel so
        // a routed stream isn't hidden behind an unlinked one. (Routing is
        // per-binary, so `routed` is identical across a group.)
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut deduped: Vec<crate::state::AppStream> = Vec::with_capacity(per_node.len());
        for s in per_node {
            if let Some(&idx) = seen.get(&s.binary) {
                if deduped[idx].current_channel.is_none() && s.current_channel.is_some() {
                    deduped[idx].current_channel = s.current_channel;
                }
                continue;
            }
            seen.insert(s.binary.clone(), deduped.len());
            deduped.push(s);
        }
        Ok(deduped)
    }

    /// Discover real output sinks (physical devices) via `pw-metadata 0` + `pw-dump`.
    ///
    /// Best-effort: on any subprocess failure an empty `Vec` is returned.
    /// Never panics; never returns `Err`. Shared by the UI output selector and
    /// the headset-sink detection helpers below (G1).
    fn list_output_sinks(&mut self) -> Vec<arctis_audio::OutputSink> {
        // Step 1: get default sink name from pw-metadata 0.
        let default_sink = match self.runner.run("pw-metadata", &["0"]) {
            Ok(out) if out.status == 0 => arctis_audio::parse_default_sink_name(&out.stdout),
            _ => None,
        };

        // Step 2: run pw-dump to enumerate all nodes.
        let out = match self.runner.run("pw-dump", &[]) {
            Ok(o) => o,
            Err(_) => return Vec::new(),
        };
        if out.status != 0 {
            return Vec::new();
        }

        // Step 3: parse.
        arctis_audio::parse_output_sinks(&out.stdout, default_sink.as_deref())
            .unwrap_or_default()
    }

    /// Discover real output devices (physical sinks) for the UI output selector.
    pub fn list_output_devices(&mut self) -> Vec<crate::state::OutputDeviceSnapshot> {
        self.list_output_sinks()
            .into_iter()
            .map(|s| crate::state::OutputDeviceSnapshot {
                node_name: s.node_name,
                description: s.description,
                is_default: s.is_default,
            })
            .collect()
    }

    /// True when the sink looks like the headset hardware sink.
    fn is_headset_sink(node_name: &str) -> bool {
        let lower = node_name.to_lowercase();
        // Virtual channel sinks are "Arctis_<Channel>" — parse_output_sinks
        // already excludes them, so a plain "arctis" match is safe here.
        lower.contains("steelseries") || lower.contains("arctis")
    }

    /// Detect the headset hardware sink by scanning the real output sinks for a
    /// node_name that contains "steelseries" or "arctis" (case-insensitive).
    /// Returns `None` if no hardware headset sink is found or on any subprocess
    /// failure. Never panics.
    pub fn detect_headset_sink(&mut self) -> Option<String> {
        self.list_output_sinks()
            .into_iter()
            .find(|d| Self::is_headset_sink(&d.node_name))
            .map(|d| d.node_name)
    }

    /// Detect the headset hardware sink's PipeWire object id (for `wpctl <id>`,
    /// which does not accept node names). Same discovery as `detect_headset_sink`.
    pub(super) fn detect_headset_sink_id(&mut self) -> Option<u32> {
        self.list_output_sinks()
            .into_iter()
            .find(|d| Self::is_headset_sink(&d.node_name))
            .map(|d| d.id)
    }

    /// Route a running stream to a channel: resolve channel id → sink node.name,
    /// live-move the specific stream (by node id) via pw-metadata, and persist a
    /// binary→sink rule so it sticks next launch. `stream` may be a node id or a
    /// binary; the binary is resolved from discovery for persistence.
    pub fn move_stream(&mut self, stream: &str, channel_id: &str) -> Result<(), EngineError> {
        // Resolve channel -> sink node.name from the active profile.
        let sink = {
            let profile = self.config.active()?;
            profile
                .channels
                .iter()
                .find(|c| c.id == channel_id)
                .map(|c| c.node_name.clone())
                .ok_or_else(|| EngineError::BadRequest(format!("unknown channel: {channel_id}")))?
        };

        // Find the requested stream (by node id string or by binary) to resolve
        // which APP (binary) is being routed. Use the per-node view, not the
        // deduped badge list, so we can move every node of that app below.
        let streams = self.list_app_streams_raw()?;
        let target = streams
            .iter()
            .find(|s| s.id.to_string() == stream || s.binary == stream)
            .ok_or_else(|| EngineError::BadRequest(format!("no running stream: {stream}")))?
            .clone();

        // Live-move EVERY currently-running output node of this app, not just the
        // matched one. Browsers (Vivaldi/Chrome) hold multiple output nodes; moving
        // a single node leaves the others on the old sink, which is the "jumps back"
        // symptom the user sees. Routing is per-binary, so move all same-binary nodes.
        let node_ids: Vec<u32> = streams
            .iter()
            .filter(|s| s.binary == target.binary)
            .map(|s| s.id)
            .collect();
        for id in node_ids {
            let argv = move_stream_argv(&id.to_string(), &sink)?;
            let args: Vec<&str> = argv.iter().map(String::as_str).collect();
            let out = self.runner.run("pw-metadata", &args)?;
            if out.status != 0 {
                return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                    program: "pw-metadata".into(),
                    status: out.status,
                    stderr: out.stderr,
                }));
            }
        }

        // Persist binary -> sink without a second live move.
        // The per-node moves above already covered every running instance; calling
        // set_route would also trigger a best-effort binary-match live move (an
        // extra pw-dump + pw-metadata). persist_route writes config + WP fragment +
        // emits RouteSet without any additional pw-dump / pw-metadata calls.
        self.persist_route(&target.binary, &sink)?;
        Ok(())
    }

    /// Set (or clear) which channel's sink is the system default output. When set,
    /// uses `pw-metadata -n default 0 default.configured.audio.sink {"name":"<node.name>"}`
    /// to set the default by name (robust across restarts; `wpctl set-default` requires a
    /// numeric node-id which is ephemeral and would error with "is not a valid number").
    /// Persists + emits.
    pub fn set_default_sink_channel(
        &mut self,
        channel: Option<String>,
    ) -> Result<(), EngineError> {
        // Validate + resolve sink before mutating.
        let sink = match &channel {
            Some(id) => {
                let p = self.config.active()?;
                Some(
                    p.channels
                        .iter()
                        .find(|c| &c.id == id)
                        .map(|c| c.node_name.clone())
                        .ok_or_else(|| {
                            EngineError::BadRequest(format!("unknown channel: {id}"))
                        })?,
                )
            }
            None => None,
        };
        {
            let name = self.config.active_profile.clone();
            let p = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            p.default_sink_channel = channel.clone();
        }
        self.save_config()?;
        if let Some(sink_name) = sink {
            // pw-metadata accepts node names; wpctl set-default requires a numeric id.
            let val = format!("{{\"name\":\"{sink_name}\"}}");
            let out = self.runner.run(
                "pw-metadata",
                &["-n", "default", "0", "default.configured.audio.sink", &val],
            )?;
            if out.status != 0 {
                return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                    program: "pw-metadata".into(),
                    status: out.status,
                    stderr: out.stderr,
                }));
            }
        }
        self.emit(Event::DefaultSinkChannelSet { channel });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
