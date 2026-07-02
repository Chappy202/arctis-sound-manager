mod commands;
mod daemon_control;
mod error;
mod meters;
mod state;
mod tray;

use state::{DaemonState, MeterSubscribers, WindowVisibility};
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;

/// Record main-window visibility for the deferred first show and the
/// hidden-to-tray poll-cadence gate (see state.rs). Called at every Rust-side
/// show/hide site so the flag can never drift from what we actually did.
pub(crate) fn mark_visible(app: &tauri::AppHandle, visible: bool) {
    app.state::<WindowVisibility>().0.store(visible, Ordering::Relaxed);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // `--hidden` (autostart-into-tray) starts without a window. Otherwise the
    // window is revealed by the frontend AFTER its first paint via the
    // `show_when_ready` command — never eagerly here, so a cold launch cannot
    // flash an unpainted (white) webview.
    let argv: Vec<String> = std::env::args().collect();
    let start_hidden = tray::should_start_hidden(&argv);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // A second instance launched: bring the existing window to the front.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
                mark_visible(app, true);
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .manage(Mutex::new(DaemonState::new()))
        .manage(MeterSubscribers::default())
        .manage(WindowVisibility::new(!start_hidden))
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::switch_profile,
            commands::set_eq_band,
            // Ack-only drag-tick variants (no EngineState over the bridge)
            commands::set_eq_band_ack,
            commands::set_channel_volume_ack,
            // Deferred first show (cold-launch white-flash fix)
            commands::show_when_ready,
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
            // GUI login autostart (Task 6)
            commands::gui_set_autostart,
            commands::gui_autostart_enabled,
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
                    // Hidden-to-tray cadence gate: 250 ms visible → ~3 s hidden.
                    let mut last_poll: Option<std::time::Instant> = None;
                    // Daemon liveness edge detector: emit `daemon-down` once
                    // when a request fails after previously succeeding.
                    let mut daemon_up = false;
                    loop {
                        ticker.tick().await;
                        let visible =
                            handle.state::<WindowVisibility>().0.load(Ordering::Relaxed);
                        if !state::should_poll(
                            last_poll.map(|t| t.elapsed().as_millis()),
                            visible,
                            3_000,
                        ) {
                            continue;
                        }
                        last_poll = Some(std::time::Instant::now());
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
                        match result {
                            Ok(Ok(resp)) if resp.ok => {
                                daemon_up = true;
                                if let Some(engine_state) = resp.state {
                                    if last_state.as_ref() != Some(&engine_state) {
                                        let _ = handle.emit("state-changed", &engine_state);
                                        tray::apply_view(&handle, &tray::tray_view(&engine_state));
                                        last_state = Some(engine_state);
                                    }
                                }
                            }
                            other => {
                                // Daemon went down mid-session: tell the UI once
                                // instead of silently serving stale state. Clearing
                                // last_state forces a `state-changed` re-emit on
                                // recovery even if the snapshot is unchanged, so
                                // the UI's disconnected status clears itself.
                                if daemon_up {
                                    daemon_up = false;
                                    last_state = None;
                                    let msg = match other {
                                        Ok(Ok(resp)) => resp
                                            .error
                                            .unwrap_or_else(|| "daemon returned an error".into()),
                                        Ok(Err(e)) => e.to_string(),
                                        Err(e) => format!("join error: {e}"),
                                    };
                                    let _ = handle.emit("daemon-down", msg);
                                }
                            }
                        }
                    }
                });
            }

            // ── Streams-poll task (every ~1.5 s) ────────────────────────────
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut ticker =
                        tokio::time::interval(std::time::Duration::from_millis(1500));
                    // Emit-on-change guard, mirroring the state poll: an idle
                    // stream list stops re-serialising AppStream[] every 1.5 s.
                    let mut last_streams: Option<Vec<arctis_engine::AppStream>> = None;
                    // Hidden-to-tray cadence gate: 1.5 s visible → ~10 s hidden.
                    let mut last_poll: Option<std::time::Instant> = None;
                    loop {
                        ticker.tick().await;
                        let visible =
                            handle.state::<WindowVisibility>().0.load(Ordering::Relaxed);
                        if !state::should_poll(
                            last_poll.map(|t| t.elapsed().as_millis()),
                            visible,
                            10_000,
                        ) {
                            continue;
                        }
                        last_poll = Some(std::time::Instant::now());
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
                                    if last_streams.as_ref() != Some(&streams) {
                                        let _ = handle.emit("streams-changed", &streams);
                                        last_streams = Some(streams);
                                    }
                                }
                            }
                        }
                    }
                });
            }

            // ── Real signal-peak meter task (~15 Hz) ────────────────────────
            // While subscribed, pw-record capture workers run for each Arctis
            // channel sink (via its .monitor port) and the clean-mic source.
            // PCM peaks are collected every ~66 ms and emitted as a `levels`
            // event with { node_name: 0.0..1.0 } real signal peak values.
            //
            // Lifecycle is tied to the UI subscriber count (meters::lifecycle):
            //   * 0 → 1: start the MeterTask (spawns supervisors + pw-record).
            //   * 1 → 0: drop it — Drop kills the children, so a hidden window
            //     or a meterless page runs ZERO capture processes.
            // Supervisors respawn exited children (~1 s bounded backoff), so
            // meters recover when the daemon/sinks come up after the GUI.
            //
            // Emit-on-change guard: skip the emit when no peak changed by more
            // than EMIT_EPSILON since the last tick (mirrors `state-changed`).
            // 15 Hz is plenty: the JS peakDecay envelope smooths the display.
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut task: Option<meters::MeterTask> = None;

                    // Emit at ~15 Hz (every 66 ms).
                    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(66));
                    let mut last_emitted: Option<meters::LevelsPayload> = None;
                    loop {
                        ticker.tick().await;

                        let subscribers = handle
                            .state::<MeterSubscribers>()
                            .0
                            .load(Ordering::Relaxed);
                        let current = match meters::lifecycle(subscribers, task.is_some()) {
                            meters::Lifecycle::Idle => continue,
                            meters::Lifecycle::Stop => {
                                task = None; // Drop kills all pw-record children
                                last_emitted = None;
                                continue;
                            }
                            meters::Lifecycle::Start => {
                                // start_meter_task spawns OS threads + pw-record
                                // children; run it on a blocking thread so we
                                // don't block the async executor.
                                match tauri::async_runtime::spawn_blocking(
                                    meters::start_meter_task,
                                )
                                .await
                                {
                                    Ok(new_task) => task.insert(new_task),
                                    Err(e) => {
                                        eprintln!("meters: start failed (will retry): {e}");
                                        continue;
                                    }
                                }
                            }
                            meters::Lifecycle::Run => match task.as_mut() {
                                Some(t) => t,
                                None => continue, // unreachable by construction
                            },
                        };

                        let payload = current.current_levels();
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

            // NOTE: no eager win.show() here. Showing before the webview's
            // first paint is exactly what flashed a white window on cold
            // launch — the frontend invokes `show_when_ready` after paint.

            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                // X hides to tray instead of quitting; the process stays resident.
                tauri::WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                    api.prevent_close();
                    let _ = window.hide();
                    mark_visible(window.app_handle(), false);
                }
                // Focus implies visible — catch-all for show paths that
                // bypass our helpers (e.g. a WM unhide).
                tauri::WindowEvent::Focused(true) if window.label() == "main" => {
                    mark_visible(window.app_handle(), true);
                }
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
