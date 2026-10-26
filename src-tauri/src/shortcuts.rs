use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use crate::diagnostics::report_error;

// Desktop item representation for the frontend
#[derive(Serialize)]
pub struct DesktopItem {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

// Scan user and public desktop folders
#[tauri::command]
pub fn get_desktop_items() -> Result<Vec<DesktopItem>, String> {
    let mut items = Vec::new();
    let mut directories = Vec::new();

    if let Ok(user_home) = std::env::var("USERPROFILE") {
        directories.push(PathBuf::from(user_home).join("Desktop"));
    }

    if let Ok(public_dir) = std::env::var("PUBLIC") {
        directories.push(PathBuf::from(public_dir).join("Desktop"));
    }

    for desktop_path in directories {
        if let Ok(entries) = fs::read_dir(desktop_path).inspect_err(|error| {
            if error.kind() != std::io::ErrorKind::NotFound { report_error("Read desktop directory", error); }
        }) {
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => { report_error("Read desktop entry", error); continue; }
                };
                let path = entry.path();
                let file_name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                let is_dir = path.is_dir();

                if !file_name.is_empty() && file_name != "desktop" {
                    items.push(DesktopItem {
                        name: file_name,
                        path: path.to_string_lossy().to_string(),
                        is_dir,
                    });
                }
            }
        }
    }
    Ok(items)
}

// Launch selected application or folder
#[tauri::command]
pub fn launch_item(path: String) -> Result<(), String> {
    Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| report_error("Launch desktop shortcut", e))?;
    Ok(())
}
