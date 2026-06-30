mod commands;
mod daemon_control;
mod error;
mod meters;
mod state;
mod tray;

use state::{DaemonState, MeterSubscribers};
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Mutex::new(DaemonState::new()))
        .manage(MeterSubscribers::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::switch_profile,
            commands::set_eq_band,
            commands::set_channel_eq,
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
            commands::list_factory_profiles,
            commands::surround_set_blocksize,
            commands::surround_import_hrirs,
            commands::surround_fetch_hrirs,
            // A8: factory profile creation
            commands::profile_create_from_factory,
            // F3b: profile management
            commands::profile_rename,
            commands::profile_delete,
            commands::profile_export,
            commands::profile_import,
            // F3b: EQ presets
            commands::eq_preset_save,
            commands::eq_preset_apply,
            commands::eq_preset_delete,
            // eq-mic-preset-packs: mic preset apply
            commands::mic_preset_apply,
            // F4: Channel add / remove
            commands::channel_add,
            commands::channel_remove,
            // R2: Coexistence teardown
            commands::coexist_status,
            commands::coexist_disable,
            // Sonar mixer (Task 11)
            commands::list_streams,
            commands::list_outputs,
            commands::move_stream,
            commands::set_master_volume,
            commands::set_mic_volume,
            commands::set_master_mute,
            commands::set_chatmix,
            commands::set_default_sink_channel,
            // Level-meter subscriber gate (perf)
            commands::meter_subscribe,
            commands::meter_unsubscribe,
            // Daemon lifecycle (Task 5)
            commands::daemon_status,
            commands::daemon_start,
            commands::daemon_stop,
            commands::daemon_restart,
            commands::daemon_set_autostart,
        ])
        .setup(|app| {
            // Make the bundled daemon durable across AppImage updates.
            daemon_control::maybe_sync_bundled_daemon(
                &daemon_control::RealEnv,
                env!("CARGO_PKG_VERSION"),
            );

            // ── State-poll task (every 250 ms) ──────────────────────────────
            // Drives near-real-time GUI updates (volume, ChatMix dial position,
            // mute). The daemon is request-response (no event push), so the UI
            // polls; 250 ms feels live without spamming. engine.state()'s pw-dump
            // is TTL-cached (~1 s), so faster polling does NOT add subprocesses.
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut ticker =
                        tokio::time::interval(std::time::Duration::from_millis(250));
                    // Emit-on-change guard: only push `state-changed` when the
                    // snapshot actually differs from the last one we emitted.
                    // The poll stays at 250 ms for low latency, but an idle GUI
                    // does zero reactive work (EngineState derives PartialEq, and
                    // none of its fields are time-varying), so the EQ curve and
                    // every other $derived stop recomputing 4x/sec while idle.
                    let mut last_state: Option<arctis_engine::EngineState> = None;
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
                                    if last_state.as_ref() != Some(&engine_state) {
                                        let _ = handle.emit("state-changed", &engine_state);
                                        last_state = Some(engine_state);
                                    }
                                }
                            }
                        }
                        // Daemon-down ticks are silently ignored;
                        // the UI keeps its last known good state.
                    }
                });
            }

            // ── Streams-poll task (every ~1.5 s) ────────────────────────────
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut ticker =
                        tokio::time::interval(std::time::Duration::from_millis(1500));
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
                                &arctis_client::Request::ListStreams,
                            )
                        })
                        .await;
                        if let Ok(Ok(resp)) = result {
                            if resp.ok {
                                if let Some(streams) = resp.streams {
                                    let _ = handle.emit("streams-changed", &streams);
                                }
                            }
                        }
                    }
                });
            }

            // ── Real signal-peak meter task (~15 Hz) ────────────────────────
            // Spawns pw-record capture workers for each Arctis channel sink
            // (via its .monitor port) and the clean-mic source.  Collects
            // PCM peaks from the workers every ~66 ms and emits a `levels`
            // event with { node_name: 0.0..1.0 } real signal peak values.
            //
            // Two guards keep this off the scroll-compositing hot path:
            //   * Subscriber gate — when no LevelMeter is mounted the UI's
            //     subscriber count is 0, so we skip the emit entirely.
            //   * Emit-on-change — skip the emit when no peak changed by more
            //     than EMIT_EPSILON since the last tick (mirrors `state-changed`).
            // 15 Hz is plenty: the JS peakDecay envelope smooths the display.
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

                    // Emit at ~15 Hz (every 66 ms).
                    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(66));
                    let mut last_emitted: Option<meters::LevelsPayload> = None;
                    loop {
                        ticker.tick().await;

                        // Subscriber gate: if no LevelMeter is mounted, do no
                        // emit work at all. Drain the watch channels so a freshly
                        // mounted meter starts from current data, and reset the
                        // change-guard so the first post-subscribe tick emits.
                        let subscribers = handle
                            .state::<MeterSubscribers>()
                            .0
                            .load(std::sync::atomic::Ordering::Relaxed);
                        if subscribers == 0 {
                            let _ = task.current_levels();
                            last_emitted = None;
                            continue;
                        }

                        let payload = task.current_levels();
                        // Emit-on-change guard: skip when nothing audible changed.
                        if let Some(prev) = &last_emitted {
                            if meters::levels_unchanged(prev, &payload) {
                                continue;
                            }
                        }
                        let _ = handle.emit("levels", &payload);
                        last_emitted = Some(payload);
                    }
                });
            }

            // ── System tray ────────────────────────────────────────────────
            // Non-fatal: if the tray fails to build (e.g. no appindicator at
            // runtime) the app still runs as a normal window.
            match tray::build_tray(&app.handle().clone()) {
                Ok(handles) => {
                    tray::attach_menu_handlers(&handles.tray);
                    app.manage(handles);
                }
                Err(e) => eprintln!("tray: build failed (continuing without tray): {e}"),
            }

            // Temporary (Task 6 replaces with --hidden-aware logic): always show.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // X hides to tray instead of quitting; the process stays resident.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
