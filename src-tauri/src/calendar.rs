use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tauri::Emitter;
use chrono::{Local, Utc, TimeZone, NaiveTime, Duration};
use reqwest::Client;
use dotenvy::dotenv;
use crate::diagnostics::report_error;
use serde::de::DeserializeOwned;

const REDIRECT_PORT: u16 = 8989;
const GOOGLE_SCOPES: &str = "https://www.googleapis.com/auth/calendar.readonly";
const AUTH_CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
static AUTH_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Debug)]
struct GoogleApiError {
    code: Option<String>,
    message: String,
}

// Only recognized machine-readable codes are retained; never log response bodies
// or Google's free-form messages, which may contain private request details.
fn google_error_code(body: &serde_json::Value) -> Option<String> {
    let code = body.get("error").and_then(|error| error.as_str()
        .or_else(|| error.get("errors")?.get(0)?.get("reason")?.as_str()));
    code.filter(|code| matches!(*code,
        "invalid_grant" | "invalid_client" | "invalid_request" | "unauthorized_client"
        | "unsupported_grant_type" | "access_denied" | "invalid_scope"
        | "rateLimitExceeded" | "userRateLimitExceeded" | "quotaExceeded"
        | "dailyLimitExceeded" | "forbidden" | "notFound" | "authError"
        | "insufficientPermissions" | "backendError" | "badRequest"
        | "required" | "conditionNotMet" | "usageLimits"
    )).map(str::to_owned)
}

async fn read_google_response<T: DeserializeOwned>(
    response: reqwest::Response,
    context: &'static str,
) -> Result<T, GoogleApiError> {
    let status = response.status();
    if !status.is_success() {
        let code = response.json::<serde_json::Value>().await.ok()
            .as_ref().and_then(google_error_code);
        let detail = match &code {
            Some(code) => format!("HTTP {status} ({code})"),
            None => format!("HTTP {status}"),
        };
        return Err(GoogleApiError { code, message: report_error(context, detail) });
    }
    response.json().await.map_err(|error| GoogleApiError {
        code: None,
        message: report_error(context, error.without_url()),
    })
}

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
    #[serde(default)]
    items: Vec<GoogleEvent>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
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
    match fs::read_to_string(path) {
        Ok(data) => match serde_json::from_str::<TokenStorage>(&data) {
            Ok(storage) => return storage.refresh_token,
            Err(_) => { report_error("Load Google refresh token", "Token file is invalid JSON or has an invalid format"); }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => { report_error("Read Google token file", error); }
    }
    None
}

fn save_refresh_token(token: &str) {
    let path = get_token_storage_path();
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            report_error("Create Google token directory", error);
            return;
        }
    }
    let storage = TokenStorage {
        refresh_token: Some(token.to_string()),
    };
    match serde_json::to_string_pretty(&storage) {
        Ok(json) => if let Err(error) = fs::write(path, json) {
            report_error("Save Google refresh token", error);
        },
        Err(_) => { report_error("Serialize Google refresh token", "Could not serialize token storage"); }
    }
}

enum OAuthCallback {
    Ignore,
    Code(String),
    Denied,
}

// Read complete headers, including requests split over multiple TCP packets.
async fn handle_oauth_connection(mut stream: TcpStream) -> Result<OAuthCallback, String> {
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|part| part == b"\r\n\r\n") {
        let count = stream.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if count == 0 || request.len() + count > 8192 {
            return Ok(OAuthCallback::Ignore);
        }
        request.extend_from_slice(&buffer[..count]);
    }

    let request = String::from_utf8_lossy(&request);
    let mut parts = request.lines().next().unwrap_or_default().split_whitespace();
    let method = parts.next();
    let target = parts.next().unwrap_or_default();
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| "Invalid authentication callback.".to_string())?;
    let params = url.query_pairs();
    let code = params.clone().find(|(key, value)| key == "code" && !value.is_empty())
        .map(|(_, value)| value.into_owned());
    let denied = params.clone().any(|(key, _)| key == "error");
    let is_callback = method == Some("GET") && url.path() == "/";
    let (status, message) = if !is_callback || (code.is_none() && !denied) {
        ("404 Not Found", "Waiting for Google sign-in.")
    } else if denied {
        ("200 OK", "Google sign-in was cancelled or denied. You can close this window.")
    } else {
        ("200 OK", "Authorization received. You can close this window and return to LIA.")
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{message}",
        message.len()
    );
    // A closed browser tab must not discard an authorization code already received.
    match timeout(std::time::Duration::from_secs(1), stream.write_all(response.as_bytes())).await {
        Ok(Ok(())) => {},
        Ok(Err(error)) => { report_error("Write Google callback response", error); },
        Err(_) => { report_error("Write Google callback response", "Timed out"); },
    }
    if !is_callback {
        return Ok(OAuthCallback::Ignore);
    }
    if denied {
        return Ok(OAuthCallback::Denied);
    }
    Ok(code.map(OAuthCallback::Code).unwrap_or(OAuthCallback::Ignore))
}

