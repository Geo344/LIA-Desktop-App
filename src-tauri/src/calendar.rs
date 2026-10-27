use crate::diagnostics::report_error;
use crate::token_store::{self, StoredToken};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Duration, Local, NaiveTime, TimeZone, Utc};
use dotenvy::dotenv;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::process::Command;
use subtle::ConstantTimeEq;
use tauri::Emitter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::time::timeout;
use zeroize::{Zeroize, Zeroizing};

const GOOGLE_SCOPES: &str = "https://www.googleapis.com/auth/calendar.events.readonly";
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
    let code = body.get("error").and_then(|error| {
        error
            .as_str()
            .or_else(|| error.get("errors")?.get(0)?.get("reason")?.as_str())
    });
    code.filter(|code| {
        matches!(
            *code,
            "invalid_grant"
                | "invalid_client"
                | "invalid_request"
                | "unauthorized_client"
                | "unsupported_grant_type"
                | "access_denied"
                | "invalid_scope"
                | "rateLimitExceeded"
                | "userRateLimitExceeded"
                | "quotaExceeded"
                | "dailyLimitExceeded"
                | "forbidden"
                | "notFound"
                | "authError"
                | "insufficientPermissions"
                | "backendError"
                | "badRequest"
                | "required"
                | "conditionNotMet"
                | "usageLimits"
        )
    })
    .map(str::to_owned)
}

async fn read_google_response<T: DeserializeOwned>(
    response: reqwest::Response,
    context: &'static str,
) -> Result<T, GoogleApiError> {
    let status = response.status();
    if !status.is_success() {
        let code = response
            .json::<serde_json::Value>()
            .await
            .ok()
            .as_ref()
            .and_then(google_error_code);
        let detail = match &code {
            Some(code) => format!("HTTP {status} ({code})"),
            None => format!("HTTP {status}"),
        };
        return Err(GoogleApiError {
            code,
            message: report_error(context, detail),
        });
    }
    response.json().await.map_err(|error| GoogleApiError {
        code: None,
        message: report_error(context, error.without_url()),
    })
}

// --- DATA STRUCTURES ---

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    scope: Option<String>,
}

impl Drop for TokenResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(token) = &mut self.refresh_token {
            token.zeroize();
        }
    }
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

fn random_secret() -> Result<Zeroizing<String>, String> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(bytes.as_mut())
        .map_err(|_| report_error("Create OAuth secret", "System random generator failed"))?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_ref())))
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

struct OAuthAttempt {
    redirect_uri: reqwest::Url,
    state: Zeroizing<String>,
    verifier: Zeroizing<String>,
}

impl OAuthAttempt {
    fn new(address: std::net::SocketAddr) -> Result<Self, String> {
        Ok(Self {
            redirect_uri: reqwest::Url::parse(&format!("http://{address}/oauth/callback"))
                .map_err(|_| "Could not build OAuth callback URL")?,
            state: random_secret()?,
            verifier: random_secret()?,
        })
    }

    fn authorization_url(&self, client_id: &str) -> reqwest::Url {
        let mut url = reqwest::Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("client_id", client_id),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", GOOGLE_SCOPES),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("state", self.state.as_str()),
            ("code_challenge", pkce_challenge(&self.verifier).as_str()),
            ("code_challenge_method", "S256"),
        ]);
        url
    }
}

#[derive(Debug, PartialEq)]
enum OAuthCallback {
    Ignore,
    Code(String),
    Denied,
}

