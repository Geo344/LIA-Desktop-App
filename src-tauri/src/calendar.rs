use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::net::TcpListener;
use std::io::{Read, Write};
use chrono::{Local, Utc, TimeZone, NaiveTime, Duration};
use reqwest::Client;
use dotenvy::dotenv;

const REDIRECT_PORT: u16 = 8989;
const GOOGLE_SCOPES: &str = "https://www.googleapis.com/auth/calendar.readonly";

// --- DATA STRUCTURES ---

#[derive(Deserialize, Serialize, Default)]
struct TokenStorage {
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Deserialize, Debug)]
struct CalendarResponse {
    items: Vec<GoogleEvent>,
}

#[derive(Deserialize, Debug)]
struct GoogleEvent {
    summary: Option<String>,
    start: Option<EventTime>,
    end: Option<EventTime>,
}

#[derive(Deserialize, Debug)]
struct EventTime {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    date: Option<String>, // For all-day events
}

#[derive(Serialize, Clone)]
pub struct AgendaEvent {
    pub title: String,
    pub start_time: String,
    pub end_time: String,
    pub starts_in_ten: bool,
    pub is_in_progress: bool,
    pub is_all_day: bool,
}

// --- OAUTH TOKEN MANAGEMENT ---

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

async fn get_calendar_access_token() -> Result<String, String> {
    dotenv().ok();
    let client_id = std::env::var("GOOGL_CLIENT_ID").map_err(|_| "Missing GOOGL_CLIENT_ID")?;
    let client_secret = std::env::var("GOOGL_CLIENT_SECRET").map_err(|_| "Missing GOOGL_CLIENT_SECRET")?;
    let client = Client::new();

    if let Some(refresh_token) = load_refresh_token() {
        let params = [
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];

        let res = client.post("https://oauth2.googleapis.com/token").form(&params).send().await.map_err(|e| e.to_string())?;
        if res.status().is_success() {
            let tokens: TokenResponse = res.json().await.map_err(|e| e.to_string())?;
            return Ok(tokens.access_token);
        }
    }

    // Fallback to manual browser login if no refresh token exists
    let redirect_uri = format!("http://127.0.0.1:{}/", REDIRECT_PORT);
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        client_id, redirect_uri, GOOGLE_SCOPES
    );

    let _ = Command::new("rundll32").args(["url.dll,FileProtocolHandler", &auth_url]).spawn();
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
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<h2>Login successful! You can close this window.</h2>";
        let _ = stream.write_all(response.as_bytes());
    }

    if auth_code.is_empty() {
        return Err("Failed to capture authorization code.".into());
    }

    let params = [
        ("code", auth_code.as_str()),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
    ];

    let token_res: TokenResponse = client.post("https://oauth2.googleapis.com/token").form(&params).send().await.map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
    
    if let Some(ref_token) = token_res.refresh_token {
        save_refresh_token(&ref_token);
    }

    Ok(token_res.access_token)
}

// --- CALENDAR API LOGIC ---

#[tauri::command]
pub async fn fetch_todays_events() -> Result<Vec<AgendaEvent>, String> {
    let access_token = get_calendar_access_token().await?;
    let client = Client::new();

    // Calculate strict time boundaries for the current day
    let now = Local::now();
    let end_of_day = now.date_naive().and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap());
    let end_of_day_local = Local.from_local_datetime(&end_of_day).single().unwrap();

    // Google API requires RFC3339 format
    let time_min = now.to_rfc3339();
    let time_max = end_of_day_local.to_rfc3339();

    let api_url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/primary/events?timeMin={}&timeMax={}&singleEvents=true&orderBy=startTime",
        urlencoding::encode(&time_min),
        urlencoding::encode(&time_max)
    );

    let response: CalendarResponse = client
        .get(&api_url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let mut agenda = Vec::new();
    let warning_threshold = now.with_timezone(&Utc) + Duration::minutes(10);

    for item in response.items {
        let title = item.summary.unwrap_or_else(|| "Busy".to_string());
        
        let mut starts_in_ten = false;
        let mut is_all_day = false;
        let mut start_time_str = String::new();
        let mut end_time_str = String::new();

        if let Some(start) = item.start {
            if let Some(dt_str) = start.date_time {
                // Parse standard timed event
                if let Ok(parsed_start) = chrono::DateTime::parse_from_rfc3339(&dt_str) {
                    let start_utc = parsed_start.with_timezone(&Utc);
                    let now_utc = now.with_timezone(&Utc);
                    
                    // Trigger the warning flag if the event is strictly within the next 10 minutes
                    if start_utc > now_utc && start_utc <= warning_threshold {
                        starts_in_ten = true;
                    }

                    start_time_str = parsed_start.with_timezone(&Local).format("%I:%M %p").to_string();
                }
            } else if let Some(date_str) = start.date {
                // Handle All-Day events
                is_all_day = true;
                start_time_str = date_str;
            }
        }

        if let Some(end) = item.end {
            if let Some(dt_str) = end.date_time {
                if let Ok(parsed_end) = chrono::DateTime::parse_from_rfc3339(&dt_str) {
                    end_time_str = parsed_end.with_timezone(&Local).format("%I:%M %p").to_string();
                }
            }
        }

        agenda.push(AgendaEvent {
            title,
            start_time: start_time_str,
            end_time: end_time_str,
            starts_in_ten,
            is_all_day,
        });
    }

    Ok(agenda)
}