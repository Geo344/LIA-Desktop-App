use serde::Serialize;
use base64::{engine::general_purpose::STANDARD, Engine as _};

#[cfg(target_os = "windows")]
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};

#[cfg(target_os = "windows")]
use windows::Storage::Streams::DataReader;

#[derive(Serialize, Clone)]
pub struct MediaState {
    pub is_active: bool,
    pub title: String,
    pub artist: String,
    pub is_playing: bool,
    pub thumbnail_base64: Option<String>, 
}

#[tauri::command]
pub async fn get_media_state() -> Result<MediaState, String> {
    #[cfg(target_os = "windows")]
    {
        let empty_state = MediaState { is_active: false, title: "".into(), artist: "".into(), is_playing: false, thumbnail_base64: None };

        let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync().and_then(|op| op.get()) {
            Ok(m) => m,
            Err(_) => return Ok(empty_state.clone()),
        };

        let session = match manager.GetCurrentSession() {
            Ok(s) => s,
            Err(_) => return Ok(empty_state.clone()),
        };

        let props = match session.TryGetMediaPropertiesAsync().and_then(|op| op.get()) {
            Ok(p) => p,
            Err(_) => return Ok(empty_state.clone()),
        };

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

        return Ok(MediaState {
            is_active: true,
            title,
            artist,
            is_playing,
            thumbnail_base64,
        });
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