// Accept only one unambiguous response for this exact, single-use attempt.
// Reject malformed callbacks without terminating the real sign-in attempt.
fn parse_oauth_callback(request: &[u8], attempt: &OAuthAttempt) -> OAuthCallback {
    if request.len() > 8192 {
        return OAuthCallback::Ignore;
    }
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut parsed = httparse::Request::new(&mut headers);
    if !matches!(parsed.parse(request), Ok(httparse::Status::Complete(length)) if length == request.len())
    {
        return OAuthCallback::Ignore;
    }
    let target = parsed.path.unwrap_or_default();
    if parsed.method != Some("GET")
        || parsed.version != Some(1)
        || target.split('?').next() != Some(attempt.redirect_uri.path())
    {
        return OAuthCallback::Ignore;
    }
    let expected_host = format!("127.0.0.1:{}", attempt.redirect_uri.port().unwrap());
    let mut host_count = 0;
    for header in parsed.headers {
        let key = header.name;
        let Ok(value) = std::str::from_utf8(header.value) else {
            return OAuthCallback::Ignore;
        };
        if key.eq_ignore_ascii_case("host") {
            host_count += 1;
            if value.trim() != expected_host {
                return OAuthCallback::Ignore;
            }
        }
        if key.eq_ignore_ascii_case("transfer-encoding")
            || (key.eq_ignore_ascii_case("content-length") && value.trim() != "0")
        {
            return OAuthCallback::Ignore;
        }
    }
    if host_count != 1 {
        return OAuthCallback::Ignore;
    }
    let Ok(url) = attempt.redirect_uri.join(target) else {
        return OAuthCallback::Ignore;
    };
    if url.fragment().is_some() {
        return OAuthCallback::Ignore;
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    let states: Vec<_> = pairs.iter().filter(|(key, _)| key == "state").collect();
    let codes: Vec<_> = pairs.iter().filter(|(key, _)| key == "code").collect();
    let errors: Vec<_> = pairs.iter().filter(|(key, _)| key == "error").collect();
    if states.len() != 1 || !bool::from(states[0].1.as_bytes().ct_eq(attempt.state.as_bytes())) {
        return OAuthCallback::Ignore;
    }
    match (codes.as_slice(), errors.as_slice()) {
        ([code], []) if !code.1.is_empty() => OAuthCallback::Code(code.1.to_string()),
        ([], [error]) if !error.1.is_empty() => OAuthCallback::Denied,
        _ => OAuthCallback::Ignore,
    }
}

async fn handle_oauth_connection(
    mut stream: TcpStream,
    attempt: &OAuthAttempt,
) -> Result<OAuthCallback, String> {
    let mut request = Zeroizing::new(Vec::new());
    let mut buffer = Zeroizing::new([0; 1024]);
    while !request.windows(4).any(|part| part == b"\r\n\r\n") {
        let count = stream
            .read(buffer.as_mut())
            .await
            .map_err(|e| e.to_string())?;
        if count == 0 || request.len() + count > 8192 {
            return Ok(OAuthCallback::Ignore);
        }
        request.extend_from_slice(&buffer[..count]);
    }
    let callback = parse_oauth_callback(&request, attempt);
    let (status, message) = match &callback {
        OAuthCallback::Ignore => (
            "400 Bad Request",
            "This callback does not match the active sign-in attempt.",
        ),
        OAuthCallback::Denied => (
            "200 OK",
            "Google sign-in was cancelled or denied. You can close this window.",
        ),
        OAuthCallback::Code(_) => (
            "200 OK",
            "Authorization received. You can close this window and return to LIA.",
        ),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nPragma: no-cache\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n{message}", message.len()
    );
    match timeout(
        std::time::Duration::from_secs(1),
        stream.write_all(response.as_bytes()),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            report_error("Write Google callback response", error);
        }
        Err(_) => {
            report_error("Write Google callback response", "Timed out");
        }
    }
    Ok(callback)
}

async fn receive_oauth_callback(
    listener: TcpListener,
    attempt: &OAuthAttempt,
    deadline: std::time::Duration,
) -> Result<Zeroizing<String>, String> {
    timeout(deadline, async {
        loop {
            let (stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
            match timeout(
                std::time::Duration::from_secs(5),
                handle_oauth_connection(stream, attempt),
            )
            .await
            {
                Ok(Ok(OAuthCallback::Code(code))) => return Ok(Zeroizing::new(code)),
                Ok(Ok(OAuthCallback::Denied)) => {
                    return Err("Google sign-in was cancelled or denied. Please try again.".into())
                }
                Ok(Ok(OAuthCallback::Ignore)) => continue,
                Ok(Err(error)) => {
                    report_error("Read Google callback", error);
                }
                Err(_) => {
                    report_error("Read Google callback", "Connection timed out");
                }
            }
        }
    })
    .await
    .map_err(|_| "Google sign-in timed out. Please try again.".to_string())?
}

fn check_granted_scope(scope: &str) -> Result<(), String> {
    let scopes: Vec<_> = scope.split_whitespace().collect();
    if scopes.is_empty() || scopes.iter().any(|scope| *scope != GOOGLE_SCOPES) {
        return Err(report_error("Google Calendar permissions",
            "LIA needs a new event-only grant. Remove LIA's existing access in your Google Account's third-party connections, then click Retry and sign in again. Removing access affects other clients in the same Google Cloud project."));
    }
    Ok(())
}

fn google_client() -> Result<Client, String> {
    Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| report_error("Google HTTP client", e.without_url()))
}

