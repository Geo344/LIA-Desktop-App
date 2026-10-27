# LIA (Lightweight Integrated Assistant)
My own personally designed wallpaper desktop app with an integrated assistant (In development). Only works on Windows rn lol.

- Vivy Fluorite Eye's Song anime wallpaper
- Shortcuts with personally designed icons that map specifically to the shortcuts in my own desktop folder.
- Music player widget was developed for Youtube music playlists, but it can display the details of other Youtube videos and other things
- Added sound effects are all taken from Sword Art Online Aincrad Arc. 

# Tauri Architecture
Backend: Rust
Frontend: Typescript + React

# Local Deployment
npm run tauri dev - local development command
npm run tauri build - build .exe application file

# Google Calendar security

Use your Google OAuth client of type **Web application**, with the Calendar API enabled.
Set `GOOGL_CLIENT_ID` and `GOOGL_CLIENT_SECRET` in a local `.env` file. These names
intentionally use `GOOGL`. Do not bundle or commit your real `.env` file.

In Google Cloud's OAuth client settings, **Authorized redirect URIs** must contain
exactly `http://127.0.0.1:8989/`, including the trailing slash. `localhost`, a different
port, or an `/oauth/callback` path is a different URI. Keep the same client ID and
secret; no Desktop app credentials are needed for this personal local setup.

Sign-in opens the system browser after binding `127.0.0.1:8989` and uses that
exact registered callback at `/`. Each attempt has a new random `state` and PKCE S256
verifier/challenge. The app rejects mismatched, duplicate, or malformed callback
parameters and expires the attempt after two minutes.

If Google displays an error page, authorization has not reached LIA's callback.
Check the error heading on that page and confirm the registered URI matches the
fixed URI above. A busy port is reported before opening the browser; the app never
silently switches to an unregistered port.
After a timeout or app restart, use the newest sign-in tab; older tabs cannot
complete a newer attempt. Logs distinguish a timeout without any callback from
rejected callbacks and identify each attempt without logging authorization URLs,
codes, state values, or tokens.

Updated Windows builds use a session-wide instance guard shared by debug and
release versions. A second launch exits before starting widgets, audio, or
authentication. Older binaries built without the guard must still be closed
manually. Close an existing release build before starting `npm run tauri dev`.

Refresh tokens are encrypted with **Windows user-scoped DPAPI** in
`%APPDATA%\LIA\tokens.dpapi`. The first use migrates the old `tokens.json` in that
directory: it saves the encrypted version before deleting the plaintext file.
Migration fails without deleting the original if encryption or saving fails.
There is no working-directory fallback. Old token files copied elsewhere or kept
in backups are not removed by migration. DPAPI does not protect against malware
already running as your Windows user; notes in `data.json` remain ordinary files.

The app requests `calendar.events.readonly` to read events and
`calendar.calendarlist.readonly` to discover your calendars. It reads every page
of the calendar list and each calendar's events, including your primary calendar,
calendars you created, and subscribed/shared calendars under **Other calendars**.
Hidden or unchecked calendars are included. Calendars shared with only free/busy
access cannot provide event details and are excluded. Shared private event details
remain subject to Google's sharing permissions.

The agenda still covers ongoing events and the rest of the current local day.
It merges events by start time and puts all-day events first. Each row displays
the event's time and title. An event present in multiple calendars appears once per
calendar. If access to one calendar disappears after discovery, the widget warns
you while showing the others. Authentication, quota, and server failures remain
errors rather than being mistaken for an empty agenda.

An older `calendar.events.readonly` token automatically prompts for the additional
calendar-list permission on next use. Grant both permissions in Google's consent
screen. Broader or unrelated scopes, such as `calendar.readonly`, are rejected.
If an old grant prevents sign-in, the widget offers **Reconnect Google**. Its
explanation states that revocation affects every OAuth client in the same Google
Cloud project. Clicking it revokes the saved authorization, clears the local
credential after Google confirms the reset, and starts browser sign-in. Normal
polling and **Retry** never revoke access. Network or server errors during reset
preserve the credential so revocation can be retried. Alternatively, remove LIA
under your Google Account's **Third-party connections**, then click **Retry**.
Keep a separate Google Cloud project for LIA.

Production CSP allows local scripts/assets, inline styles used by React, data
images used for artwork, and Tauri IPC. It blocks remote scripts, images, frames,
and frontend network requests. Google API calls run in Rust. The development
policy additionally allows Vite's local WebSocket and React refresh preamble;
remote `TAURI_DEV_HOST` development is not included in this policy.

Shortcut launch commands accept configured names rather than paths. Add new
shortcut identifiers to `APPROVED_SHORTCUTS` in `src-tauri/src/shortcuts.rs` as well
as the UI configuration. Only desktop directories and `.lnk`/`.url` files are
eligible; desktop shortcuts themselves must still be trusted.

Checks: `cargo test --manifest-path src-tauri/Cargo.toml --lib --offline` and
`npm run build`. DPAPI tests use dummy tokens in temporary directories and need
access to a normal Windows user profile.
