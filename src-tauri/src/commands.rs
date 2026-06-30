use crate::error::CommandError;
use crate::state::{DaemonState, MeterSubscribers};
use arctis_client::{send_request_to, Request};
use arctis_engine::{
    AppStream, EngineState, EqBandSnapshot, FactoryProfileInfo, OutputDeviceSnapshot,
};
use std::sync::atomic::Ordering;
use tauri::State;
use tokio::sync::Mutex;

// Re-export for serde derives (CoexistReport + CoexistDisableResult must be serializable via Tauri).
// They are defined in arctis_client::protocol and already derive Serialize+Deserialize.

/// Internal helper: lock state to get socket path, run the blocking send on a
/// threadpool thread (so we never block the async executor), then interpret the
/// daemon's `Response`.
async fn call(
    state: &State<'_, Mutex<DaemonState>>,
    req: Request,
) -> Result<EngineState, CommandError> {
    let socket = state.lock().await.socket.clone();
    let resp = tauri::async_runtime::spawn_blocking(move || send_request_to(&socket, &req))
        .await
        .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))??;
    if resp.ok {
        resp.state
            .ok_or_else(|| CommandError::Daemon("ok response missing state".into()))
    } else {
        Err(CommandError::Daemon(
            resp.error.unwrap_or_else(|| "unknown daemon error".into()),
        ))
    }
}

/// Variant of `call` for verbs that return a text payload (e.g. ProfileExport).
/// Extracts `resp.text` instead of `resp.state`.
async fn call_text(
    state: &State<'_, Mutex<DaemonState>>,
    req: Request,
) -> Result<String, CommandError> {
    let socket = state.lock().await.socket.clone();
    let resp = tauri::async_runtime::spawn_blocking(move || send_request_to(&socket, &req))
        .await
        .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))??;
    if resp.ok {
        resp.text
            .ok_or_else(|| CommandError::Daemon("ok response missing text payload".into()))
    } else {
        Err(CommandError::Daemon(
            resp.error.unwrap_or_else(|| "unknown daemon error".into()),
        ))
    }
}

/// Variant of `call` for ListStreams (returns the `streams` payload).
async fn call_streams(
    state: &State<'_, Mutex<DaemonState>>,
    req: Request,
) -> Result<Vec<AppStream>, CommandError> {
    let socket = state.lock().await.socket.clone();
    let resp = tauri::async_runtime::spawn_blocking(move || send_request_to(&socket, &req))
        .await
        .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))??;
    if resp.ok {
        Ok(resp.streams.unwrap_or_default())
    } else {
        Err(CommandError::Daemon(
            resp.error.unwrap_or_else(|| "unknown daemon error".into()),
        ))
    }
}

/// Variant of `call` for ListFactoryProfiles (returns the `factory_profiles` payload).
async fn call_factory_profiles(
    state: &State<'_, Mutex<DaemonState>>,
    req: Request,
) -> Result<Vec<FactoryProfileInfo>, CommandError> {
    let socket = state.lock().await.socket.clone();
    let resp = tauri::async_runtime::spawn_blocking(move || send_request_to(&socket, &req))
        .await
        .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))??;
    if resp.ok {
        Ok(resp.factory_profiles.unwrap_or_default())
    } else {
        Err(CommandError::Daemon(
            resp.error.unwrap_or_else(|| "unknown daemon error".into()),
        ))
    }
}

/// Variant of `call` for ListOutputs (returns the `output_devices` payload).
async fn call_outputs(
    state: &State<'_, Mutex<DaemonState>>,
    req: Request,
) -> Result<Vec<OutputDeviceSnapshot>, CommandError> {
    let socket = state.lock().await.socket.clone();
    let resp = tauri::async_runtime::spawn_blocking(move || send_request_to(&socket, &req))
        .await
        .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))??;
    if resp.ok {
        Ok(resp.output_devices.unwrap_or_default())
    } else {
        Err(CommandError::Daemon(
            resp.error.unwrap_or_else(|| "unknown daemon error".into()),
        ))
    }
}

#[tauri::command]
pub async fn get_state(state: State<'_, Mutex<DaemonState>>) -> Result<EngineState, CommandError> {
    call(&state, Request::GetState).await
}

#[tauri::command]
pub async fn switch_profile(
    name: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SwitchProfile { name }).await
}

