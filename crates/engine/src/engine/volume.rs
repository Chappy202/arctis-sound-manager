//! Channel/master/mic volume + mute and the ChatMix slider/dial paths.
use super::*;

/// Full attenuation applied to the losing side of the ChatMix dial (dB).
const CHATMIX_FULL_ATTEN_DB: f32 = -40.0;

/// Derive the GUI ChatMix position 0..=9 from the two hardware-reported mix
/// levels (`media_mix` = game level, `chat_mix` = chat level, each 0..=100).
/// 9 = full game (chat fully attenuated), 0 = full chat (game attenuated).
/// Both inputs are clamped to 0..=100 before the computation.
pub fn mix_to_chatmix_position(media_mix: u8, chat_mix: u8) -> i64 {
    let m = media_mix.min(100) as i64;
    let c = chat_mix.min(100) as i64;
    let raw = (((m - c + 100) as f32) / 200.0 * 9.0).round() as i64;
    raw.clamp(0, 9)
}

/// Map a ChatMix position 0..=9 to (game_db, chat_db) attenuations.
/// center = 4.5 is the true midpoint of the 0..=9 range and matches the position
/// readback mapping in `mix_to_chatmix_position` (used by the hardware dial path in
/// `crates/cli/src/dial.rs`). Because the center is 4.5, no integer position is
/// exactly neutral: position 4 leans slightly toward chat (game ~-4.4 dB) and
/// position 5 slightly toward game; true balance sits between 4 and 5.
/// Endpoints: 9 => full game (chat fully attenuated), 0 => full chat (game attenuated).
///
/// GUI-slider vs hardware-dial guarantee: both write through the SAME perceptual
/// (cubic) channelVolumes path, and both derive the reported position via
/// `mix_to_chatmix_position`. The curves differ on the losing side, though: the
/// slider realises an exact dB ramp (0 → CHATMIX_FULL_ATTEN_DB, converted for the
/// cubic path by `db_to_volume_pct_cubic`), while `apply_dial_mix` applies the
/// firmware-reported mix percentages verbatim (the dial's taper is owned by the
/// headset firmware and may reach 0 % = silence at full tilt).
fn chatmix_to_volumes(position: i64) -> (f32, f32) {
    let p = position.clamp(0, 9) as f32;
    let center = 4.5_f32;
    if (p - center).abs() < f32::EPSILON {
        return (0.0, 0.0);
    }
    if p > center {
        // bias toward game: attenuate chat proportionally
        let t = (p - center) / (9.0 - center); // 0..1
        (0.0, CHATMIX_FULL_ATTEN_DB * t)
    } else {
        let t = (center - p) / center; // 0..1
        (CHATMIX_FULL_ATTEN_DB * t, 0.0)
    }
}

/// Map a ChatMix position to the (game_pct, chat_pct) pair to write through the
/// perceptual (cubic) volume path. The dB targets from `chatmix_to_volumes` MUST
/// be converted with `db_to_volume_pct_cubic` (pct = 100·10^(dB/60)): the write
/// path cubes pct/100, so the linear `db_to_volume_pct` (pct = 100·10^(dB/20))
/// would triple the intended attenuation (position 5 ≈ −13.3 dB instead of −4.4;
/// "full" would be −120 dB instead of −40).
pub(crate) fn chatmix_to_volume_pcts(position: i64) -> (u8, u8) {
    let (game_db, chat_db) = chatmix_to_volumes(position);
    (
        db_to_volume_pct_cubic(game_db),
        db_to_volume_pct_cubic(chat_db),
    )
}

