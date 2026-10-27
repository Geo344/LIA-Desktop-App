use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

static LAST_ERRORS: LazyLock<Mutex<HashMap<&'static str, (String, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Log contextual failures to backend stderr. Suppress identical polling errors
/// for 30 seconds. Callers must not pass credentials, URLs, or response bodies.
pub(crate) fn report_error(context: &'static str, error: impl Display) -> String {
    report("ERROR", context, error)
}

pub(crate) fn report_info(context: &'static str, detail: impl Display) {
    report("INFO", context, detail);
}

fn report(level: &'static str, context: &'static str, detail: impl Display) -> String {
    let message = format!("{context}: {detail}");
    let should_log = LAST_ERRORS.lock().map(|mut errors| {
        let now = Instant::now();
        if let Some((previous, timestamp)) = errors.get(context) {
            if previous == &message && now.duration_since(*timestamp) < Duration::from_secs(30) {
                return false;
            }
        }
        errors.insert(context, (message.clone(), now));
        true
    }).unwrap_or(true);
    if should_log {
        eprintln!("[{} LIA {level}] {message}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%:z"));
    }
    message
}