async fn get_calendar_access_token(app: &tauri::AppHandle) -> Result<Zeroizing<String>, String> {
    let _auth_guard = AUTH_LOCK
        .try_lock()
        .map_err(|_| "Google Calendar authentication is already in progress.".to_string())?;
    dotenv().ok();
    let client_id = std::env::var("GOOGL_CLIENT_ID").map_err(|_| "Missing GOOGL_CLIENT_ID")?;
    let client_secret =
        std::env::var("GOOGL_CLIENT_SECRET").map_err(|_| "Missing GOOGL_CLIENT_SECRET")?;
    let client = google_client()?;

    if let Some(stored) = token_store::load()? {
        let params = [
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", stored.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];

        let res = client
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await
            .map_err(|e| report_error("Google token refresh request", e.without_url()))?;
        match read_google_response::<TokenResponse>(res, "Google token refresh response").await {
            Ok(mut tokens) => {
                let scope = tokens.scope.as_deref().unwrap_or(&stored.scope);
                check_granted_scope(scope)?;
                if tokens.access_token.is_empty() {
                    return Err("Google returned an empty access token".into());
                }
                if tokens.refresh_token.is_some() || scope != stored.scope {
                    token_store::save(&StoredToken {
                        refresh_token: tokens
                            .refresh_token
                            .as_deref()
                            .unwrap_or(&stored.refresh_token)
                            .to_owned(),
                        scope: scope.to_owned(),
                    })?;
                }
                return Ok(Zeroizing::new(std::mem::take(&mut tokens.access_token)));
            }
            // Reauthenticate only for a rejected refresh token. Server outages,
            // quota failures, and client configuration errors must not open login.
            Err(error) if error.code.as_deref() == Some("invalid_grant") => {
                token_store::clear()?;
            }
            Err(error) => return Err(error.message),
        }
    }

    // Bind first on an OS-assigned loopback port, before opening the browser.
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("Could not start Google sign-in callback: {e}"))?;
    let attempt = OAuthAttempt::new(listener.local_addr().map_err(|e| e.to_string())?)?;
    let auth_url = attempt.authorization_url(&client_id);
    let system_root = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("Windows system directory is unavailable")?;
    Command::new(system_root.join("System32").join("rundll32.exe"))
        .args(["url.dll,FileProtocolHandler", auth_url.as_str()])
        .spawn()
        .map_err(|e| format!("Could not open Google sign-in: {e}"))?;
    if let Err(error) = app.emit("calendar-auth-waiting", ()) {
        report_error("Notify calendar sign-in status", error);
    }
    let auth_code = receive_oauth_callback(listener, &attempt, AUTH_CALLBACK_TIMEOUT).await?;

    let params = [
        ("code", auth_code.as_str()),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("redirect_uri", attempt.redirect_uri.as_str()),
        ("code_verifier", attempt.verifier.as_str()),
        ("grant_type", "authorization_code"),
    ];

    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| report_error("Google authorization exchange request", e.without_url()))?;
    let mut token_res: TokenResponse =
        read_google_response(response, "Google authorization exchange response")
            .await
            .map_err(|error| error.message)?;

    check_granted_scope(token_res.scope.as_deref().unwrap_or_default())?;
    if token_res.access_token.is_empty() {
        return Err("Google returned an empty access token".into());
    }
    let refresh_token = token_res.refresh_token.as_ref().filter(|token| !token.is_empty())
        .ok_or("Google did not return an offline token. Remove LIA's access in your Google Account and retry.")?;
    token_store::save(&StoredToken {
        refresh_token: refresh_token.clone(),
        scope: GOOGLE_SCOPES.to_owned(),
    })?;
    Ok(Zeroizing::new(std::mem::take(&mut token_res.access_token)))
}

// --- CALENDAR API LOGIC ---

