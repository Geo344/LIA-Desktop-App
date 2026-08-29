use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tauri::{Manager, State, WebviewWindow};

use dotenvy::dotenv;
use rand::seq::SliceRandom;
use std::io::{Read, Write};
use std::net::TcpListener;

// Native Sound Player
use rodio::{Decoder, OutputStream, OutputStreamHandle, Source};
use std::io::Cursor;

// Base64 encoding for album art
use base64::{engine::general_purpose::STANDARD, Engine as _};

// Native Windows Media Controls
#[cfg(target_os = "windows")]
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};

#[cfg(target_os = "windows")]
use windows::Storage::Streams::DataReader;

// Embed the sound effect directly into the compiled Rust binary
static SOUND_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/ConfirmSound.wav");

#[cfg(target_os = "windows")]
use windows::{
    core::{w, PCWSTR},
    Win32::Foundation::{BOOL, HWND, LPARAM, RECT, WPARAM},
    Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowExW, FindWindowW, GetSystemMetrics, GetWindowLongPtrW,
        SendMessageTimeoutW, SetParent, SetWindowLongPtrW, SetWindowPos, SystemParametersInfoW,
        GWL_STYLE, SMTO_NORMAL, SM_CXSCREEN, SPI_GETWORKAREA, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WS_CHILD,
        WS_POPUP, WS_VISIBLE, GetWindowTextW, SendMessageW, WM_CLOSE, SetForegroundWindow, 
        ShowWindow, SW_HIDE
    },
};

// Close any existing YouTube Music windows before launching a new one
#[cfg(target_os = "windows")]
unsafe extern "system" fn close_ytm_windows_proc(hwnd: HWND, _: LPARAM) -> BOOL {
    let mut text: [u16; 512] = [0; 512];
    let len = GetWindowTextW(hwnd, &mut text);
    let title = String::from_utf16_lossy(&text[..len as usize]);

    // Target the standalone app window, explicitly avoiding standard Chrome tabs
    if title.contains("YouTube Music") && !title.contains("Google Chrome") {
        // Send a native close request to the window
        let _ = SendMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    
    BOOL(1) // Continue searching just in case there are multiple
}

#[cfg(target_os = "windows")]
fn close_previous_ytm_session() {
    unsafe {
        let _ = EnumWindows(Some(close_ytm_windows_proc), LPARAM(0));
    }
}

// When opening new YTM window, send a simulated Spacebar keystroke 
// to trigger native Play/Pause, then hide the window
#[cfg(target_os = "windows")]
unsafe extern "system" fn play_and_hide_ytm_proc(hwnd: HWND, _: LPARAM) -> BOOL {
    let mut text: [u16; 512] = [0; 512];
    let len = GetWindowTextW(hwnd, &mut text);
    let title = String::from_utf16_lossy(&text[..len as usize]);

    // Ensure we only target the standalone app, not standard Chrome tabs
    if title.contains("YouTube Music") && !title.contains("Google Chrome") {
        use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP};
        const VK_SPACE: u8 = 0x20;

        // 1. Force the window to the front so it receives our keystroke
        let _ = SetForegroundWindow(hwnd);
        std::thread::sleep(std::time::Duration::from_millis(50));

        // 2. Press Spacebar to trigger native Play/Pause
        keybd_event(VK_SPACE, 0, windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_SPACE, 0, KEYEVENTF_KEYUP, 0);

        // 3. Give Chrome a tiny fraction of a second to register the interaction
        std::thread::sleep(std::time::Duration::from_millis(150));

        // 4. Forcefully strip the window from the screen and taskbar
        let _ = ShowWindow(hwnd, SW_HIDE);

        return BOOL(0); // Stop enumerating once found and executed
    }
    BOOL(1)
}

// Trigger the Play/Pause and hide sequence for any new YTM window
#[cfg(target_os = "windows")]
fn trigger_play_and_hide() {
    unsafe {
        let _ = EnumWindows(Some(play_and_hide_ytm_proc), LPARAM(0));
    }
}