#[tauri::command]
pub async fn set_eq_band(
    channel: String,
    band: usize,
    kind: String,
    freq_hz: f32,
    q: f32,
    gain_db: f32,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(
        &state,
        Request::SetEqBand {
            channel,
            band,
            kind,
            freq_hz,
            q,
            gain_db,
        },
    )
    .await
}

#[tauri::command]
pub async fn set_channel_eq(
    channel: String,
    bands: Vec<EqBandSnapshot>,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetChannelEq { channel, bands }).await
}

#[tauri::command]
pub async fn set_route(
    app_binary: String,
    target_sink: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(
        &state,
        Request::Route {
            app_binary,
            target_sink,
        },
    )
    .await
}

#[tauri::command]
pub async fn clear_route(
    app_binary: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::RouteClear { app_binary }).await
}

#[tauri::command]
pub async fn set_channel_output(
    channel: String,
    device: Option<String>,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetChannelOutput { channel, device }).await
}

#[tauri::command]
pub async fn profile_new(
    name: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::ProfileNew { name }).await
}

#[tauri::command]
pub async fn device_set(
    control: String,
    value: i64,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::DeviceSet { control, value }).await
}

#[tauri::command]
pub async fn mic_enable(
    enabled: bool,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::MicEnable { enabled }).await
}

#[tauri::command]
pub async fn mic_stage(
    stage: String,
    enabled: bool,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::MicStage { stage, enabled }).await
}

#[tauri::command]
pub async fn mic_set(
    param: String,
    value: f32,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::MicSet { param, value }).await
}

#[tauri::command]
pub async fn mic_eq_band(
    band: usize,
    kind: String,
    freq_hz: f32,
    q: f32,
    gain_db: f32,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(
        &state,
        Request::MicEqBand {
            band,
            kind,
            freq_hz,
            q,
            gain_db,
        },
    )
    .await
}

#[tauri::command]
pub async fn mic_hw_mic(
    device: Option<String>,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::MicHwMic { device }).await
}

#[tauri::command]
pub async fn mic_suppression_backend(
    backend: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::MicSuppressionBackend { backend }).await
}

// ── F2.2: Per-channel volume / mute commands ─────────────────────────────────

#[tauri::command]
pub async fn set_channel_volume(
    channel: String,
    volume_pct: u8,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetChannelVolume { channel, volume_pct }).await
}

#[tauri::command]
pub async fn set_channel_mute(
    channel: String,
    muted: bool,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetChannelMute { channel, muted }).await
}

// ── F1.5: Surround / HRIR commands ──────────────────────────────────────────

#[tauri::command]
pub async fn surround_enable(
    enabled: bool,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SurroundEnable { enabled }).await
}

#[tauri::command]
pub async fn surround_set_hrir(
    name: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SurroundSetHrir { name }).await
}

#[tauri::command]
pub async fn surround_set_channels(
    channels: Vec<String>,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SurroundSetChannels { channels }).await
}

#[tauri::command]
pub async fn surround_set_hw_sink(
    hw_sink: Option<String>,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SurroundSetHwSink { hw_sink }).await
}

#[tauri::command]
pub async fn list_factory_profiles(
    state: State<'_, Mutex<DaemonState>>,
) -> Result<Vec<FactoryProfileInfo>, CommandError> {
    call_factory_profiles(&state, Request::ListFactoryProfiles).await
}

#[tauri::command]
pub async fn surround_set_blocksize(
    blocksize: Option<u32>,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SurroundSetBlocksize { blocksize }).await
}

// ── A6: HRIR import / fetch commands ─────────────────────────────────────────

#[tauri::command]
pub async fn surround_import_hrirs(
    dir: Option<String>,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SurroundImportHrirs { dir }).await
}

#[tauri::command]
pub async fn surround_fetch_hrirs(
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SurroundFetchHrirs).await
}

// ── A8: Factory profile creation ─────────────────────────────────────────────

#[tauri::command]
pub async fn profile_create_from_factory(
    template: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::ProfileCreateFromFactory { template }).await
}

// ── F3b: Profile management commands ─────────────────────────────────────────

#[tauri::command]
pub async fn profile_rename(
    old: String,
    new: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::ProfileRename { old, new }).await
}

#[tauri::command]
pub async fn profile_delete(
    name: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::ProfileDelete { name }).await
}

