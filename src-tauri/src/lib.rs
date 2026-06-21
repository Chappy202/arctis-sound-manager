mod commands;
mod error;
mod meters;
mod state;

use state::DaemonState;
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Mutex::new(DaemonState::new()))
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::switch_profile,
            commands::set_eq_band,
            commands::set_route,
            commands::clear_route,
            commands::set_channel_output,
            commands::profile_new,
            commands::device_set,
            commands::mic_enable,
            commands::mic_stage,
            commands::mic_set,
            commands::mic_eq_band,
            commands::mic_hw_mic,
            commands::mic_suppression_backend,
            commands::set_channel_volume,
            commands::set_channel_mute,
            commands::surround_enable,
            commands::surround_set_hrir,
            commands::surround_set_channels,
            commands::surround_set_hw_sink,
            // F3b: profile management
            commands::profile_rename,
            commands::profile_delete,
            commands::profile_export,
            commands::profile_import,
            // F3b: EQ presets
            commands::eq_preset_save,
            commands::eq_preset_apply,
            commands::eq_preset_delete,
            // F4: Channel add / remove
            commands::channel_add,
            commands::channel_remove,
            // R2: Coexistence teardown
            commands::coexist_status,
            commands::coexist_disable,
        ])
        .setup(|app| {
            // ── State-poll task (every 2 s) ─────────────────────────────────
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
                    loop {
                        ticker.tick().await;
                        let socket = {
                            let st = handle.state::<Mutex<DaemonState>>();
                            let guard = st.lock().await;
                            guard.socket.clone()
                        };
                        let result = tauri::async_runtime::spawn_blocking(move || {
                            arctis_client::send_request_to(
                                &socket,
                                &arctis_client::Request::GetState,
                            )
                        })
                        .await;
                        if let Ok(Ok(resp)) = result {
                            if resp.ok {
                                if let Some(engine_state) = resp.state {
                                    let _ = handle.emit("state-changed", &engine_state);
                                }
                            }
                        }
                        // Daemon-down ticks are silently ignored;
                        // the UI keeps its last known good state.
                    }
                });
            }

            // ── Level-meter task (every 2 s) ─────────────────────────────────
            // Samples configured software volume for Arctis channel sinks + the
            // clean-mic source via `pw-dump`.  Emits a `levels` event carrying a
            // JSON object { node_name: 0.0..1.0 }.
            //
            // NOTE: This reflects the *configured volume scalar* (from PipeWire's
            // Props.channelVolumes), NOT a real-time audio signal peak or RMS.
            // True peak metering requires a native pipewire-rs capture stream
            // (follow-up task).  This is intentionally honest: no fake data.
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
                    loop {
                        ticker.tick().await;
                        let result =
                            tauri::async_runtime::spawn_blocking(meters::sample_levels).await;
                        match result {
                            Ok(Some(payload)) => {
                                let _ = handle.emit("levels", &payload);
                            }
                            Ok(None) => {
                                // pw-dump unavailable or no target nodes found — skip tick.
                            }
                            Err(_) => {
                                // spawn_blocking join error — skip tick, never crash.
                            }
                        }
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