// Google Integration
const REDIRECT_PORT: u16 = 8989;
const GOOGLE_SCOPES: &str = "https://www.googleapis.com/auth/youtube.readonly%20https://www.googleapis.com/auth/calendar.readonly";

#[derive(serde::Deserialize, serde::Serialize, Default)]
struct TokenStorage {
    refresh_token: Option<String>,
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(serde::Deserialize)]
struct PlaylistItemListResponse {
    items: Vec<PlaylistItem>,
}

#[derive(serde::Deserialize)]
struct PlaylistItem {
    #[serde(rename = "contentDetails")]
    content_details: Option<ContentDetails>,
}

#[derive(serde::Deserialize)]
struct ContentDetails {
    #[serde(rename = "videoId")]
    video_id: String,
}

// Desktop item representation for the frontend
#[derive(Serialize)]
pub struct DesktopItem {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

// Thread-safe state holding only the stream handle
pub struct AppAudioState {
    pub stream_handle: OutputStreamHandle,
}

#[derive(Serialize, Clone)]
pub struct MediaState {
    pub is_active: bool,
    pub title: String,
    pub artist: String,
    pub is_playing: bool,
    pub thumbnail_base64: Option<String>, // <--- Replaced progress_percent with thumbnail string
}

// Google Token storage and authentication functions
fn get_token_storage_path() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        PathBuf::from(appdata).join("LIA").join("tokens.json")
    } else {
        PathBuf::from("tokens.json")
    }
}

fn load_refresh_token() -> Option<String> {
    let path = get_token_storage_path();
    if let Ok(data) = fs::read_to_string(path) {
        if let Ok(storage) = serde_json::from_str::<TokenStorage>(&data) {
            return storage.refresh_token;
        }
    }
    None
}

fn save_refresh_token(token: &str) {
    let path = get_token_storage_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let storage = TokenStorage {
        refresh_token: Some(token.to_string()),
    };
    if let Ok(json) = serde_json::to_string_pretty(&storage) {
        let _ = fs::write(path, json);
    }
}

async fn get_access_token(client_id: &str, client_secret: &str) -> Result<String, String> {
    let client = reqwest::Client::new();

    // 1. If we have a stored refresh token, silently request a fresh access token
    if let Some(refresh_token) = load_refresh_token() {
        let params = [
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", &refresh_token),
            ("grant_type", "refresh_token"),
        ];

        let res = client
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if res.status().is_success() {
            let tokens: TokenResponse = res.json().await.map_err(|e| e.to_string())?;
            return Ok(tokens.access_token);
        }
    }

    // 2. Otherwise, run the one-time interactive browser login
    let redirect_uri = format!("http://127.0.0.1:{}/", REDIRECT_PORT);
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        client_id, redirect_uri, GOOGLE_SCOPES
    );

    // Open browser for one-time approval
    let _ = Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", &auth_url])
        .spawn();

    // Spin up lightweight local loopback listener to catch redirect
    let listener = TcpListener::bind(format!("127.0.0.1:{}", REDIRECT_PORT)).map_err(|e| e.to_string())?;
    let mut auth_code = String::new();

    if let Ok((mut stream, _)) = listener.accept() {
        let mut buffer = [0; 2048];
        let bytes_read = stream.read(&mut buffer).map_err(|e| e.to_string())?;
        let request = String::from_utf8_lossy(&buffer[..bytes_read]);

        if let Some(code_part) = request.split("code=").nth(1) {
            if let Some(code) = code_part.split('&').next().and_then(|s| s.split_whitespace().next()) {
                auth_code = code.to_string();
            }
        }

        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<h2>Login successful! You can close this window and return to your desktop.</h2>";
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }

    if auth_code.is_empty() {
        return Err("Failed to capture authorization code from Google login.".into());
    }

    // Exchange auth code for tokens
    let params = [
        ("code", auth_code.as_str()),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("redirect_uri", redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
    ];

    let token_res: TokenResponse = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    if let Some(ref_token) = token_res.refresh_token {
        save_refresh_token(&ref_token);
    }

    Ok(token_res.access_token)
}

