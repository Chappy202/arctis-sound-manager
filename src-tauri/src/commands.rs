use crate::error::CommandError;
use crate::state::DaemonState;
use arctis_client::{send_request_to, Request};
use arctis_engine::EngineState;
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
    volume_db: f32,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetChannelVolume { channel, volume_db }).await
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
