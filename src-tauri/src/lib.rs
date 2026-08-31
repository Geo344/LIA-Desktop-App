mod audio;
mod shortcuts;
mod media;
mod wallpaper;
mod calendar;

use tauri::Manager;
use audio::{play_ping, AppAudioState};
use shortcuts::{get_desktop_items, launch_item};
use media::{get_media_state, media_play_pause, media_next, media_prev};
use wallpaper::attach_to_workerw;
use calendar::fetch_todays_events;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let audio_state = AppAudioState::new();

    tauri::Builder::default()
        .manage(audio_state)
        .plugin(tauri_plugin_opener::init())
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
            fetch_todays_events
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}