/// Export a profile as TOML text. Returns the raw TOML string, NOT an EngineState.
#[tauri::command]
pub async fn profile_export(
    name: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<String, CommandError> {
    call_text(&state, Request::ProfileExport { name }).await
}

#[tauri::command]
pub async fn profile_import(
    toml: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::ProfileImport { toml }).await
}

// ── R2: Coexistence teardown commands ────────────────────────────────────────

#[tauri::command]
pub async fn coexist_status(
    state: State<'_, Mutex<DaemonState>>,
) -> Result<arctis_client::CoexistReport, CommandError> {
    let socket = state.lock().await.socket.clone();
    let resp = tauri::async_runtime::spawn_blocking(move || {
        send_request_to(&socket, &Request::CoexistStatus)
    })
    .await
    .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))??;
    if resp.ok {
        resp.coexist_report
            .ok_or_else(|| CommandError::Daemon("ok response missing coexist_report".into()))
    } else {
        Err(CommandError::Daemon(
            resp.error.unwrap_or_else(|| "unknown daemon error".into()),
        ))
    }
}

#[tauri::command]
pub async fn coexist_disable(
    dry_run: bool,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<arctis_client::CoexistDisableResult, CommandError> {
    let socket = state.lock().await.socket.clone();
    let resp = tauri::async_runtime::spawn_blocking(move || {
        send_request_to(&socket, &Request::CoexistDisable { dry_run })
    })
    .await
    .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))??;
    if resp.ok {
        resp.coexist_result
            .ok_or_else(|| CommandError::Daemon("ok response missing coexist_result".into()))
    } else {
        Err(CommandError::Daemon(
            resp.error.unwrap_or_else(|| "unknown daemon error".into()),
        ))
    }
}

// ── F4: Channel add / remove commands ────────────────────────────────────────

#[tauri::command]
pub async fn channel_add(
    id: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::ChannelAdd { id }).await
}

#[tauri::command]
pub async fn channel_remove(
    id: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::ChannelRemove { id }).await
}

// ── Sonar mixer commands (Task 11) ───────────────────────────────────────────

#[tauri::command]
pub async fn list_streams(
    state: State<'_, Mutex<DaemonState>>,
) -> Result<Vec<AppStream>, CommandError> {
    call_streams(&state, Request::ListStreams).await
}

#[tauri::command]
pub async fn list_outputs(
    state: State<'_, Mutex<DaemonState>>,
) -> Result<Vec<OutputDeviceSnapshot>, CommandError> {
    call_outputs(&state, Request::ListOutputs).await
}

#[tauri::command]
pub async fn move_stream(
    stream: String,
    channel: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::MoveStream { stream, channel }).await
}

#[tauri::command]
pub async fn set_master_volume(
    volume_pct: u8,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetMasterVolume { volume_pct }).await
}

#[tauri::command]
pub async fn set_mic_volume(
    volume_pct: u8,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetMicVolume { volume_pct }).await
}

#[tauri::command]
pub async fn set_master_mute(
    muted: bool,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetMasterMute { muted }).await
}

#[tauri::command]
pub async fn set_chatmix(
    position: i64,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetChatmix { position }).await
}

#[tauri::command]
pub async fn set_default_sink_channel(
    channel: Option<String>,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetDefaultSinkChannel { channel }).await
}

// ── F3b: EQ preset commands ───────────────────────────────────────────────────

#[tauri::command]
pub async fn eq_preset_save(
    name: String,
    channel: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::EqPresetSave { name, channel }).await
}

#[tauri::command]
pub async fn eq_preset_apply(
    preset: String,
    channel: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::EqPresetApply { preset, channel }).await
}

#[tauri::command]
pub async fn eq_preset_delete(
    name: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::EqPresetDelete { name }).await
}

// ── Task 5 (eq-mic-preset-packs): mic preset apply command ───────────────────

#[tauri::command]
pub async fn mic_preset_apply(
    name: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::ApplyMicPreset { name }).await
}

// ── Daemon lifecycle commands ─────────────────────────────────────────────────
//
// These commands do NOT use DaemonState/socket IPC — the daemon may not be
// running yet.  They build a fresh RealEnv + resolve paths on each call.

use crate::daemon_control::{self as dc, DaemonStatus};

