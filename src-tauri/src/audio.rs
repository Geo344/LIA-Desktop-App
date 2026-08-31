use rodio::{Decoder, OutputStream, OutputStreamHandle, Source};
use std::io::Cursor;
use tauri::State;

// Embed both sound effects directly into the compiled Rust binary
static SHORTCUT_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Shortcut-button.wav");
static MUSIC_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Music-button.wav");

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
        _ => SHORTCUT_BYTES, // Default to the shortcut click
    };

    let cursor = Cursor::new(bytes);
    if let Ok(source) = Decoder::new(cursor) {
        let _ = audio.stream_handle.play_raw(source.convert_samples());
    }
}