use serde::Serialize;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::sync::Mutex;

#[cfg(target_os = "windows")]
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};

#[cfg(target_os = "windows")]
use windows::Storage::Streams::DataReader;

#[cfg(target_os = "windows")]
use windows::{
    Win32::Foundation::{BOOL, HWND, LPARAM},
    Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW},
};

// Global cache to store the last known media state
static LAST_MEDIA_STATE: Mutex<Option<MediaState>> = Mutex::new(None);

#[derive(Serialize, Clone)]
pub struct MediaState {
    pub is_active: bool,
    pub title: String,
    pub artist: String,
    pub is_playing: bool,
    pub thumbnail_base64: Option<String>, 
}

// Hook to check if the YouTube Music window is currently open
#[cfg(target_os = "windows")]
unsafe extern "system" fn check_ytm_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut text: [u16; 512] = [0; 512];
    let len = GetWindowTextW(hwnd, &mut text);
    let title = String::from_utf16_lossy(&text[..len as usize]);

    if title.contains("YouTube Music") && !title.contains("Google Chrome") {
        let found_ptr = lparam.0 as *mut bool;
        if !found_ptr.is_null() {
            *found_ptr = true;
        }
        return BOOL(0); // Stop enumerating once found
    }
    
    BOOL(1) 
}

#[tauri::command]
pub async fn get_media_state() -> Result<MediaState, String> {
    #[cfg(target_os = "windows")]
    {
        let empty_state = MediaState { is_active: false, title: "".into(), artist: "".into(), is_playing: false, thumbnail_base64: None };

        let manager_res = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().and_then(|op| op.get());
        
        let mut current_state: Option<MediaState> = None;

        if let Ok(manager) = manager_res {
            if let Ok(session) = manager.GetCurrentSession() {
                if let Ok(props) = session.TryGetMediaPropertiesAsync().and_then(|op| op.get()) {
                    let title = props.Title().map(|s| s.to_string()).unwrap_or_default();
                    let artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();

                    let is_playing = session.GetPlaybackInfo()
                        .and_then(|info| info.PlaybackStatus())
                        .map(|status| status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
                        .unwrap_or(false);

                    let mut thumbnail_base64 = None;
                    if let Ok(thumbnail_ref) = props.Thumbnail() {
                        if let Ok(stream_op) = thumbnail_ref.OpenReadAsync() {
                            if let Ok(stream) = stream_op.get() {
                                if let Ok(size) = stream.Size() {
                                    if size > 0 {
                                        if let Ok(reader) = DataReader::CreateDataReader(&stream) {
                                            if let Ok(load_op) = reader.LoadAsync(size as u32) {
                                                if let Ok(_) = load_op.get() {
                                                    let mut buffer = vec![0u8; size as usize];
                                                    if let Ok(_) = reader.ReadBytes(&mut buffer) {
                                                        thumbnail_base64 = Some(STANDARD.encode(&buffer));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Only capture valid sessions with a title to avoid empty ghost states
                    if !title.is_empty() {
                        current_state = Some(MediaState {
                            is_active: true,
                            title,
                            artist,
                            is_playing,
                            thumbnail_base64,
                        });
                    }
                }
            }
        }

        // Check if YouTube Music is physically open on the OS
        let mut is_ytm_open = false;
        unsafe {
            let _ = EnumWindows(Some(check_ytm_window_proc), LPARAM(&mut is_ytm_open as *mut _ as isize));
        }

        if let Ok(mut cache) = LAST_MEDIA_STATE.lock() {
            if let Some(state) = current_state {
                // SMTC found a valid playing session. Update the cache and return it.
                *cache = Some(state.clone());
                return Ok(state);
            } else if is_ytm_open {
                // SMTC lost the session, but YouTube Music is still open. 
                // Return the last known cached state so the widget doesn't disappear.
                if let Some(mut cached_state) = cache.clone() {
                    cached_state.is_playing = false; // Accurately reflect that it's paused
                    return Ok(cached_state);
                } else {
                    // The app is open but we haven't captured any song data yet
                    return Ok(MediaState {
                        is_active: true,
                        title: "Waiting for music...".into(),
                        artist: "YouTube Music".into(),
                        is_playing: false,
                        thumbnail_base64: None,
                    });
                }
            } else {
                // Neither SMTC nor YTM is active. Clear the cache and hide the widget.
                *cache = None;
                return Ok(empty_state);
            }
        }

        Ok(empty_state)
    }

    #[cfg(not(target_os = "windows"))]
    Ok(MediaState { is_active: false, title: "".into(), artist: "".into(), is_playing: false, thumbnail_base64: None })
}

#[tauri::command]
pub async fn media_play_pause() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    if let Ok(op) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
        if let Ok(manager) = op.get() { 
            if let Ok(session) = manager.GetCurrentSession() {
                if let Ok(op2) = session.TryTogglePlayPauseAsync() {
                    let _ = op2.get(); 
                }
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn media_next() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    if let Ok(op) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
        if let Ok(manager) = op.get() { 
            if let Ok(session) = manager.GetCurrentSession() {
                if let Ok(op2) = session.TrySkipNextAsync() {
                    let _ = op2.get(); 
                }
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn media_prev() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    if let Ok(op) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
        if let Ok(manager) = op.get() { 
            if let Ok(session) = manager.GetCurrentSession() {
                if let Ok(op2) = session.TrySkipPreviousAsync() {
                    let _ = op2.get(); 
                }
            }
        }
    }
    Ok(())
}