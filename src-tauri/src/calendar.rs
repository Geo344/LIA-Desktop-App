use crate::diagnostics::{report_error, report_info};
use crate::token_store::{self, StoredToken};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Duration, Local, NaiveTime, TimeZone, Utc};
use dotenvy::dotenv;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use subtle::ConstantTimeEq;
use tauri::Emitter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::time::timeout;
use zeroize::{Zeroize, Zeroizing};

const EVENT_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events.readonly";
const CALENDAR_LIST_SCOPE: &str = "https://www.googleapis.com/auth/calendar.calendarlist.readonly";
const GOOGLE_SCOPES: &str = concat!(
    "https://www.googleapis.com/auth/calendar.events.readonly ",
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly"
);
const AUTH_CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
// Web clients require the exact registered port, path, and trailing slash.
const GOOGLE_REDIRECT_URI: &str = "http://127.0.0.1:8989/";
static AUTH_LOCK: Mutex<()> = Mutex::const_new(());
static AUTH_ATTEMPTS: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct GoogleApiError {
    status: Option<reqwest::StatusCode>,
    code: Option<String>,
    message: String,
}

impl GoogleApiError {
    fn is_rejected_refresh_token(&self) -> bool {
        self.status == Some(reqwest::StatusCode::BAD_REQUEST)
            && self.code.as_deref() == Some("invalid_grant")
    }
}

#[derive(Clone, Copy, PartialEq)]
enum GoogleResponseKind {
    Standard,
    TokenRefresh,
    TokenRevocation,
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
                | "invalid_token"
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
    read_google_response_for(response, context, GoogleResponseKind::Standard).await
}

