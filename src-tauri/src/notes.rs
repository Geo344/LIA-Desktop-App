use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

// --- DATA MODELS ---

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    pub completed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UserData {
    pub todos: Vec<TodoItem>,
    pub notes: String,
}

// Automatically initializes empty arrays/strings for new users
impl Default for UserData {
    fn default() -> Self {
        Self {
            todos: Vec::new(),
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
                // Gracefully handles corrupted or empty JSON strings by falling back to defaults
                let user_data: UserData = serde_json::from_str(&data).unwrap_or_default();
                Ok(user_data)
            }
            Err(e) => Err(format!("Failed to read data.json: {}", e)),
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
        let _ = fs::create_dir_all(parent);
    }

    let json = serde_json::to_string_pretty(&data).map_err(|e| format!("Serialization error: {}", e))?;
    
    fs::write(path, json).map_err(|e| format!("Failed to write data.json: {}", e))?;
    
    Ok(())
}