// Scan user and public desktop folders
#[tauri::command]
fn get_desktop_items() -> Result<Vec<DesktopItem>, String> {
    let mut items = Vec::new();
    let mut directories = Vec::new();

    if let Ok(user_home) = std::env::var("USERPROFILE") {
        directories.push(PathBuf::from(user_home).join("Desktop"));
    }

    if let Ok(public_dir) = std::env::var("PUBLIC") {
        directories.push(PathBuf::from(public_dir).join("Desktop"));
    }

    for desktop_path in directories {
        if let Ok(entries) = fs::read_dir(desktop_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                let is_dir = path.is_dir();

                if !file_name.is_empty() && file_name != "desktop" {
                    items.push(DesktopItem {
                        name: file_name,
                        path: path.to_string_lossy().to_string(),
                        is_dir,
                    });
                }
            }
        }
    }
    Ok(items)
}

// Launch selected application or folder
#[tauri::command]
fn launch_item(path: String) -> Result<(), String> {
    Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

// Play ping sound effect for shortcuts
#[tauri::command]
fn play_ping(audio: State<'_, AppAudioState>) {
    let cursor = Cursor::new(SOUND_BYTES);
    if let Ok(source) = Decoder::new(cursor) {
        let _ = audio.stream_handle.play_raw(source.convert_samples());
    }
}

// Launch YouTube Music in a hidden Chrome window
#[tauri::command]
async fn launch_hidden_ytm() -> Result<(), String> {
    dotenv().ok();

    let client_id = std::env::var("GOOGL_CLIENT_ID")
        .map_err(|_| "GOOGL_CLIENT_ID missing in .env".to_string())?;
    let client_secret = std::env::var("GOOGL_CLIENT_SECRET")
        .map_err(|_| "GOOGL_CLIENT_SECRET missing in .env".to_string())?;

    let access_token = get_access_token(&client_id, &client_secret).await?;

    let playlist_id = "PL9aMbwJZsrPvwepEucfl_qjbJ7hepcXO1";
    let api_url = format!(
        "https://www.googleapis.com/youtube/v3/playlistItems?part=contentDetails&maxResults=50&playlistId={}",
        playlist_id
    );

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| e.to_string())?;

    let playlist_data: PlaylistItemListResponse = client
        .get(&api_url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let mut video_ids: Vec<String> = playlist_data
        .items
        .into_iter()
        .filter_map(|item| item.content_details.map(|cd| cd.video_id))
        .collect();

    if video_ids.is_empty() {
        return Err("No video items found in playlist.".into());
    }

    {
        let mut rng = rand::thread_rng();
        video_ids.shuffle(&mut rng);
    }

    #[cfg(target_os = "windows")]
    close_previous_ytm_session();
    std::thread::sleep(std::time::Duration::from_millis(50));

    let joined_ids = video_ids.join(",");
    let watch_url = format!("https://www.youtube.com/watch_videos?video_ids={}", joined_ids);

    let res = client.get(&watch_url)
        .header("Cookie", "CONSENT=YES+;")
        .send()
        .await
        .map_err(|e| e.to_string())?;
        
    let final_url = res.url().as_str();
    let mut ytm_url = final_url.replace("www.youtube.com", "music.youtube.com");
    if !ytm_url.contains("autoplay=1") {
        ytm_url = format!("{}&autoplay=1", ytm_url);
    }

    let escaped_url = ytm_url.replace("&", "^&");

    Command::new("cmd")
        .args([
            "/C",
            "start",
            "",
            "chrome",
            "--profile-directory=Default",
            &format!("--app={}", escaped_url),
            "--autoplay-policy=no-user-gesture-required",
            "--window-position=-9999,-9999",
            "--window-size=1,1",
        ])
        .spawn()
        .map_err(|e| e.to_string())?;

    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(3000));
        #[cfg(target_os = "windows")]
        trigger_play_and_hide();
    });

    Ok(())
}