#[tauri::command]
pub async fn fetch_todays_events(app: tauri::AppHandle) -> Result<Vec<AgendaEvent>, String> {
    let access_token = get_calendar_access_token(&app)
        .await
        .map_err(|error| report_error("Calendar authentication", error))?;
    let client = google_client()?;

    // Calculate strict time boundaries for the current day
    let now = Local::now();
    let end_of_day = now
        .date_naive()
        .and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap());
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
        let mut request = client.get(&api_url).bearer_auth(access_token.as_str());
        if let Some(token) = &page_token {
            request = request.query(&[("pageToken", token)]);
        }

        let response = request
            .send()
            .await
            .map_err(|e| report_error("Google calendar events request", e.without_url()))?;
        let response: CalendarResponse =
            read_google_response(response, "Google calendar events response")
                .await
                .map_err(|error| error.message)?;

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
                if let Ok(parsed_start) =
                    chrono::DateTime::parse_from_rfc3339(&dt_str).inspect_err(|_| {
                        report_error("Parse Google event start", "Invalid RFC3339 timestamp");
                    })
                {
                    let start_utc = parsed_start.with_timezone(&Utc);
                    let now_utc = now.with_timezone(&Utc);

                    // Check end time to see if event is already over or currently active
                    if let Some(end) = &item.end {
                        if let Some(end_dt_str) = &end.date_time {
                            if let Ok(parsed_end) = chrono::DateTime::parse_from_rfc3339(end_dt_str)
                                .inspect_err(|_| {
                                    report_error(
                                        "Parse Google event end",
                                        "Invalid RFC3339 timestamp",
                                    );
                                })
                            {
                                let end_utc = parsed_end.with_timezone(&Utc);

                                // 1. Drop events that already ended
                                if end_utc <= now_utc {
                                    continue;
                                }

                                // 2. Mark event as in-progress
                                if start_utc <= now_utc && end_utc > now_utc {
                                    is_in_progress = true;
                                }

                                end_time_str = parsed_end
                                    .with_timezone(&Local)
                                    .format("%I:%M %p")
                                    .to_string();
                            }
                        }
                    }

                    // 3. Mark 10-minute upcoming warning
                    if !is_in_progress && start_utc > now_utc && start_utc <= warning_threshold {
                        starts_in_ten = true;
                    }

                    start_time_str = parsed_start
                        .with_timezone(&Local)
                        .format("%I:%M %p")
                        .to_string();
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
            .enable_all()
            .build()
            .unwrap()
            .block_on(future);
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
        let response = Client::builder()
            .no_proxy()
            .timeout(StdDuration::from_secs(2))
            .build()
            .unwrap()
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap();
        server.await.unwrap();
        response
    }

    #[test]
    fn google_http_errors_preserve_safe_codes_and_exclude_private_details() {
        run_test(async {
            let response = mock_google_response("403 Forbidden", r#"{"error":{"message":"private-event-title secret-token","errors":[{"reason":"rateLimitExceeded"}]}}"#).await;
            let error = read_google_response::<CalendarResponse>(response, "Test calendar quota")
                .await
                .unwrap_err();
            assert_eq!(error.code.as_deref(), Some("rateLimitExceeded"));
            assert!(error.message.contains("403"));
            assert!(!error.message.contains("private-event-title"));
            assert!(!error.message.contains("secret-token"));
        });
    }

    #[test]
    fn rejected_refresh_token_is_distinguished_from_server_failure() {
        run_test(async {
            let response = mock_google_response(
                "400 Bad Request",
                r#"{"error":"invalid_grant","error_description":"secret-token"}"#,
            )
            .await;
            let error =
                read_google_response::<serde_json::Value>(response, "Test rejected refresh")
                    .await
                    .unwrap_err();
            assert_eq!(error.code.as_deref(), Some("invalid_grant"));
            assert!(!error.message.contains("secret-token"));
            let response = mock_google_response(
                "503 Service Unavailable",
                "<html>upstream unavailable</html>",
            )
            .await;
            let error = read_google_response::<CalendarResponse>(response, "Test server failure")
                .await
                .unwrap_err();
            assert!(error.message.contains("503"));
            assert!(error.code.is_none());
        });
    }

    #[test]
    fn google_success_is_parsed_and_malformed_success_is_rejected() {
        run_test(async {
            let response =
                mock_google_response("200 OK", r#"{"items":[],"nextPageToken":"next"}"#).await;
            let page = read_google_response::<CalendarResponse>(response, "Test calendar success")
                .await
                .unwrap();
            assert!(page.items.is_empty());
            assert_eq!(page.next_page_token.as_deref(), Some("next"));
            let response = mock_google_response("200 OK", "not JSON").await;
            assert!(
                read_google_response::<CalendarResponse>(response, "Test malformed success")
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    fn unknown_google_error_codes_are_not_echoed() {
        let body = serde_json::json!({"error": "secret-token"});
        assert!(google_error_code(&body).is_none());
    }

    fn attempt_at(address: std::net::SocketAddr) -> OAuthAttempt {
        OAuthAttempt::new(address).unwrap()
    }

    fn callback_request(attempt: &OAuthAttempt, query: &str) -> String {
        format!(
            "GET /oauth/callback?{query} HTTP/1.1\r\nHost: {}:{}\r\n\r\n",
            attempt.redirect_uri.host_str().unwrap(),
            attempt.redirect_uri.port().unwrap()
        )
    }

    #[test]
    fn pkce_matches_rfc7636_vector_and_secrets_are_fresh() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let first = attempt_at("127.0.0.1:45000".parse().unwrap());
        let second = attempt_at("127.0.0.1:45000".parse().unwrap());
        assert_ne!(first.state.as_str(), second.state.as_str());
        assert_ne!(first.verifier.as_str(), second.verifier.as_str());
        assert_eq!(first.verifier.len(), 43);
        let url = first.authorization_url("client+id&other=value");
        let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(params["client_id"], "client+id&other=value");
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["code_challenge"], pkce_challenge(&first.verifier));
        assert_eq!(params["state"], first.state.as_str());
        assert_eq!(params["scope"], GOOGLE_SCOPES);
        assert!(!url.as_str().contains(first.verifier.as_str()));
    }

    #[test]
    fn callbacks_require_state_and_reject_ambiguous_fields() {
        let attempt = attempt_at("127.0.0.1:45000".parse().unwrap());
        for query in [
            "code=unexpected".to_string(),
            "code=unexpected&state=wrong".to_string(),
            "error=access_denied&state=wrong".to_string(),
            format!("state={}&code=one&code=two", attempt.state.as_str()),
            format!(
                "state={}&state={}&code=one",
                attempt.state.as_str(),
                attempt.state.as_str()
            ),
            format!(
                "state={}&code=one&error=access_denied",
                attempt.state.as_str()
            ),
            format!("state={}&error=one&error=two", attempt.state.as_str()),
            format!("state={}&code=", attempt.state.as_str()),
        ] {
            assert_eq!(
                parse_oauth_callback(callback_request(&attempt, &query).as_bytes(), &attempt),
                OAuthCallback::Ignore
            );
        }
        let valid = callback_request(
            &attempt,
            &format!("state={}&code=test%2Bcode%2Fvalue", attempt.state.as_str()),
        );
        assert_eq!(
            parse_oauth_callback(valid.as_bytes(), &attempt),
            OAuthCallback::Code("test+code/value".into())
        );
        let other_attempt = attempt_at("127.0.0.1:45000".parse().unwrap());
        assert_eq!(
            parse_oauth_callback(valid.as_bytes(), &other_attempt),
            OAuthCallback::Ignore
        );
    }

    #[test]
    fn callbacks_require_exact_host_path_and_get_without_body() {
        let attempt = attempt_at("127.0.0.1:45000".parse().unwrap());
        let valid = callback_request(
            &attempt,
            &format!("state={}&code=one", attempt.state.as_str()),
        );
        for invalid in [
            valid.replace("GET ", "POST "),
            valid.replace("/oauth/callback?", "/wrong?"),
            valid.replace("Host: 127.0.0.1:45000", "Host: evil.example"),
            valid.replace("Host: 127.0.0.1:45000", "Host: 127.0.0.1:45001"),
            valid.replace(
                "Host: 127.0.0.1:45000",
                "Host: 127.0.0.1:45000\r\nHost: 127.0.0.1:45000",
            ),
            valid.replace("Host: 127.0.0.1:45000\r\n", ""),
            valid.replace("Host:", "Host :"),
            valid.replace("\r\n\r\n", "\r\nContent-Length: 1\r\n\r\nx"),
            valid.replace("\r\n\r\n", "\r\nTransfer-Encoding: chunked\r\n\r\n"),
            valid.replace(" HTTP/1.1", "#fragment HTTP/1.1"),
            format!("{valid}{valid}"),
        ] {
            assert_eq!(
                parse_oauth_callback(invalid.as_bytes(), &attempt),
                OAuthCallback::Ignore
            );
        }
    }

    #[test]
    fn scope_check_rejects_old_or_unverified_grants() {
        assert!(check_granted_scope(GOOGLE_SCOPES).is_ok());
        for scope in [
            "",
            "https://www.googleapis.com/auth/calendar.readonly",
            "https://www.googleapis.com/auth/calendar",
        ] {
            assert!(check_granted_scope(scope).is_err());
        }
        assert!(check_granted_scope(&format!(
            "{GOOGLE_SCOPES} https://www.googleapis.com/auth/drive"
        ))
        .is_err());
    }

    #[test]
    fn callback_timeout_releases_port_without_a_connection() {
        run_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let attempt = attempt_at(address);
            let error = receive_oauth_callback(listener, &attempt, StdDuration::from_millis(50))
                .await
                .unwrap_err();
            assert!(error.contains("timed out"));
            let _replacement = TcpListener::bind(address).await.unwrap();
        });
    }

    #[test]
    fn callback_deadline_covers_a_connected_but_silent_browser() {
        run_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let attempt = attempt_at(address);
            let mut browser = TcpStream::connect(address).await.unwrap();
            let error = receive_oauth_callback(listener, &attempt, StdDuration::from_millis(50))
                .await
                .unwrap_err();
            assert!(error.contains("timed out"));
            let mut byte = [0];
            assert_eq!(browser.read(&mut byte).await.unwrap(), 0);
            let _replacement = TcpListener::bind(address).await.unwrap();
        });
    }

    #[test]
    fn invalid_callback_does_not_consume_attempt_and_fragmented_callback_works() {
        run_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let attempt = attempt_at(address);
            let invalid = callback_request(&attempt, "code=forged&state=wrong");
            let valid = callback_request(
                &attempt,
                &format!("code=test%2Bcode%2Fvalue&state={}", attempt.state.as_str()),
            );
            let browser = tokio::spawn(async move {
                let mut unrelated = TcpStream::connect(address).await.unwrap();
                unrelated.write_all(invalid.as_bytes()).await.unwrap();
                let mut response = Vec::new();
                unrelated.read_to_end(&mut response).await.unwrap();
                assert!(response.starts_with(b"HTTP/1.1 400"));
                let mut callback = TcpStream::connect(address).await.unwrap();
                callback.write_all(&valid.as_bytes()[..20]).await.unwrap();
                tokio::time::sleep(StdDuration::from_millis(10)).await;
                callback.write_all(&valid.as_bytes()[20..]).await.unwrap();
                let mut response = Vec::new();
                callback.read_to_end(&mut response).await.unwrap();
                assert!(response.starts_with(b"HTTP/1.1 200"));
                let response = String::from_utf8(response).unwrap();
                assert!(response.contains("Cache-Control: no-store"));
                assert!(!response.contains("test+code/value"));
            });
            let code = receive_oauth_callback(listener, &attempt, StdDuration::from_secs(2))
                .await
                .unwrap();
            assert_eq!(code.as_str(), "test+code/value");
            browser.await.unwrap();
        });
    }

    #[test]
    fn validated_callback_denial_returns_an_error_and_releases_port() {
        run_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let attempt = attempt_at(address);
            let mut browser = TcpStream::connect(address).await.unwrap();
            let request = callback_request(
                &attempt,
                &format!("error=access_denied&state={}", attempt.state.as_str()),
            );
            browser.write_all(request.as_bytes()).await.unwrap();
            let error = receive_oauth_callback(listener, &attempt, StdDuration::from_secs(2))
                .await
                .unwrap_err();
            assert!(error.contains("cancelled or denied"));
            let _replacement = TcpListener::bind(address).await.unwrap();
        });
    }
}
