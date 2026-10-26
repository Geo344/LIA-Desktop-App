use serde::Serialize;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::sync::Mutex;
use crate::diagnostics::report_error;

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

#[cfg(target_os = "windows")]
fn current_session() -> Result<Option<windows::Media::Control::GlobalSystemMediaTransportControlsSession>, String> {
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|op| op.get())
        .map_err(|error| report_error("Request Windows media manager", error))?;
    match manager.GetCurrentSession() {
        Ok(session) => Ok(Some(session)),
        // A null WinRT interface means there is no current session.
        Err(error) if error.code().0 == 0x80004003u32 as i32 => Ok(None),
        Err(error) => Err(report_error("Get current Windows media session", error)),
    }
}

#[cfg(target_os = "windows")]
fn read_thumbnail(props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties) -> Result<Option<String>, String> {
    let thumbnail = match props.Thumbnail() {
        Ok(thumbnail) => thumbnail,
        Err(error) if error.code().0 == 0x80004003u32 as i32 => return Ok(None),
        Err(error) => return Err(report_error("Get media thumbnail", error)),
    };
    let stream = thumbnail.OpenReadAsync().and_then(|op| op.get())
        .map_err(|error| report_error("Open media thumbnail", error))?;
    let size = stream.Size().map_err(|error| report_error("Get media thumbnail size", error))?;
    if size == 0 { return Ok(None); }
    // Bound allocation and avoid truncating the length passed to WinRT.
    if size > 16 * 1024 * 1024 {
        return Err(report_error("Read media thumbnail", "Artwork exceeds 16 MiB"));
    }
    let reader = DataReader::CreateDataReader(&stream)
        .map_err(|error| report_error("Create thumbnail reader", error))?;
    let loaded = reader.LoadAsync(size as u32).and_then(|op| op.get())
        .map_err(|error| report_error("Load media thumbnail", error))?;
    if loaded != size as u32 {
        return Err(report_error("Load media thumbnail", "Incomplete artwork data"));
    }
    let mut buffer = vec![0u8; size as usize];
    reader.ReadBytes(&mut buffer).map_err(|error| report_error("Read media thumbnail", error))?;
    Ok(Some(STANDARD.encode(buffer)))
}

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

        let mut current_state: Option<MediaState> = None;
        if let Some(session) = current_session()? {
            let props = session.TryGetMediaPropertiesAsync().and_then(|op| op.get())
                .map_err(|error| report_error("Get Windows media properties", error))?;
            let title = props.Title().map_err(|error| report_error("Get media title", error))?.to_string();
            let artist = props.Artist().map_err(|error| report_error("Get media artist", error))?.to_string();
            let is_playing = session.GetPlaybackInfo().and_then(|info| info.PlaybackStatus())
                .map_err(|error| report_error("Get media playback status", error))?
                == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
            // Artwork is optional: log failures but preserve usable playback data.
            let thumbnail_base64 = read_thumbnail(&props).unwrap_or(None);
            if !title.is_empty() {
                current_state = Some(MediaState {
                    is_active: true, title, artist, is_playing, thumbnail_base64,
                });
            }
        }

        // Check if YouTube Music is physically open on the OS
        let mut is_ytm_open = false;
        unsafe {
            let _ = EnumWindows(Some(check_ytm_window_proc), LPARAM(&mut is_ytm_open as *mut _ as isize));
        }

        {
            let mut cache = LAST_MEDIA_STATE.lock()
                .map_err(|_| report_error("Lock media cache", "Cache mutex is poisoned"))?;
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

    }

    #[cfg(not(target_os = "windows"))]
    Ok(MediaState { is_active: false, title: "".into(), artist: "".into(), is_playing: false, thumbnail_base64: None })
}

#[cfg(target_os = "windows")]
fn control_media(action: &'static str) -> Result<(), String> {
    let session = current_session()?.ok_or_else(|| report_error(action, "No active media session"))?;
    let result = match action {
        "Play/pause media" => session.TryTogglePlayPauseAsync(),
        "Skip to next media" => session.TrySkipNextAsync(),
        _ => session.TrySkipPreviousAsync(),
    }.and_then(|op| op.get()).map_err(|error| report_error(action, error))?;
    if !result {
        return Err(report_error(action, "Media application rejected the playback command"));
    }
    Ok(())
}

#[tauri::command]
pub async fn media_play_pause() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    { return control_media("Play/pause media"); }
    #[cfg(not(target_os = "windows"))]
    Ok(())
}

#[tauri::command]
pub async fn media_next() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    { return control_media("Skip to next media"); }
    #[cfg(not(target_os = "windows"))]
    Ok(())
}

#[tauri::command]
pub async fn media_prev() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    { return control_media("Skip to previous media"); }
    #[cfg(not(target_os = "windows"))]
    Ok(())
}
