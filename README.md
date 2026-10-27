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

Use a Google OAuth client of type **Desktop app**, with the Calendar API enabled.
Set `GOOGL_CLIENT_ID` and `GOOGL_CLIENT_SECRET` in a local `.env` file. These names
intentionally use `GOOGL`. Do not bundle or commit your real `.env` file.

Sign-in opens the system browser and uses a random port on `127.0.0.1` with an
exact `/oauth/callback` path. Each attempt has a new random `state` and PKCE S256
verifier/challenge. The app rejects mismatched, duplicate, or malformed callback
parameters and expires the attempt after two minutes.

Refresh tokens are encrypted with **Windows user-scoped DPAPI** in
`%APPDATA%\LIA\tokens.dpapi`. The first use migrates the old `tokens.json` in that
directory: it saves the encrypted version before deleting the plaintext file.
Migration fails without deleting the original if encryption or saving fails.
There is no working-directory fallback. Old token files copied elsewhere or kept
in backups are not removed by migration. DPAPI does not protect against malware
already running as your Windows user; notes in `data.json` remain ordinary files.

The app now requests only `calendar.events.readonly` and checks the granted
scope before reading events. An existing `calendar.readonly` grant is rejected.
If the widget asks for a new grant, remove LIA under your Google Account's
**Third-party connections**, then click **Retry** and sign in again. Revoking
Google access affects all clients in the same Google Cloud project, so LIA does
not revoke it automatically. Keep a separate Google Cloud project for LIA.

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
