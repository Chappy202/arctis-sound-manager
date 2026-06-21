mod commands;
mod error;
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
            commands::set_channel_output,
            commands::profile_new,
            commands::device_set,
            commands::mic_enable,
            commands::mic_stage,
            commands::mic_set,
            commands::mic_eq_band,
            commands::mic_hw_mic,
            commands::mic_suppression_backend,
            commands::surround_enable,
            commands::surround_set_hrir,
            commands::surround_set_channels,
            commands::surround_set_hw_sink,
        ])
        .setup(|app| {
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
                        arctis_client::send_request_to(&socket, &arctis_client::Request::GetState)
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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