async fn read_google_response_for<T: DeserializeOwned>(
    response: reqwest::Response,
    context: &'static str,
    kind: GoogleResponseKind,
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
        let error = GoogleApiError {
            status: Some(status),
            code,
            message: format!("{context}: {detail}"),
        };
        if kind == GoogleResponseKind::TokenRefresh && error.is_rejected_refresh_token() {
            report_info(
                context,
                "Saved Google authorization expired or was revoked; continuing to browser sign-in.",
            );
        } else if kind == GoogleResponseKind::TokenRevocation
            && error.status == Some(reqwest::StatusCode::BAD_REQUEST)
            && error.code.as_deref() == Some("invalid_token")
        {
            report_info(
                context,
                "Saved Google authorization is already invalid; continuing to a new sign-in.",
            );
        } else {
            report_error(context, detail);
        }
        return Err(error);
    }
    response.json().await.map_err(|error| GoogleApiError {
        status: Some(status),
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

#[derive(Deserialize)]
struct CalendarListResponse {
    #[serde(default)]
    items: Vec<CalendarEntry>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct CalendarEntry {
    id: String,
    summary: Option<String>,
    #[serde(rename = "summaryOverride")]
    summary_override: Option<String>,
    #[serde(rename = "accessRole")]
    access_role: String,
    #[serde(default)]
    deleted: bool,
}

impl CalendarEntry {
    fn can_read_events(&self) -> bool {
        !self.deleted
            && !self.id.is_empty()
            && matches!(
                self.access_role.as_str(),
                "reader" | "writer" | "writerWithoutPrivateAccess" | "owner"
            )
    }

    fn display_name(&self) -> &str {
        self.summary_override
            .as_deref()
            .filter(|name| !name.is_empty())
            .or(self.summary.as_deref().filter(|name| !name.is_empty()))
            .unwrap_or("Calendar")
    }
}

#[derive(Deserialize, Debug)]
struct GoogleEvent {
    id: Option<String>,
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
    pub calendar_id: String,
    pub calendar_name: String,
    pub event_id: String,
    pub title: String,
    pub start_time: String,
    pub end_time: String,
    pub starts_in_ten: bool,
    pub is_in_progress: bool,
    pub is_all_day: bool,
}

#[derive(Serialize)]
pub struct CalendarAgenda {
    pub events: Vec<AgendaEvent>,
    pub unavailable_calendars: Vec<String>,
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
    // A non-secret log identifier, unrelated to OAuth state or PKCE material.
    log_id: String,
    redirect_uri: reqwest::Url,
    state: Zeroizing<String>,
    verifier: Zeroizing<String>,
}

impl OAuthAttempt {
    fn new(address: std::net::SocketAddr) -> Result<Self, String> {
        Ok(Self {
            log_id: format!(
                "{}-{}",
                std::process::id(),
                AUTH_ATTEMPTS.fetch_add(1, Ordering::Relaxed)
            ),
            redirect_uri: reqwest::Url::parse(&format!("http://{address}/"))
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
    let mut connections = 0;
    let result = timeout(deadline, async {
        loop {
            let (stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
            connections += 1;
            match timeout(
                std::time::Duration::from_secs(5),
                handle_oauth_connection(stream, attempt),
            )
            .await
            {
                Ok(Ok(OAuthCallback::Code(code))) => {
                    report_info(
                        "Google sign-in",
                        format!(
                            "Attempt {}: callback validated; exchanging authorization code",
                            attempt.log_id
                        ),
                    );
                    return Ok(Zeroizing::new(code));
                }
                Ok(Ok(OAuthCallback::Denied)) => {
                    return Err("Google sign-in was cancelled or denied. Please try again.".into())
                }
                Ok(Ok(OAuthCallback::Ignore)) => {
                    report_info(
                        "Google sign-in",
                        format!(
                            "Attempt {}: ignored a callback that did not match this sign-in",
                            attempt.log_id
                        ),
                    );
                    continue;
                }
                Ok(Err(error)) => {
                    report_error("Read Google callback", error);
                }
                Err(_) => {
                    report_error("Read Google callback", "Connection timed out");
                }
            }
        }
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            report_info(
                "Google sign-in",
                format!(
                    "Attempt {}: timed out after {connections} loopback connections",
                    attempt.log_id
                ),
            );
            if connections == 0 {
                Err("Google sign-in timed out before the browser returned to LIA. Check Google's error page and confirm that the Web client's authorized redirect URI is http://127.0.0.1:8989/ including the trailing slash, then retry using the newest sign-in tab.".into())
            } else {
                Err("Google sign-in timed out without a valid callback. Retry and finish sign-in in the newest browser tab.".into())
            }
        }
    }
}

fn check_allowed_scopes(scope: &str) -> Result<(), String> {
    let scopes: Vec<_> = scope.split_whitespace().collect();
    let issue = if scopes.is_empty() {
        Some("Google did not provide verifiable calendar permissions.")
    } else if scopes.iter().any(|scope| {
        matches!(
            *scope,
            "https://www.googleapis.com/auth/calendar.readonly"
                | "https://www.googleapis.com/auth/calendar"
        )
    }) {
        Some("Google still reports the old, broader calendar permission for LIA.")
    } else if scopes
        .iter()
        .any(|scope| ![EVENT_SCOPE, CALENDAR_LIST_SCOPE].contains(scope))
    {
        Some("Google returned additional permissions outside those LIA requests.")
    } else {
        None
    };
    match issue {
        // The command boundary logs this once; validation must not duplicate it.
        Some(issue) => Err(format!(
            "Google Calendar permissions: {issue} Choose Reconnect Google below to reset this authorization and sign in again, or remove LIA in your Google Account's third-party connections and click Retry. Retry alone reuses the existing grant. Removing access affects other clients in the same Google Cloud project."
        )),
        None => Ok(()),
    }
}

fn has_required_scopes(scope: &str) -> bool {
    [EVENT_SCOPE, CALENDAR_LIST_SCOPE]
        .iter()
        .all(|required| scope.split_whitespace().any(|granted| granted == *required))
}

fn check_granted_scope(scope: &str) -> Result<(), String> {
    check_allowed_scopes(scope)?;
    if !has_required_scopes(scope) {
        return Err(
            "Grant LIA access to both calendar events and your calendar list, then retry.".into(),
        );
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

fn web_callback_address() -> std::net::SocketAddr {
    let uri = reqwest::Url::parse(GOOGLE_REDIRECT_URI).expect("Fixed Google redirect URI");
    std::net::SocketAddr::new(
        uri.host_str().unwrap().parse().expect("Fixed loopback IP"),
        uri.port().expect("Fixed Web client port"),
    )
}

async fn bind_oauth_listener(address: std::net::SocketAddr) -> Result<TcpListener, String> {
    TcpListener::bind(address).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::AddrInUse {
            report_error("Google callback listener", "The registered callback port is already in use. Close another LIA instance or the program using port 8989, then retry. The Web client cannot fall back to a random port.")
        } else {
            report_error("Google callback listener", error)
        }
    })
}

fn authorization_code_parameters<'a>(
    attempt: &'a OAuthAttempt,
    client_id: &'a str,
    client_secret: &'a str,
    code: &'a str,
) -> [(&'static str, &'a str); 6] {
    [
        ("code", code),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("redirect_uri", attempt.redirect_uri.as_str()),
        ("code_verifier", attempt.verifier.as_str()),
        ("grant_type", "authorization_code"),
    ]
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
        match read_google_response_for::<TokenResponse>(
            res,
            "Google token refresh response",
            GoogleResponseKind::TokenRefresh,
        )
        .await
        {
            Ok(mut tokens) => {
                report_info(
                    "Google Calendar authentication",
                    "Checking permissions from the saved refresh token",
                );
                let scope = tokens.scope.as_deref().unwrap_or(&stored.scope);
                if let Err(error) = check_allowed_scopes(scope) {
                    notify_reconnect_required(app);
                    return Err(error);
                }
                if tokens.access_token.is_empty() {
                    return Err("Google returned an empty access token".into());
                }
                if has_required_scopes(scope) {
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
                // An older event-only token needs consent for calendar-list
                // discovery. Keep it encrypted until the replacement is saved.
            }
            // Reauthenticate only for a rejected refresh token. Server outages,
            // quota failures, and client configuration errors must not open login.
            Err(error) if error.is_rejected_refresh_token() => {
                token_store::clear()?;
            }
            Err(error) => return Err(error.message),
        }
    }

    // Bind the registered Web callback before opening the browser. Never change
    // ports on a conflict: Google would reject an unregistered redirect URI.
    let listener = bind_oauth_listener(web_callback_address()).await?;
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
    report_info(
        "Google sign-in",
        format!(
            "Attempt {}: browser launched; waiting for a Google redirect for up to {} seconds",
            attempt.log_id,
            AUTH_CALLBACK_TIMEOUT.as_secs()
        ),
    );
    if let Err(error) = app.emit("calendar-auth-waiting", ()) {
        report_error("Notify calendar sign-in status", error);
    }
    let auth_code = receive_oauth_callback(listener, &attempt, AUTH_CALLBACK_TIMEOUT).await?;

    let params = authorization_code_parameters(&attempt, &client_id, &client_secret, &auth_code);

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

    report_info(
        "Google Calendar authentication",
        "Browser sign-in completed; checking permissions from the new token",
    );
    let scope = token_res.scope.as_deref().unwrap_or_default();
    if let Err(error) = check_allowed_scopes(scope) {
        notify_reconnect_required(app);
        return Err(error);
    }
    check_granted_scope(scope)?;
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

fn notify_reconnect_required(app: &tauri::AppHandle) {
    if let Err(error) = app.emit("calendar-reconnect-required", ()) {
        report_error("Notify Google reconnection status", error);
    }
}

async fn revoke_google_authorization(
    client: &Client,
    endpoint: &reqwest::Url,
    refresh_token: &str,
) -> Result<(), String> {
    let response = client
        .post(endpoint.clone())
        .form(&[("token", refresh_token)])
        .send()
        .await
        .map_err(|error| report_error("Revoke Google authorization", error.without_url()))?;
    // Successful revocation has an empty response body, not a JSON token object.
    if response.status().is_success() {
        return Ok(());
    }
    match read_google_response_for::<serde_json::Value>(
        response,
        "Revoke Google authorization",
        GoogleResponseKind::TokenRevocation,
    )
    .await
    {
        // An already invalid token needs no further revocation.
        Err(error)
            if error.status == Some(reqwest::StatusCode::BAD_REQUEST)
                && error.code.as_deref() == Some("invalid_token") =>
        {
            Ok(())
        }
        Err(error) => Err(error.message),
        Ok(_) => Err("Google did not confirm revocation. Please retry.".into()),
    }
}

/// This command is invoked only by the explicit Reconnect Google button, whose
/// explanation includes Google's project-wide revocation effect. Normal polling
/// and Retry never revoke authorization.
#[tauri::command]
pub async fn reconnect_google_calendar(app: tauri::AppHandle) -> Result<CalendarAgenda, String> {
    {
        let _guard = AUTH_LOCK
            .try_lock()
            .map_err(|_| "Google Calendar authentication is already in progress.".to_string())?;
        let stored = token_store::load()?.ok_or(
            "No saved Google authorization is available to reset. Remove LIA's access in your Google Account, then click Retry."
        )?;
        let client = google_client()?;
        let endpoint = reqwest::Url::parse("https://oauth2.googleapis.com/revoke").unwrap();
        report_info(
            "Google Calendar reconnection",
            "User requested a reset of Google authorization",
        );
        // Preserve the local credential on network/server errors so the user can
        // retry revocation. Clear it only after Google confirms the reset.
        revoke_google_authorization(&client, &endpoint, &stored.refresh_token).await?;
        token_store::clear()?;
        report_info(
            "Google Calendar reconnection",
            "Old authorization reset; starting browser sign-in",
        );
    }
    fetch_todays_events(app).await
}

async fn google_request<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    context: &'static str,
) -> Result<T, GoogleApiError> {
    let response = request.send().await.map_err(|error| GoogleApiError {
        status: None,
        code: None,
        message: report_error(context, error.without_url()),
    })?;
    read_google_response(response, context).await
}

async fn fetch_calendar_list(
    client: &Client,
    access_token: &str,
    api_root: &reqwest::Url,
) -> Result<Vec<CalendarEntry>, GoogleApiError> {
    let url = api_root
        .join("users/me/calendarList")
        .expect("Fixed Calendar API path");
    let mut calendars = Vec::new();
    let mut page_token: Option<String> = None;
    let mut seen_ids = std::collections::HashSet::new();
    loop {
        let mut request = client.get(url.clone()).bearer_auth(access_token).query(&[
            ("minAccessRole", "reader"),
            ("showHidden", "true"),
            ("showDeleted", "false"),
            ("maxResults", "250"),
            (
                "fields",
                "nextPageToken,items(id,summary,summaryOverride,accessRole,deleted)",
            ),
        ]);
        if let Some(token) = &page_token {
            request = request.query(&[("pageToken", token)]);
        }
        let page: CalendarListResponse = google_request(request, "Google calendar list").await?;
        calendars.extend(
            page.items.into_iter().filter(|calendar| {
                calendar.can_read_events() && seen_ids.insert(calendar.id.clone())
            }),
        );
        page_token = page.next_page_token.filter(|token| !token.is_empty());
        if page_token.is_none() {
            break;
        }
    }
    Ok(calendars)
}

async fn fetch_calendar_events(
    client: &Client,
    access_token: &str,
    api_root: &reqwest::Url,
    calendar_id: &str,
    time_min: &str,
    time_max: &str,
) -> Result<Vec<GoogleEvent>, GoogleApiError> {
    let mut url = api_root
        .join("calendars/")
        .expect("Fixed Calendar API path");
    // Encode the ID as one path segment, including any @, #, +, or slash.
    url.path_segments_mut()
        .expect("HTTP API URL")
        .pop_if_empty()
        .push(calendar_id)
        .push("events");
    let mut events = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut request = client.get(url.clone()).bearer_auth(access_token).query(&[
            ("timeMin", time_min),
            ("timeMax", time_max),
            ("singleEvents", "true"),
            ("orderBy", "startTime"),
            (
                "fields",
                "nextPageToken,items(id,summary,start(dateTime,date),end(dateTime,date))",
            ),
        ]);
        if let Some(token) = &page_token {
            request = request.query(&[("pageToken", token)]);
        }
        let page: CalendarResponse = google_request(request, "Google calendar events").await?;
        events.extend(page.items);
        page_token = page.next_page_token.filter(|token| !token.is_empty());
        if page_token.is_none() {
            break;
        }
    }
    Ok(events)
}

fn calendar_is_unavailable(error: &GoogleApiError) -> bool {
    match error.status {
        Some(reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE) => true,
        Some(reqwest::StatusCode::FORBIDDEN) => matches!(
            error.code.as_deref(),
            Some("forbidden" | "insufficientPermissions" | "notFound")
        ),
        _ => false,
    }
}

async fn fetch_account_agenda(
    client: &Client,
    access_token: &str,
    api_root: &reqwest::Url,
    now: chrono::DateTime<Local>,
) -> Result<CalendarAgenda, String> {
    let end_of_day = now
        .date_naive()
        .and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap());
    let end_of_day_local = Local
        .from_local_datetime(&end_of_day)
        .single()
        .ok_or("Could not determine the end of today in the local time zone")?;
    let time_min = now.to_rfc3339();
    let time_max = end_of_day_local.to_rfc3339();
    let calendars = fetch_calendar_list(client, access_token, api_root)
        .await
        .map_err(|error| error.message)?;
    let mut calendar_events = Vec::new();
    let mut unavailable_calendars = Vec::new();
    for calendar in calendars {
        match fetch_calendar_events(
            client,
            access_token,
            api_root,
            &calendar.id,
            &time_min,
            &time_max,
        )
        .await
        {
            Ok(events) => calendar_events.push((calendar, events)),
            // Sharing can change between discovery and fetching. Keep the other
            // calendars and explicitly warn the UI about the missing one.
            Err(error) if calendar_is_unavailable(&error) => {
                unavailable_calendars.push(calendar.display_name().to_owned())
            }
            // Auth, quota, outage, and transport errors are not an empty agenda.
            Err(error) => return Err(error.message),
        }
    }
    Ok(CalendarAgenda {
        events: build_agenda(calendar_events, now),
        unavailable_calendars,
    })
}

#[tauri::command]
pub async fn fetch_todays_events(app: tauri::AppHandle) -> Result<CalendarAgenda, String> {
    let access_token = get_calendar_access_token(&app)
        .await
        .map_err(|error| report_error("Calendar authentication", error))?;
    let client = google_client()?;
    let api_root = reqwest::Url::parse("https://www.googleapis.com/calendar/v3/").unwrap();
    fetch_account_agenda(&client, &access_token, &api_root, Local::now()).await
}

fn build_agenda(
    calendar_events: Vec<(CalendarEntry, Vec<GoogleEvent>)>,
    now: chrono::DateTime<Local>,
) -> Vec<AgendaEvent> {
    let mut agenda = Vec::new();
    let warning_threshold = now.with_timezone(&Utc) + Duration::minutes(10);

    for (calendar, events) in calendar_events {
        for item in events {
            let mut start_order = i64::MAX;
            let title = item.summary.unwrap_or_else(|| "Busy".to_string());

            let mut starts_in_ten = false;
            let mut is_in_progress = false;
            let mut is_all_day = false;
            let mut start_time_str = String::new();
            let mut end_time_str = String::new();

            if let Some(start) = item.start {
                if let Some(dt_str) = start.date_time {
                    if let Ok(parsed_start) = chrono::DateTime::parse_from_rfc3339(&dt_str)
                        .inspect_err(|_| {
                            report_error("Parse Google event start", "Invalid RFC3339 timestamp");
                        })
                    {
                        let start_utc = parsed_start.with_timezone(&Utc);
                        start_order = start_utc.timestamp();
                        let now_utc = now.with_timezone(&Utc);

                        // Check end time to see if event is already over or currently active
                        if let Some(end) = &item.end {
                            if let Some(end_dt_str) = &end.date_time {
                                if let Ok(parsed_end) = chrono::DateTime::parse_from_rfc3339(
                                    end_dt_str,
                                )
                                .inspect_err(|_| {
                                    report_error(
                                        "Parse Google event end",
                                        "Invalid RFC3339 timestamp",
                                    );
                                }) {
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
                        if !is_in_progress && start_utc > now_utc && start_utc <= warning_threshold
                        {
                            starts_in_ten = true;
                        }

                        start_time_str = parsed_start
                            .with_timezone(&Local)
                            .format("%I:%M %p")
                            .to_string();
                    }
                } else if let Some(date_str) = start.date {
                    is_all_day = true;
                    start_order = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
                        .ok()
                        .and_then(|date| date.and_hms_opt(0, 0, 0))
                        .map(|date| date.and_utc().timestamp())
                        .unwrap_or(i64::MAX);
                    start_time_str = date_str;
                }
            }

            agenda.push((
                (!is_all_day, start_order),
                AgendaEvent {
                    calendar_id: calendar.id.clone(),
                    calendar_name: calendar.display_name().to_owned(),
                    event_id: item.id.unwrap_or_default(),
                    title,
                    start_time: start_time_str,
                    end_time: end_time_str,
                    starts_in_ten,
                    is_in_progress,
                    is_all_day,
                },
            ));
        }
    }
    // Merge real timestamps instead of the formatted 12-hour display strings.
    // All-day events stay together at the top of the agenda.
    agenda.sort_by_key(|(order, _)| *order);
    agenda.into_iter().map(|(_, event)| event).collect()
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

    async fn mock_revocation(
        status: &str,
        body: &str,
    ) -> (Client, reqwest::Url, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let server = tokio::spawn(async move {
            let (mut stream, _) = timeout(StdDuration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            let header_end = loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                if let Some(index) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
            assert!(headers.starts_with("POST /revoke HTTP/1.1\r\n"));
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            while request.len() < header_end + length {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
            }
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request[header_end..header_end + length].to_vec()).unwrap()
        });
        let client = Client::builder()
            .no_proxy()
            .timeout(StdDuration::from_secs(3))
            .build()
            .unwrap();
        (
            client,
            reqwest::Url::parse(&format!("http://{address}/revoke")).unwrap(),
            server,
        )
    }

    #[test]
    fn explicit_reconnection_revokes_with_form_body_and_accepts_empty_success() {
        run_test(async {
            let (client, endpoint, server) = mock_revocation("200 OK", "").await;
            revoke_google_authorization(&client, &endpoint, "test+token&value=secret")
                .await
                .unwrap();
            let body = server.await.unwrap();
            let form = reqwest::Url::parse(&format!("http://localhost/?{body}")).unwrap();
            let pairs: Vec<_> = form.query_pairs().collect();
            assert_eq!(pairs.len(), 1);
            assert_eq!(pairs[0].0, "token");
            assert_eq!(pairs[0].1, "test+token&value=secret");
        });
    }

    #[test]
    fn explicit_reconnection_handles_already_revoked_tokens_and_rejects_other_failures() {
        run_test(async {
            for (status, body, succeeds) in [
                ("400 Bad Request", r#"{"error":"invalid_token"}"#, true),
                ("400 Bad Request", r#"{"error":"invalid_client"}"#, false),
                (
                    "503 Service Unavailable",
                    r#"{"error":{"errors":[{"reason":"backendError"}]}}"#,
                    false,
                ),
            ] {
                let (client, endpoint, server) = mock_revocation(status, body).await;
                let result = revoke_google_authorization(&client, &endpoint, "dummy-token").await;
                assert_eq!(result.is_ok(), succeeds);
                server.await.unwrap();
            }
        });
    }

    async fn mock_calendar_api(
        replies: Vec<(u16, serde_json::Value)>,
    ) -> (
        Client,
        reqwest::Url,
        tokio::task::JoinHandle<Vec<reqwest::Url>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, body) in replies {
                let (mut stream, _) = timeout(StdDuration::from_secs(3), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request
                    .to_lowercase()
                    .contains("authorization: bearer test-token\r\n"));
                let target = request.split_whitespace().nth(1).unwrap();
                requests.push(reqwest::Url::parse(&format!("http://{address}{target}")).unwrap());
                let body = body.to_string();
                let response = format!("HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    reqwest::StatusCode::from_u16(status).unwrap(), body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        let client = Client::builder()
            .no_proxy()
            .timeout(StdDuration::from_secs(3))
            .build()
            .unwrap();
        let root = reqwest::Url::parse(&format!("http://{address}/calendar/v3/")).unwrap();
        (client, root, server)
    }

    fn agenda_test_time() -> chrono::DateTime<Local> {
        chrono::DateTime::parse_from_rfc3339("2026-10-27T10:00:00Z")
            .unwrap()
            .with_timezone(&Local)
    }

    #[test]
    fn account_agenda_paginates_discovery_and_events_and_merges_sources() {
        run_test(async {
            use serde_json::json;
            let shared_id = "shared+team/#?@group.calendar.google.com";
            let (client, root, server) = mock_calendar_api(vec![
                (200, json!({"items":[
                    {"id":"personal@example.com","summary":"Personal","accessRole":"owner"},
                    {"id":"busy-only","accessRole":"freeBusyReader"},
                    {"id":"deleted","accessRole":"reader","deleted":true}
                ],"nextPageToken":"list-2"})),
                (200, json!({"items":[],"nextPageToken":"list-3"})),
                (200, json!({"items":[
                    {"id":"personal@example.com","summary":"duplicate","accessRole":"owner"},
                    {"id":"semester","summary":"Semester","accessRole":"owner","selected":false},
                    {"id":shared_id,"summary":"Raw team name","summaryOverride":"Project team","accessRole":"reader","hidden":true}
                ]})),
                (200, json!({"items":[{"id":"late","summary":"Late","start":{"dateTime":"2026-10-27T13:00:00Z"},"end":{"dateTime":"2026-10-27T14:00:00Z"}}],"nextPageToken":"events-2"})),
                (200, json!({"items":[],"nextPageToken":"events-3"})),
                (200, json!({"items":[
                    {"id":"ended","start":{"dateTime":"2026-10-27T08:00:00Z"},"end":{"dateTime":"2026-10-27T09:00:00Z"}},
                    {"id":"active","summary":"Active","start":{"dateTime":"2026-10-27T09:55:00Z"},"end":{"dateTime":"2026-10-27T10:30:00Z"}}
                ]})),
                (200, json!({"items":[
                    {"id":"early","summary":"Early","start":{"dateTime":"2026-10-27T10:05:00Z"},"end":{"dateTime":"2026-10-27T11:00:00Z"}},
                    {"id":"all-day","summary":"All day","start":{"date":"2026-10-27"},"end":{"date":"2026-10-28"}}
                ]})),
                (200, json!({"items":[{"id":"private","start":{"dateTime":"2026-10-27T12:00:00+01:00"},"end":{"dateTime":"2026-10-27T13:00:00+01:00"}}]})),
            ]).await;
            let agenda = fetch_account_agenda(&client, "test-token", &root, agenda_test_time())
                .await
                .unwrap();
            assert!(agenda.unavailable_calendars.is_empty());
            assert_eq!(
                agenda
                    .events
                    .iter()
                    .map(|event| event.event_id.as_str())
                    .collect::<Vec<_>>(),
                ["all-day", "active", "early", "private", "late"]
            );
            assert!(agenda.events[0].is_all_day);
            assert!(agenda.events[1].is_in_progress);
            assert!(agenda.events[2].starts_in_ten);
            assert_eq!(agenda.events[3].title, "Busy");
            assert_eq!(agenda.events[3].calendar_id, shared_id);
            assert_eq!(agenda.events[3].calendar_name, "Project team");
            let requests = server.await.unwrap();
            assert_eq!(requests.len(), 8);
            let query = |url: &reqwest::Url| {
                url.query_pairs()
                    .into_owned()
                    .collect::<std::collections::HashMap<_, _>>()
            };
            for request in &requests[..3] {
                assert_eq!(request.path(), "/calendar/v3/users/me/calendarList");
                assert_eq!(query(request)["showHidden"], "true");
                assert_eq!(query(request)["minAccessRole"], "reader");
            }
            assert_eq!(query(&requests[1])["pageToken"], "list-2");
            assert_eq!(query(&requests[2])["pageToken"], "list-3");
            assert_eq!(query(&requests[4])["pageToken"], "events-2");
            assert_eq!(query(&requests[5])["pageToken"], "events-3");
            for request in &requests[3..] {
                assert_eq!(query(request)["timeMin"], agenda_test_time().to_rfc3339());
                assert_eq!(query(request)["timeMax"], query(&requests[3])["timeMax"]);
                assert_eq!(query(request)["singleEvents"], "true");
                assert_eq!(query(request)["orderBy"], "startTime");
            }
            let encoded_id = requests[7]
                .path()
                .strip_prefix("/calendar/v3/calendars/")
                .unwrap()
                .strip_suffix("/events")
                .unwrap();
            assert_eq!(urlencoding::decode(encoded_id).unwrap(), shared_id);
        });
    }

    #[test]
    fn lost_shared_calendar_is_reported_without_discarding_available_events() {
        run_test(async {
            use serde_json::json;
            let (client, root, server) = mock_calendar_api(vec![
                (
                    200,
                    json!({"items":[
                        {"id":"mine","summary":"Mine","accessRole":"owner"},
                        {"id":"shared","summary":"Shared","accessRole":"reader"}
                    ]}),
                ),
                (
                    200,
                    json!({"items":[{"id":"event","start":{"date":"2026-10-27"}}]}),
                ),
                (403, json!({"error":{"errors":[{"reason":"forbidden"}]}})),
            ])
            .await;
            let agenda = fetch_account_agenda(&client, "test-token", &root, agenda_test_time())
                .await
                .unwrap();
            assert_eq!(agenda.events.len(), 1);
            assert_eq!(agenda.events[0].calendar_name, "Mine");
            assert_eq!(agenda.unavailable_calendars, ["Shared"]);
            server.await.unwrap();
        });
    }

    #[test]
    fn quota_authentication_and_outage_failures_are_not_silently_skipped() {
        run_test(async {
            use serde_json::json;
            for (status, reason) in [
                (403, "rateLimitExceeded"),
                (429, "rateLimitExceeded"),
                (401, "authError"),
                (500, "backendError"),
            ] {
                let (client, root, server) = mock_calendar_api(vec![
                    (200, json!({"items":[{"id":"mine","accessRole":"owner"}]})),
                    (status, json!({"error":{"errors":[{"reason":reason}]}})),
                ])
                .await;
                assert!(
                    fetch_account_agenda(&client, "test-token", &root, agenda_test_time())
                        .await
                        .is_err()
                );
                server.await.unwrap();
            }
        });
    }

    #[test]
    fn empty_calendar_list_returns_empty_agenda_without_primary_fallback() {
        run_test(async {
            let (client, root, server) =
                mock_calendar_api(vec![(200, serde_json::json!({"items":[]}))]).await;
            let agenda = fetch_account_agenda(&client, "test-token", &root, agenda_test_time())
                .await
                .unwrap();
            assert!(agenda.events.is_empty());
            assert!(agenda.unavailable_calendars.is_empty());
            assert_eq!(server.await.unwrap().len(), 1);
        });
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
            let error = read_google_response_for::<serde_json::Value>(
                response,
                "Test rejected refresh",
                GoogleResponseKind::TokenRefresh,
            )
            .await
            .unwrap_err();
            assert_eq!(error.code.as_deref(), Some("invalid_grant"));
            assert!(error.is_rejected_refresh_token());
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
            assert!(!error.is_rejected_refresh_token());
        });
    }

    #[test]
    fn client_errors_and_server_failures_do_not_trigger_token_recovery() {
        run_test(async {
            for (status, body) in [
                ("400 Bad Request", r#"{"error":"invalid_client"}"#),
                ("403 Forbidden", r#"{"error":"access_denied"}"#),
                ("500 Internal Server Error", r#"{"error":"invalid_grant"}"#),
            ] {
                let response = mock_google_response(status, body).await;
                let error = read_google_response_for::<serde_json::Value>(
                    response,
                    "Test nonrecoverable refresh",
                    GoogleResponseKind::TokenRefresh,
                )
                .await
                .unwrap_err();
                assert!(!error.is_rejected_refresh_token());
            }
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
            "GET {}?{query} HTTP/1.1\r\nHost: {}:{}\r\n\r\n",
            attempt.redirect_uri.path(),
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
    fn web_client_uses_identical_registered_uri_in_both_oauth_requests() {
        let attempt = attempt_at(web_callback_address());
        assert_eq!(attempt.redirect_uri.as_str(), "http://127.0.0.1:8989/");
        assert_eq!(attempt.redirect_uri.as_str(), GOOGLE_REDIRECT_URI);
        let url = attempt.authorization_url("test-web-client");
        let authorization: std::collections::HashMap<_, _> = url.query_pairs().collect();
        let exchange: std::collections::HashMap<_, _> = authorization_code_parameters(
            &attempt,
            "test-web-client",
            "test-client-secret",
            "test-code",
        )
        .into_iter()
        .collect();
        assert_eq!(authorization["redirect_uri"], GOOGLE_REDIRECT_URI);
        assert_eq!(exchange["redirect_uri"], authorization["redirect_uri"]);
        assert_eq!(exchange["client_secret"], "test-client-secret");
        assert_eq!(exchange["code_verifier"], attempt.verifier.as_str());
        let request = callback_request(
            &attempt,
            &format!("state={}&code=test-code", attempt.state.as_str()),
        );
        assert_eq!(
            parse_oauth_callback(request.as_bytes(), &attempt),
            OAuthCallback::Code("test-code".into())
        );
        // An older browser tab cannot complete a later attempt on the same port.
        let later_attempt = attempt_at(web_callback_address());
        assert_eq!(
            parse_oauth_callback(request.as_bytes(), &later_attempt),
            OAuthCallback::Ignore
        );
    }

    #[test]
    fn occupied_callback_port_fails_without_using_another_port() {
        run_test(async {
            let existing = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = existing.local_addr().unwrap();
            let error = bind_oauth_listener(address).await.unwrap_err();
            assert!(error.contains("already in use"));
            assert!(error.contains("cannot fall back"));
            drop(existing);
            let _replacement = bind_oauth_listener(address).await.unwrap();
        });
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
            valid.replace("GET /?", "GET /wrong?"),
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
        assert!(check_granted_scope(&format!("{CALENDAR_LIST_SCOPE} {EVENT_SCOPE}")).is_ok());
        // An old narrow grant may prompt for the additional discovery scope;
        // broad grants still need explicit removal, and are never reused.
        assert!(check_allowed_scopes(EVENT_SCOPE).is_ok());
        assert!(!has_required_scopes(EVENT_SCOPE));
        assert!(check_granted_scope(EVENT_SCOPE).is_err());
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
    fn scope_errors_distinguish_legacy_unknown_and_unexpected_permissions() {
        let old =
            check_allowed_scopes("https://www.googleapis.com/auth/calendar.readonly").unwrap_err();
        assert!(old.contains("old, broader calendar permission"));
        assert!(old.contains("Retry alone reuses the existing grant"));
        let unknown = check_allowed_scopes("").unwrap_err();
        assert!(unknown.contains("did not provide verifiable"));
        let extra = check_allowed_scopes(&format!("{GOOGLE_SCOPES} private-unrecognized-value"))
            .unwrap_err();
        assert!(extra.contains("additional permissions"));
        assert!(!extra.contains("private-unrecognized-value"));
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
            assert!(error.contains("before the browser returned"));
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
            assert!(error.contains("without a valid callback"));
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