async fn receive_oauth_callback(
    listener: TcpListener,
    deadline: std::time::Duration,
) -> Result<String, String> {
    // The deadline covers every accept/read/write, including unrelated requests.
    // Owning the listener here releases its port on success, error, or timeout.
    timeout(deadline, async {
        loop {
            let (stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
            // Do not let an idle connection monopolize the entire login window.
            match timeout(std::time::Duration::from_secs(5), handle_oauth_connection(stream)).await {
                Ok(Ok(OAuthCallback::Code(code))) => return Ok(code),
                Ok(Ok(OAuthCallback::Denied)) => return Err("Google sign-in was cancelled or denied. Please try again.".into()),
                Ok(Ok(OAuthCallback::Ignore)) => continue,
                Ok(Err(error)) => { report_error("Read Google callback", error); },
                Err(_) => { report_error("Read Google callback", "Connection timed out"); },
            }
        }
    }).await.map_err(|_| "Google sign-in timed out. Please try again.".to_string())?
}

async fn get_calendar_access_token(app: &tauri::AppHandle) -> Result<String, String> {
    let _auth_guard = AUTH_LOCK.try_lock()
        .map_err(|_| "Google Calendar authentication is already in progress.".to_string())?;
    dotenv().ok();
    let client_id = std::env::var("GOOGL_CLIENT_ID").map_err(|_| "Missing GOOGL_CLIENT_ID")?;
    let client_secret = std::env::var("GOOGL_CLIENT_SECRET").map_err(|_| "Missing GOOGL_CLIENT_SECRET")?;
    let client = Client::builder().timeout(std::time::Duration::from_secs(30))
        .build().map_err(|e| e.to_string())?;

    if let Some(refresh_token) = load_refresh_token() {
        let params = [
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];

        let res = client.post("https://oauth2.googleapis.com/token").form(&params).send().await
            .map_err(|e| report_error("Google token refresh request", e.without_url()))?;
        match read_google_response::<TokenResponse>(res, "Google token refresh response").await {
            Ok(tokens) => return Ok(tokens.access_token),
            // Reauthenticate only for a rejected refresh token. Server outages,
            // quota failures, and client configuration errors must not open login.
            Err(error) if error.code.as_deref() == Some("invalid_grant") => {},
            Err(error) => return Err(error.message),
        }
    }

    // Fallback to manual browser login if no refresh token exists
    let redirect_uri = format!("http://127.0.0.1:{}/", REDIRECT_PORT);
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        client_id, redirect_uri, GOOGLE_SCOPES
    );

    let listener = TcpListener::bind(("127.0.0.1", REDIRECT_PORT)).await
        .map_err(|e| format!("Could not start Google sign-in callback: {e}"))?;
    Command::new("rundll32").args(["url.dll,FileProtocolHandler", &auth_url]).spawn()
        .map_err(|e| format!("Could not open Google sign-in: {e}"))?;
    if let Err(error) = app.emit("calendar-auth-waiting", ()) {
        report_error("Notify calendar sign-in status", error);
    }
    let auth_code = receive_oauth_callback(listener, AUTH_CALLBACK_TIMEOUT).await?;

    let params = [
        ("code", auth_code.as_str()),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
    ];

    let response = client.post("https://oauth2.googleapis.com/token").form(&params).send().await
        .map_err(|e| report_error("Google authorization exchange request", e.without_url()))?;
    let token_res: TokenResponse = read_google_response(response, "Google authorization exchange response")
        .await.map_err(|error| error.message)?;
    
    if let Some(ref_token) = token_res.refresh_token {
        save_refresh_token(&ref_token);
    }

    Ok(token_res.access_token)
}

