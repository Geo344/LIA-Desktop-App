use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use crate::diagnostics::report_error;

// --- DATA MODELS ---

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    pub completed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TodoList {
    pub id: String,
    pub title: String,
    #[serde(default)] // Automatically assigns 'false' to existing lists without this property
    pub archived: bool,
    pub items: Vec<TodoItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UserData {
    pub lists: Vec<TodoList>,
    pub notes: String,
}

// Automatically initializes empty arrays/strings for new users
impl Default for UserData {
    fn default() -> Self {
        Self {
            lists: Vec::new(),
            notes: String::new(),
        }
    }
}

// --- FILE SYSTEM ROUTING ---

fn get_data_storage_path() -> PathBuf {
    // Maps directly to C:\Users\<Name>\AppData\Roaming\LIA\data.json
    if let Ok(appdata) = std::env::var("APPDATA") {
        PathBuf::from(appdata).join("LIA").join("data.json")
    } else {
        PathBuf::from("data.json")
    }
}

// --- IPC COMMANDS ---

#[tauri::command]
pub fn load_user_data() -> Result<UserData, String> {
    let path = get_data_storage_path();
    
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(data) => {
                // Do not turn corrupted data into an empty state that autosave
                // could write over the user's existing notes.
                serde_json::from_str(&data).map_err(|_| report_error("Load notes", "data.json contains invalid JSON or an invalid data format"))
            }
            Err(e) => Err(report_error("Read notes file", e)),
        }
    } else {
        // Return empty default state if the file doesn't exist yet
        Ok(UserData::default())
    }
}

#[tauri::command]
pub fn save_user_data(data: UserData) -> Result<(), String> {
    let path = get_data_storage_path();
    
    // Ensure the LIA directory exists before writing
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| report_error("Create notes directory", e))?;
    }

    let json = serde_json::to_string_pretty(&data).map_err(|_| report_error("Serialize notes", "Could not serialize user data"))?;
    
    fs::write(path, json).map_err(|e| report_error("Write notes file", e))?;
    
    Ok(())
}
