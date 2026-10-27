mod audio;
mod shortcuts;
mod media;
mod wallpaper;
mod calendar;
mod notes;
mod diagnostics;
mod token_store;
mod single_instance;

use tauri::Manager;
use audio::{play_ping, AudioEngine};
use shortcuts::{get_desktop_items, launch_item};
use media::{get_media_state, media_play_pause, media_next, media_prev};
use wallpaper::attach_to_workerw;
use calendar::{fetch_todays_events, reconnect_google_calendar};
use notes::{load_user_data, save_user_data};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Keep this handle alive for the full event loop. Debug and release builds
    // share the same name so they cannot race over calendar login or user data.
    #[cfg(target_os = "windows")]
    let _instance = match single_instance::acquire() {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            diagnostics::report_info("LIA startup", "Another LIA instance is already running; exiting this launch");
            return;
        }
        Err(error) => {
            diagnostics::report_error("LIA startup", error);
            return;
        }
    };
    let audio_engine = AudioEngine::new();

    tauri::Builder::default()
        .manage(audio_engine)
        .setup(|app| {
            if let Some(main_window) = app.get_webview_window("main") {
                #[cfg(target_os = "windows")]
                attach_to_workerw(&main_window);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_desktop_items,
            launch_item,
            play_ping,
            get_media_state,
            media_play_pause,
            media_next,
            media_prev,
            fetch_todays_events,
            reconnect_google_calendar,
            load_user_data,
            save_user_data
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
