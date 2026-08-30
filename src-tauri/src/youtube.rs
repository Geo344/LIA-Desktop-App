use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::io::{Read, Write};
use std::net::TcpListener;
use dotenvy::dotenv;
use rand::seq::SliceRandom;

#[cfg(target_os = "windows")]
use windows::{
    Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM},
    Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, SendMessageW, SetForegroundWindow, ShowWindow, SW_HIDE, WM_CLOSE,
    },
};

const REDIRECT_PORT: u16 = 8989;
const GOOGLE_SCOPES: &str = "https://www.googleapis.com/auth/youtube.readonly%20https://www.googleapis.com/auth/calendar.readonly";

// --- DATA STRUCTURES ---

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

// --- WINDOW MANAGEMENT HOOKS ---

// 1. Hook to aggressively close any existing YTM windows before we start a new one
#[cfg(target_os = "windows")]
unsafe extern "system" fn close_ytm_windows_proc(hwnd: HWND, _: LPARAM) -> BOOL {
    let mut text: [u16; 512] = [0; 512];
    let len = GetWindowTextW(hwnd, &mut text);
    let title = String::from_utf16_lossy(&text[..len as usize]);

    // Ensure we only kill the standalone PWA, not the user's active Chrome browsing session
    if title.contains("YouTube Music") && !title.contains("Google Chrome") {
        let _ = SendMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    
    BOOL(1) // Keep enumerating just in case there are multiple ghost windows
}

#[cfg(target_os = "windows")]
fn close_previous_ytm_session() {
    unsafe {
        let _ = EnumWindows(Some(close_ytm_windows_proc), LPARAM(0));
    }
}

// 2. Hook to find the newly spawned window, simulate a spacebar press, and hide it
#[cfg(target_os = "windows")]
unsafe extern "system" fn play_and_hide_ytm_proc(hwnd: HWND, _: LPARAM) -> BOOL {
    let mut text: [u16; 512] = [0; 512];
    let len = GetWindowTextW(hwnd, &mut text);
    let title = String::from_utf16_lossy(&text[..len as usize]);

    if title.contains("YouTube Music") && !title.contains("Google Chrome") {
        use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP};
        const VK_SPACE: u8 = 0x20;

        // Force the OS to give this specific window keyboard focus
        let _ = SetForegroundWindow(hwnd);
        
        // A microscopic buffer to let the OS window manager catch up before we press a key
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Simulate a physical Spacebar press to bypass YouTube's anti-autoplay protections
        keybd_event(VK_SPACE, 0, windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_SPACE, 0, KEYEVENTF_KEYUP, 0);

        // Give the Chromium web engine 150ms to register the keystroke
        std::thread::sleep(std::time::Duration::from_millis(150));

        // Strip the window from the display and taskbar entirely
        let _ = ShowWindow(hwnd, SW_HIDE);

        return BOOL(0); // Successfully executed, stop enumerating
    }
    BOOL(1)
}

#[cfg(target_os = "windows")]
fn trigger_play_and_hide() {
    unsafe {
        let _ = EnumWindows(Some(play_and_hide_ytm_proc), LPARAM(0));
    }
}

// --- GOOGLE OAUTH & TOKEN MANAGEMENT ---

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

// Silently fetches a new access token using the saved refresh token, 
// or spawns a browser for manual login if it's the user's first time
async fn get_access_token(client_id: &str, client_secret: &str) -> Result<String, String> {
    let client = reqwest::Client::new();

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

    let redirect_uri = format!("http://127.0.0.1:{}/", REDIRECT_PORT);
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        client_id, redirect_uri, GOOGLE_SCOPES
    );

    let _ = Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", &auth_url])
        .spawn();

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

// --- MAIN LAUNCH COMMAND ---

#[tauri::command]
pub async fn launch_hidden_ytm() -> Result<(), String> {
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

    // Provide a standard User-Agent so Google doesn't reject the API request
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

    // Randomize the order of the playlist natively in Rust before sending to YouTube
    {
        let mut rng = rand::thread_rng();
        video_ids.shuffle(&mut rng);
    }

    #[cfg(target_os = "windows")]
    close_previous_ytm_session();
    std::thread::sleep(std::time::Duration::from_millis(50));

    let joined_ids = video_ids.join(",");
    let watch_url = format!("https://www.youtube.com/watch_videos?video_ids={}", joined_ids);

    // A fast GET request to let YouTube convert our raw list of IDs into a unified session URL
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

    // Windows CMD requires ampersands to be escaped with a caret
    let escaped_url = ytm_url.replace("&", "^&");

    // Spawn the Chrome PWA out of sight
    Command::new("cmd")
        .args([
            "/C",
            "start",
            "",
            "chrome",
            "--profile-directory=Default",
            &format!("--app={}", escaped_url),
            "--autoplay-policy=no-user-gesture-required",
            "--window-position=-9999,-9999", // Pin it way off-screen initially
            "--window-size=1,1",
        ])
        .spawn()
        .map_err(|e| e.to_string())?;

    // --- THE ORIGINAL SLEEP MECHANIC ---
    // Park a native Rust background thread for exactly 3 seconds. 
    // This gives the Chrome app plenty of time to spawn, download the DOM, and prepare the player.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(3000));
        
        // Once awake, fire the spacebar into the loaded window and hide it instantly.
        #[cfg(target_os = "windows")]
        trigger_play_and_hide();
    });

    Ok(())
}