// Fetch current media state from Windows SMTC
#[tauri::command]
async fn get_media_state() -> Result<MediaState, String> {
    #[cfg(target_os = "windows")]
    {
        // Define a fast fallback state
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

        // --- NEW: EXTRACT RAW THUMBNAIL IMAGE BYTES ---
        let mut thumbnail_base64 = None;
        if let Ok(thumbnail_ref) = props.Thumbnail() {
            if let Ok(stream_op) = thumbnail_ref.OpenReadAsync() {
                if let Ok(stream) = stream_op.get() {
                    if let Ok(size) = stream.Size() {
                        if size > 0 {
                            // Hook into the Windows DataReader API
                            if let Ok(reader) = DataReader::CreateDataReader(&stream) {
                                // Load the exact image size into memory
                                if let Ok(load_op) = reader.LoadAsync(size as u32) {
                                    if let Ok(_) = load_op.get() {
                                        // Read the bytes and securely encode them as Base64 for React
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

// Media Control Commands
#[tauri::command]
async fn media_play_pause() -> Result<(), String> {
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
async fn media_next() -> Result<(), String> {
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
async fn media_prev() -> Result<(), String> {
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

// Attach Window Handle to Windows WorkerW wallpaper layer
#[cfg(target_os = "windows")]
static mut WORKERW_HWND: HWND = HWND(std::ptr::null_mut());

#[cfg(target_os = "windows")]
unsafe extern "system" fn enum_windows_proc(hwnd: HWND, _: LPARAM) -> BOOL {
    if let Ok(shell_view) = FindWindowExW(hwnd, HWND::default(), w!("SHELLDLL_DefView"), PCWSTR::null()) {
        if !shell_view.0.is_null() {
            if let Ok(workerw) = FindWindowExW(HWND::default(), hwnd, w!("WorkerW"), PCWSTR::null()) {
                if !workerw.0.is_null() {
                    WORKERW_HWND = workerw;
                    return BOOL(0);
                }
            }
        }
    }
    BOOL(1)
}

#[cfg(target_os = "windows")]
fn attach_to_workerw(window: &WebviewWindow) {
    unsafe {
        let progman = match FindWindowW(w!("Progman"), PCWSTR::null()) {
            Ok(hwnd) => hwnd,
            Err(_) => return,
        };

        let mut result: usize = 0;
        let _ = SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0xD),
            LPARAM(0),
            SMTO_NORMAL,
            1000,
            Some(&mut result as *mut usize),
        );

        let _ = EnumWindows(Some(enum_windows_proc), LPARAM(0));

        let target_parent = if !WORKERW_HWND.0.is_null() {
            WORKERW_HWND
        } else {
            progman
        };

        if let Ok(tauri_hwnd) = window.hwnd() {
            let hwnd = HWND(tauri_hwnd.0 as *mut _);

            let mut work_area = RECT::default();
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut work_area as *mut _ as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );

            let screen_width = GetSystemMetrics(SM_CXSCREEN);
            let work_height = work_area.bottom - work_area.top;

            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            let new_style = (style & !(WS_POPUP.0 as isize)) | (WS_CHILD.0 as isize) | (WS_VISIBLE.0 as isize);
            let _ = SetWindowLongPtrW(hwnd, GWL_STYLE, new_style);

            let _ = SetParent(hwnd, target_parent);

            let _ = SetWindowPos(
                hwnd,
                HWND::default(),
                0,
                0,
                screen_width,
                work_height,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (stream, stream_handle) = OutputStream::try_default().expect("Failed to open audio device");
    
    // Keep the hardware output stream open for the entire process lifetime
    std::mem::forget(stream);

    let audio_state = AppAudioState { stream_handle };

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
            launch_hidden_ytm,
            get_media_state,
            media_play_pause,
            media_next,
            media_prev
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}