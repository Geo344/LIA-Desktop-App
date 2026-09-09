use rodio::{cpal::traits::{DeviceTrait, HostTrait}, Decoder, OutputStream, Source};
use std::io::Cursor;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tauri::State;

// Embed sound effects directly into the compiled Rust binary
static SHORTCUT_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Shortcut-button.wav");
static MUSIC_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Music-button.wav");
static NOTEPAD_OPEN_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Notepad-Open.wav");
static NOTEPAD_SWITCH_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Notepad-Switch.wav");
static NOTEPAD_CHECK_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Notepad-CheckItem.wav");
static NOTEPAD_CLICK_BYTES: &[u8] = include_bytes!("../../src/assets/sound_effects/Notepad-Click.wav");

pub struct AudioEngine {
    tx: mpsc::Sender<String>,
}

impl AudioEngine {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<String>();

        thread::spawn(move || {
            let host = rodio::cpal::default_host();
            let mut current_device = host.default_output_device().and_then(|d| d.name().ok());
            let (mut _stream, mut stream_handle) = OutputStream::try_default().expect("Failed to initialize audio");

            loop {
                // Wait for a play request, waking up every 1 second while idle to check hardware
                match rx.recv_timeout(Duration::from_millis(1000)) {
                    Ok(sound_type) => {
                        let bytes = match sound_type.as_str() {
                            "music" => MUSIC_BYTES,
                            "notepad_open" => NOTEPAD_OPEN_BYTES,
                            "notepad_switch" => NOTEPAD_SWITCH_BYTES,
                            "notepad_check" => NOTEPAD_CHECK_BYTES,
                            "notepad_click" => NOTEPAD_CLICK_BYTES,
                            _ => SHORTCUT_BYTES, 
                        };

                        if let Ok(decoder) = Decoder::new(Cursor::new(bytes)) {
                            let _ = stream_handle.play_raw(decoder.convert_samples());
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Idle timeout reached. Check Windows for a new default device
                        if let Some(device) = host.default_output_device() {
                            if let Ok(name) = device.name() {
                                if Some(name.clone()) != current_device {
                                    // Device hot-swap detected! Rebuild the stream in the background
                                    if let Ok((new_stream, new_handle)) = OutputStream::try_default() {
                                        _stream = new_stream;
                                        stream_handle = new_handle;
                                        current_device = Some(name);
                                    }
                                }
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break, // App is closing
                }
            }
        });

        Self { tx }
    }
}

#[tauri::command]
pub fn play_ping(audio: State<'_, AudioEngine>, sound_type: String) {
    let _ = audio.tx.send(sound_type);
}