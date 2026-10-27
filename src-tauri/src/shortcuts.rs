use crate::diagnostics::report_error;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
            if error.kind() != std::io::ErrorKind::NotFound {
                report_error("Read desktop directory", error);
            }
        }) {
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        report_error("Read desktop entry", error);
                        continue;
                    }
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

// The frontend sends an identifier, never an executable path or a shell URI.
const APPROVED_SHORTCUTS: &[&str] = &[
    "2026 Fall Semester",
    "calibre",
    "Google Finance",
    "Zoom Workplace",
    "Discord",
    "Minecraft Launcher",
    "Roblox Player",
    "Steam",
    "Visual Studio Code",
    "Krita",
    "YouTube Music",
];

fn is_approved_name(name: &str) -> bool {
    APPROVED_SHORTCUTS
        .iter()
        .any(|approved| approved.eq_ignore_ascii_case(name))
}

fn approved_target(path: &Path, desktop: &Path) -> Result<PathBuf, String> {
    let root =
        fs::canonicalize(desktop).map_err(|error| report_error("Resolve desktop folder", error))?;
    let target =
        fs::canonicalize(path).map_err(|error| report_error("Resolve desktop shortcut", error))?;
    // Only immediate desktop children are eligible. Canonicalization rejects
    // traversal, symlink/junction escapes, and alternate spellings of paths.
    if target.parent() != Some(root.as_path()) {
        return Err("Shortcut must be directly inside an approved desktop folder".into());
    }
    if !target.is_dir()
        && !target
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("lnk") || extension.eq_ignore_ascii_case("url")
            })
    {
        return Err(
            "Only configured desktop folders and .lnk/.url shortcuts may be launched".into(),
        );
    }
    // Keep Explorer's ordinary path spelling. Windows canonical paths use a
    // verbatim prefix that the shell does not consistently accept.
    Ok(path.to_path_buf())
}

#[tauri::command]
pub fn launch_item(name: String) -> Result<(), String> {
    let target = if name == "Recycle Bin" {
        // This is the only allowed shell URI; it never comes from a path argument.
        "shell:RecycleBinFolder".to_string()
    } else {
        if !is_approved_name(&name) {
            return Err(report_error(
                "Launch desktop shortcut",
                "Unknown shortcut identifier",
            ));
        }
        let items = get_desktop_items()?;
        // Preserve the widget's name matching while resolving the path solely
        // from the backend's own desktop scan, not from frontend-provided data.
        let item = items
            .iter()
            .find(|item| item.name.to_lowercase().contains(&name.to_lowercase()))
            .ok_or_else(|| {
                report_error(
                    "Launch desktop shortcut",
                    "Configured shortcut was not found",
                )
            })?;
        let path = Path::new(&item.path);
        let desktop = path.parent().ok_or("Invalid desktop shortcut")?;
        approved_target(path, desktop)
            .map_err(|error| report_error("Validate desktop shortcut", error))?
            .to_string_lossy()
            .into_owned()
    };
    // Use the Windows executable's absolute location to avoid PATH search.
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("Windows system directory is unavailable")?;
    Command::new(system_root.join("explorer.exe"))
        .arg(target)
        .spawn()
        .map_err(|error| report_error("Launch desktop shortcut", error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_urls_and_shell_commands_as_shortcut_names() {
        for name in [
            "C:\\Windows\\System32\\cmd.exe",
            "shell:AppsFolder",
            "https://example.com",
            "../Steam",
            "Steam & calc",
            "",
        ] {
            assert!(!is_approved_name(name));
        }
        assert!(is_approved_name("Steam"));
        assert!(is_approved_name("visual studio code"));
    }

    #[test]
    fn rejects_outside_paths_and_executables() {
        let desktop = std::env::temp_dir();
        // These existing paths require no filesystem mutation.
        let executable = std::env::current_exe().unwrap();
        assert!(approved_target(&executable, &desktop).is_err());
        assert!(approved_target(desktop.parent().unwrap(), &desktop).is_err());
        assert!(approved_target(&executable, executable.parent().unwrap()).is_err());
    }

    #[test]
    fn accepts_desktop_shortcuts_and_folders_but_not_other_files() {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let desktop = std::env::temp_dir().join(format!("lia-shortcut-test-{suffix}"));
        fs::create_dir(&desktop).unwrap();
        let shortcut = desktop.join("Steam.lnk");
        let executable = desktop.join("Steam.exe");
        let folder = desktop.join("2026 Fall Semester");
        fs::write(&shortcut, b"dummy test shortcut; never executed").unwrap();
        fs::write(&executable, b"dummy test executable; never executed").unwrap();
        fs::create_dir(&folder).unwrap();
        assert_eq!(approved_target(&shortcut, &desktop).unwrap(), shortcut);
        assert_eq!(approved_target(&folder, &desktop).unwrap(), folder);
        assert!(approved_target(&executable, &desktop).is_err());
        fs::remove_file(shortcut).unwrap();
        fs::remove_file(executable).unwrap();
        fs::remove_dir(folder).unwrap();
        fs::remove_dir(desktop).unwrap();
    }
}