// --- CALENDAR API LOGIC ---

#[tauri::command]
pub async fn fetch_todays_events(app: tauri::AppHandle) -> Result<Vec<AgendaEvent>, String> {
    let access_token = get_calendar_access_token(&app).await
        .map_err(|error| report_error("Calendar authentication", error))?;
    let client = Client::builder().timeout(std::time::Duration::from_secs(30))
        .build().map_err(|e| report_error("Calendar HTTP client", e.without_url()))?;

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

    let mut events = Vec::new();
    let mut page_token: Option<String> = None;

    loop {
        // Keep the same time window and ordering for every page.
        let mut request = client.get(&api_url).bearer_auth(&access_token);
        if let Some(token) = &page_token {
            request = request.query(&[("pageToken", token)]);
        }

        let response = request
            .send()
            .await
            .map_err(|e| report_error("Google calendar events request", e.without_url()))?;
        let response: CalendarResponse = read_google_response(response, "Google calendar events response")
            .await.map_err(|error| error.message)?;

        events.extend(response.items);
        page_token = response.next_page_token.filter(|token| !token.is_empty());
        if page_token.is_none() {
            break;
        }
    }

    let mut agenda = Vec::new();
    let warning_threshold = now.with_timezone(&Utc) + Duration::minutes(10);

    for item in events {
        let title = item.summary.unwrap_or_else(|| "Busy".to_string());
        
        let mut starts_in_ten = false;
        let mut is_in_progress = false;
        let mut is_all_day = false;
        let mut start_time_str = String::new();
        let mut end_time_str = String::new();

        if let Some(start) = item.start {
            if let Some(dt_str) = start.date_time {
                if let Ok(parsed_start) = chrono::DateTime::parse_from_rfc3339(&dt_str)
                    .inspect_err(|_| { report_error("Parse Google event start", "Invalid RFC3339 timestamp"); }) {
                    let start_utc = parsed_start.with_timezone(&Utc);
                    let now_utc = now.with_timezone(&Utc);

                    // Check end time to see if event is already over or currently active
                    if let Some(end) = &item.end {
                        if let Some(end_dt_str) = &end.date_time {
                            if let Ok(parsed_end) = chrono::DateTime::parse_from_rfc3339(end_dt_str)
                                .inspect_err(|_| { report_error("Parse Google event end", "Invalid RFC3339 timestamp"); }) {
                                let end_utc = parsed_end.with_timezone(&Utc);
                                
                                // 1. Drop events that already ended
                                if end_utc <= now_utc {
                                    continue;
                                }

                                // 2. Mark event as in-progress
                                if start_utc <= now_utc && end_utc > now_utc {
                                    is_in_progress = true;
                                }

                                end_time_str = parsed_end.with_timezone(&Local).format("%I:%M %p").to_string();
                            }
                        }
                    }

                    // 3. Mark 10-minute upcoming warning
                    if !is_in_progress && start_utc > now_utc && start_utc <= warning_threshold {
                        starts_in_ten = true;
                    }

                    start_time_str = parsed_start.with_timezone(&Local).format("%I:%M %p").to_string();
                }
            } else if let Some(date_str) = start.date {
                is_all_day = true;
                start_time_str = date_str;
            }
        }

        agenda.push(AgendaEvent {
            title,
            start_time: start_time_str,
            end_time: end_time_str,
            starts_in_ten,
            is_in_progress,
            is_all_day,
        });
    }

    Ok(agenda)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;

    fn run_test(future: impl std::future::Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all().build().unwrap().block_on(future);
    }

    async fn mock_google_response(status: &str, body: &str) -> reqwest::Response {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            stream.read(&mut request).await.unwrap();
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let response = Client::builder().no_proxy().timeout(StdDuration::from_secs(2))
            .build().unwrap().get(format!("http://{address}/")).send().await.unwrap();
        server.await.unwrap();
        response
    }

    #[test]
    fn google_http_errors_preserve_safe_codes_and_exclude_private_details() {
        run_test(async {
            let response = mock_google_response("403 Forbidden", r#"{"error":{"message":"private-event-title secret-token","errors":[{"reason":"rateLimitExceeded"}]}}"#).await;
            let error = read_google_response::<CalendarResponse>(response, "Test calendar quota").await.unwrap_err();
            assert_eq!(error.code.as_deref(), Some("rateLimitExceeded"));
            assert!(error.message.contains("403"));
            assert!(!error.message.contains("private-event-title"));
            assert!(!error.message.contains("secret-token"));
        });
    }

    #[test]
    fn rejected_refresh_token_is_distinguished_from_server_failure() {
        run_test(async {
            let response = mock_google_response("400 Bad Request", r#"{"error":"invalid_grant","error_description":"secret-token"}"#).await;
            let error = read_google_response::<serde_json::Value>(response, "Test rejected refresh").await.unwrap_err();
            assert_eq!(error.code.as_deref(), Some("invalid_grant"));
            assert!(!error.message.contains("secret-token"));
            let response = mock_google_response("503 Service Unavailable", "<html>upstream unavailable</html>").await;
            let error = read_google_response::<CalendarResponse>(response, "Test server failure").await.unwrap_err();
            assert!(error.message.contains("503"));
            assert!(error.code.is_none());
        });
    }

    #[test]
    fn google_success_is_parsed_and_malformed_success_is_rejected() {
        run_test(async {
            let response = mock_google_response("200 OK", r#"{"items":[],"nextPageToken":"next"}"#).await;
            let page = read_google_response::<CalendarResponse>(response, "Test calendar success").await.unwrap();
            assert!(page.items.is_empty());
            assert_eq!(page.next_page_token.as_deref(), Some("next"));
            let response = mock_google_response("200 OK", "not JSON").await;
            assert!(read_google_response::<CalendarResponse>(response, "Test malformed success").await.is_err());
        });
    }

    #[test]
    fn unknown_google_error_codes_are_not_echoed() {
        let body = serde_json::json!({"error": "secret-token"});
        assert!(google_error_code(&body).is_none());
    }

    #[test]
    fn callback_timeout_releases_port_without_a_connection() {
        run_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let error = receive_oauth_callback(listener, StdDuration::from_millis(50)).await.unwrap_err();
            assert!(error.contains("timed out"));
            let _replacement = TcpListener::bind(address).await.unwrap();
        });
    }

    #[test]
    fn callback_deadline_covers_a_connected_but_silent_browser() {
        run_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let mut browser = TcpStream::connect(address).await.unwrap();
            let error = receive_oauth_callback(listener, StdDuration::from_millis(50)).await.unwrap_err();
            assert!(error.contains("timed out"));
            let mut byte = [0];
            assert_eq!(browser.read(&mut byte).await.unwrap(), 0);
            let _replacement = TcpListener::bind(address).await.unwrap();
        });
    }

    #[test]
    fn callback_ignores_unrelated_requests_and_decodes_fragmented_code() {
        run_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let browser = tokio::spawn(async move {
                let mut unrelated = TcpStream::connect(address).await.unwrap();
                unrelated.write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
                let mut response = Vec::new();
                unrelated.read_to_end(&mut response).await.unwrap();
                assert!(response.starts_with(b"HTTP/1.1 404"));

                let mut callback = TcpStream::connect(address).await.unwrap();
                callback.write_all(b"GET /?code=test%2B").await.unwrap();
                tokio::time::sleep(StdDuration::from_millis(10)).await;
                callback.write_all(b"code%2Fvalue&scope=calendar HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
                let mut response = Vec::new();
                callback.read_to_end(&mut response).await.unwrap();
                assert!(response.starts_with(b"HTTP/1.1 200"));
            });
            let code = receive_oauth_callback(listener, StdDuration::from_secs(2)).await.unwrap();
            assert_eq!(code, "test+code/value");
            browser.await.unwrap();
        });
    }

    #[test]
    fn callback_denial_returns_an_error_and_releases_port() {
        run_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let mut browser = TcpStream::connect(address).await.unwrap();
            browser.write_all(b"GET /?error=access_denied HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
            let error = receive_oauth_callback(listener, StdDuration::from_secs(2)).await.unwrap_err();
            assert!(error.contains("cancelled or denied"));
            let _replacement = TcpListener::bind(address).await.unwrap();
        });
    }
}
