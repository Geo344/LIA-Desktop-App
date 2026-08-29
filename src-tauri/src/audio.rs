use rodio::{Decoder, OutputStream, OutputStreamHandle, Source};
use std::io::Cursor;
use tauri::State;

// Embed the sound effect directly into the compiled Rust binary
static SOUND_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/ConfirmSound.wav");

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

// Play ping sound effect for shortcuts
#[tauri::command]
pub fn play_ping(audio: State<'_, AppAudioState>) {
    let cursor = Cursor::new(SOUND_BYTES);
    if let Ok(source) = Decoder::new(cursor) {
        let _ = audio.stream_handle.play_raw(source.convert_samples());
    }
}