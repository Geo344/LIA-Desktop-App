use rodio::{Decoder, OutputStream, OutputStreamHandle, Source};
use std::io::Cursor;
use tauri::State;

// Embed sound effects directly into the compiled Rust binary
static SHORTCUT_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Shortcut-button.wav");
static MUSIC_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Music-button.wav");
static NOTEPAD_OPEN_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Notepad-Open.wav");
static NOTEPAD_SWITCH_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Notepad-Switch.wav");
static NOTEPAD_CHECK_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Notepad-CheckItem.wav");
static NOTEPAD_CLICK_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Notepad-Click.wav");

// Thread-safe state holding only the stream handle
pub struct AppAudioState {
    pub stream_handle: OutputStreamHandle,
}

impl AppAudioState {
    pub fn new() -> Self {
        let (stream, stream_handle) = OutputStream::try_default().expect("Failed to open audio device");
        // Keep the hardware output stream open for the entire process lifetime
        std::mem::forget(stream);
        Self { stream_handle }
    }
}

// Play sound effect based on the requested type
#[tauri::command]
pub fn play_ping(audio: State<'_, AppAudioState>, sound_type: &str) {
    let bytes = match sound_type {
        "music" => MUSIC_BYTES,
        "notepad_open" => NOTEPAD_OPEN_BYTES,
        "notepad_switch" => NOTEPAD_SWITCH_BYTES,
        "notepad_check" => NOTEPAD_CHECK_BYTES,
        "notepad_click" => NOTEPAD_CLICK_BYTES,
        _ => SHORTCUT_BYTES, // Default to the shortcut click
    };

    let cursor = Cursor::new(bytes);
    if let Ok(source) = Decoder::new(cursor) {
        let _ = audio.stream_handle.play_raw(source.convert_samples());
    }
}