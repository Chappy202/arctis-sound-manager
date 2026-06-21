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

            // ── Real signal-peak meter task (~25 Hz) ────────────────────────
            // Spawns pw-record capture workers for each Arctis channel sink
            // (via its .monitor port) and the clean-mic source.  Collects
            // PCM peaks from the workers every 40 ms and emits a `levels`
            // event with { node_name: 0.0..1.0 } real signal peak values.
            //
            // Workers that fail (node absent, pw-record not found) hold their
            // last level at 0.0 — honest: no data = silence.  MeterTask Drop
            // kills all pw-record children cleanly.
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // start_meter_task spawns OS threads + pw-record children;
                    // run it on a blocking thread so we don't block the async
                    // executor.
                    let mut task = tauri::async_runtime::spawn_blocking(meters::start_meter_task)
                        .await
                        .expect("meter task spawn failed");

                    // Emit at ~25 Hz (every 40 ms).
                    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(40));
                    loop {
                        ticker.tick().await;
                        let payload = task.current_levels();
                        // Only emit if at least one node has reported a level.
                        // (When no Arctis nodes exist all entries are 0.0 from
                        // the watch channel default — still honest to emit.)
                        let _ = handle.emit("levels", &payload);
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