#[tauri::command]
pub async fn daemon_status() -> Result<DaemonStatus, CommandError> {
    tauri::async_runtime::spawn_blocking(|| {
        let env = dc::RealEnv;
        let socket = arctis_client::socket_path();
        let home = dc::home_dir();
        let binary = dc::resolve_binary(&dc::candidate_binaries(), &|p| p.exists());
        Ok(dc::query_status(&env, &socket, binary, &home))
    })
    .await
    .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))?
}

#[tauri::command]
pub async fn daemon_start() -> Result<DaemonStatus, CommandError> {
    tauri::async_runtime::spawn_blocking(|| {
        let env = dc::RealEnv;
        let socket = arctis_client::socket_path();
        let home = dc::home_dir();
        let binary = dc::resolve_binary(&dc::candidate_binaries(), &|p| p.exists());
        if !dc::use_systemd(&env, &home) {
            let b = binary.clone().ok_or_else(|| {
                CommandError::Daemon("asm-cli binary not found; set $ASM_CLI_BIN".into())
            })?;
            dc::start(&env, &socket, &b, &home).map_err(CommandError::Daemon)?;
        } else {
            dc::start(&env, &socket, std::path::Path::new("/unused"), &home)
                .map_err(CommandError::Daemon)?;
        }
        Ok(dc::query_status(&env, &socket, binary, &home))
    })
    .await
    .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))?
}

#[tauri::command]
pub async fn daemon_stop() -> Result<DaemonStatus, CommandError> {
    tauri::async_runtime::spawn_blocking(|| {
        let env = dc::RealEnv;
        let socket = arctis_client::socket_path();
        let home = dc::home_dir();
        let binary = dc::resolve_binary(&dc::candidate_binaries(), &|p| p.exists());
        dc::stop(&env, &socket, &home).map_err(CommandError::Daemon)?;
        Ok(dc::query_status(&env, &socket, binary, &home))
    })
    .await
    .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))?
}

#[tauri::command]
pub async fn daemon_restart() -> Result<DaemonStatus, CommandError> {
    tauri::async_runtime::spawn_blocking(|| {
        let env = dc::RealEnv;
        let socket = arctis_client::socket_path();
        let home = dc::home_dir();
        let binary = dc::resolve_binary(&dc::candidate_binaries(), &|p| p.exists());
        if !dc::use_systemd(&env, &home) {
            let b = binary.clone().ok_or_else(|| {
                CommandError::Daemon("asm-cli binary not found; set $ASM_CLI_BIN".into())
            })?;
            dc::restart(&env, &socket, &b, &home).map_err(CommandError::Daemon)?;
        } else {
            dc::restart(&env, &socket, std::path::Path::new("/unused"), &home)
                .map_err(CommandError::Daemon)?;
        }
        Ok(dc::query_status(&env, &socket, binary, &home))
    })
    .await
    .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))?
}

#[tauri::command]
pub async fn daemon_set_autostart(enabled: bool) -> Result<DaemonStatus, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let env = dc::RealEnv;
        let socket = arctis_client::socket_path();
        let home = dc::home_dir();
        let binary = dc::resolve_binary(&dc::candidate_binaries(), &|p| p.exists());
        if enabled {
            let b = binary.clone().ok_or_else(|| {
                CommandError::Daemon("asm-cli binary not found; set $ASM_CLI_BIN".into())
            })?;
            dc::install_autostart(&env, &b, &home).map_err(CommandError::Daemon)?;
        } else {
            dc::disable_autostart(&env, &home).map_err(CommandError::Daemon)?;
        }
        Ok(dc::query_status(&env, &socket, binary, &home))
    })
    .await
    .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))?
}

// ── Level-meter subscriber gate (perf) ───────────────────────────────────────
//
// The UI calls `meter_subscribe` when a LevelMeter mounts and `meter_unsubscribe`
// when it unmounts. The meter task skips emitting the `levels` event whenever the
// count is 0, so pages with no meters do zero meter-dispatch work.

#[tauri::command]
pub fn meter_subscribe(meters: State<'_, MeterSubscribers>) {
    meters.0.fetch_add(1, Ordering::Relaxed);
}

#[tauri::command]
pub fn meter_unsubscribe(meters: State<'_, MeterSubscribers>) {
    // Saturating decrement — never wrap below 0 if unsubscribe ever races ahead.
    let _ = meters
        .0
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            Some(n.saturating_sub(1))
        });
}