impl<R: CommandRunner> Engine<R> {
    /// Set the software volume for a single channel. Validates range, persists, applies live, emits.
    pub fn set_channel_volume(
        &mut self,
        channel_id: &str,
        pct: u8,
    ) -> Result<(), EngineError> {
        if pct > 100 {
            return Err(EngineError::BadRequest(format!(
                "volume_pct {pct} out of range 0..=100"
            )));
        }
        // Mutate config
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
            channel.volume_pct = pct;
        }
        self.save_config()?;
        // Apply live: perceptual (cubic) channelVolumes via pw-cli Props (same scale as
        // wpctl/PipeWire/pavucontrol, and inverse of the parse_node_volume cbrt read):
        // 50% → channelVolumes 0.125 (=0.5^3). Hardware-confirmed to match the system.
        {
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
            be.apply_volume_mute_pct(pct, channel.muted)?;
        }
        self.last_volume_write = Some(std::time::Instant::now());
        self.emit(Event::ChannelVolumeSet {
            channel_id: channel_id.to_string(),
            volume_pct: pct,
        });
        Ok(())
    }

    /// Set the mute state for a single channel. Persists, applies live, emits.
    pub fn set_channel_mute(&mut self, channel_id: &str, muted: bool) -> Result<(), EngineError> {
        // Mutate config
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
            channel.muted = muted;
        }
        self.save_config()?;
        // Apply live: find node id, then wpctl set-mute <id> <1|0>
        {
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
            let id = {
                let mut be = AudioBackend::new(&mut self.runner, spec);
                be.find_node_id()?
            };
            let mute_arg = if muted { "1" } else { "0" };
            let out = self.runner.run("wpctl", &["set-mute", &id, mute_arg])?;
            if out.status != 0 {
                return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                    program: "wpctl".into(),
                    status: out.status,
                    stderr: out.stderr,
                }));
            }
        }
        self.emit(Event::ChannelMuteSet {
            channel_id: channel_id.to_string(),
            muted,
        });
        Ok(())
    }

    /// Resolve the wpctl target for master volume/mute: the REAL hardware
    /// headset sink by object id when present, else `@DEFAULT_AUDIO_SINK@`.
    ///
    /// Master must always control the hardware tail: `set_default_sink_channel`
    /// lets the user point the system default at one of OUR virtual sinks
    /// (e.g. Arctis_Game), and targeting @DEFAULT_AUDIO_SINK@ there would stack
    /// master gain multiplicatively on that channel's own volume (double
    /// attenuation) while leaving the hardware sink uncontrolled.
    fn master_wpctl_target(&mut self) -> String {
        match self.detect_headset_sink_id() {
            Some(id) => id.to_string(),
            None => "@DEFAULT_AUDIO_SINK@".to_string(),
        }
    }

    /// Set the master output gain on the headset hardware sink via wpctl,
    /// persist, and emit MasterVolumeSet.
    pub fn set_master_volume(&mut self, pct: u8) -> Result<(), EngineError> {
        {
            let name = self.config.active_profile.clone();
            let p = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            p.master_volume_pct = pct;
        }
        self.save_config()?;
        let target = self.master_wpctl_target();
        // wpctl interprets the 0..1 factor on the CUBIC user scale (WirePlumber
        // module-mixer-api) — the same perceptual scale as our channel sliders,
        // which is exactly what a master volume slider should feel like.
        let factor = format!("{:.4}", pct as f32 / 100.0);
        let out = self.runner.run("wpctl", &["set-volume", &target, &factor])?;
        if out.status != 0 {
            return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                program: "wpctl".into(),
                status: out.status,
                stderr: out.stderr,
            }));
        }
        self.last_volume_write = Some(std::time::Instant::now());
        self.emit(Event::MasterVolumeSet { volume_pct: pct });
        Ok(())
    }

    /// Mute/unmute the master output (headset hardware sink, with the same
    /// @DEFAULT_AUDIO_SINK@ fallback as volume) via wpctl, persist, emit
    /// MasterMuteSet.
    pub fn set_master_mute(&mut self, muted: bool) -> Result<(), EngineError> {
        {
            let name = self.config.active_profile.clone();
            let p = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            p.master_mute = muted;
        }
        self.save_config()?;
        let target = self.master_wpctl_target();
        let arg = if muted { "1" } else { "0" };
        let out = self.runner.run("wpctl", &["set-mute", &target, arg])?;
        if out.status != 0 {
            return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                program: "wpctl".into(),
                status: out.status,
                stderr: out.stderr,
            }));
        }
        self.emit(Event::MasterMuteSet { muted });
        Ok(())
    }

    /// Set the microphone chain volume (0–100%) via wpctl, persist, emit MicVolumeSet.
    pub fn set_mic_volume(&mut self, pct: u8) -> Result<(), EngineError> {
        if pct > 100 {
            return Err(EngineError::BadRequest(format!(
                "mic volume_pct {pct} out of range 0..=100"
            )));
        }
        // Persist
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            profile.mic.volume_pct = pct;
        }
        self.save_config()?;
        // Apply live: find mic node id, then wpctl set-volume <id> <factor>
        let id = {
            let spec = convert::mic_chain_spec(&self.config.active()?.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            mic_be.find_node_id()?
        };
        let factor = format!("{:.4}", pct as f32 / 100.0);
        let out = self.runner.run("wpctl", &["set-volume", &id, &factor])?;
        if out.status != 0 {
            return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                program: "wpctl".into(),
                status: out.status,
                stderr: out.stderr,
            }));
        }
        self.last_volume_write = Some(std::time::Instant::now());
        self.emit(Event::MicVolumeSet { volume_pct: pct });
        Ok(())
    }

    /// Set ChatMix position (Game<->Chat balance); applies derived volumes to the
    /// game and chat channels, persists position, emits ChatmixSet.
    pub fn set_chatmix(&mut self, position: i64) -> Result<(), EngineError> {
        let pos = position.clamp(0, 9);
        {
            let name = self.config.active_profile.clone();
            let p = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            p.chatmix_position = pos;
        }
        let (game_pct, chat_pct) = chatmix_to_volume_pcts(pos);
        // Reuse set_channel_volume (live + persist) for each side; ignore "channel
        // not found" so profiles lacking game/chat don't hard-fail.
        let _ = self.set_channel_volume("game", game_pct);
        let _ = self.set_channel_volume("chat", chat_pct);
        self.save_config()?;
        self.emit(Event::ChatmixSet { position: pos });
        Ok(())
    }

    /// Apply a live hardware-dial ChatMix reading.
    ///
    /// Sets the GAME sink to `media_mix`% and the CHAT sink to `chat_mix`%
    /// using perceptual (cubic) channelVolumes (same path as `set_channel_volume`), updates
    /// the in-memory `volume_pct` and `chatmix_position`, and stamps
    /// `last_volume_write` so `state()` reflects the new values immediately —
    /// WITHOUT calling `save_config` (no per-tick disk I/O).
    /// Channels absent in the active profile are silently skipped (graceful no-op).
    pub fn apply_dial_mix(&mut self, media_mix: u8, chat_mix: u8) -> Result<(), EngineError> {
        let media_mix = media_mix.min(100);
        let chat_mix = chat_mix.min(100);
        let position = mix_to_chatmix_position(media_mix, chat_mix);

        // Clone channel configs before mutating so we release the shared borrow first.
        let game_ch: Option<arctis_config::ChannelConfig> = self
            .config
            .active()?
            .channels
            .iter()
            .find(|ch| ch.id == "game")
            .cloned();
        let chat_ch: Option<arctis_config::ChannelConfig> = self
            .config
            .active()?
            .channels
            .iter()
            .find(|ch| ch.id == "chat")
            .cloned();

        // Update in-memory volume_pct and chatmix_position — no save_config.
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            if game_ch.is_some() {
                if let Some(ch) = profile.channels.iter_mut().find(|ch| ch.id == "game") {
                    ch.volume_pct = media_mix;
                }
            }
            if chat_ch.is_some() {
                if let Some(ch) = profile.channels.iter_mut().find(|ch| ch.id == "chat") {
                    ch.volume_pct = chat_mix;
                }
            }
            profile.chatmix_position = position;
        }

        // Apply live to game sink (no-op when absent).
        if let Some(channel) = game_ch {
            let def = convert::channel_def_from_cfg(&channel);
            let spec = def.sink_spec();
            let mut be = AudioBackend::new(&mut self.runner, spec);
            if let Err(e) = be.apply_volume_mute_pct(media_mix, channel.muted) {
                eprintln!("apply_dial_mix: game volume apply error (ignoring): {e}");
            }
        }

        // Apply live to chat sink (no-op when absent).
        if let Some(channel) = chat_ch {
            let def = convert::channel_def_from_cfg(&channel);
            let spec = def.sink_spec();
            let mut be = AudioBackend::new(&mut self.runner, spec);
            if let Err(e) = be.apply_volume_mute_pct(chat_mix, channel.muted) {
                eprintln!("apply_dial_mix: chat volume apply error (ignoring): {e}");
            }
        }

        // Stamp so state() cache logic immediately returns updated values.
        self.last_volume_write = Some(std::time::Instant::now());
        self.emit(Event::ChatmixSet { position });
        Ok(())
    }

    /// Mirror the hardware base-station volume KNOB position into the app's master
    /// volume VALUE (read-only hardware mirror).
    ///
    /// This updates the active profile's in-memory `master_volume_pct` (clamped to
    /// 0..=100) and emits `MasterVolumeSet` so the GUI reflects the physical knob.
    /// SAFETY/SEMANTICS: the knob is itself the hardware gain — this does NOT apply
    /// any software gain (no `wpctl`) and does NOT persist to disk (no `save_config`);
    /// it is a transient value mirror, re-read from hardware on every poll. No-op-safe
    /// when there is no active profile.
    pub fn apply_hardware_master_volume(&mut self, pct: u8) -> Result<(), EngineError> {
        let pct = pct.min(100);
        let active_name = self.config.active_profile.clone();
        // Graceful no-op if the active profile is missing.
        let Some(profile) = self.config.profile_mut(&active_name) else {
            return Ok(());
        };
        profile.master_volume_pct = pct;
        // No volume-cache stamp: `state()` reads master_volume_pct straight from the
        // active profile (not from the pw-dump cache), so no `last_volume_write` stamp
        // is needed — and stamping would needlessly mask channel pw-dump reads.
        self.emit(Event::MasterVolumeSet { volume_pct: pct });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
