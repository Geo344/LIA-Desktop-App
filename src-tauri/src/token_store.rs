//! Refresh tokens are encrypted for the current Windows user, never for the
//! whole machine. No credentials are read from or written to the working directory.
use crate::diagnostics::report_error;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredToken {
    pub refresh_token: String,
    #[serde(default)]
    pub scope: String,
}

impl Drop for StoredToken {
    fn drop(&mut self) {
        self.refresh_token.zeroize();
    }
}

fn storage_directory() -> Result<PathBuf, String> {
    let appdata = std::env::var_os("APPDATA")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            report_error(
                "Google token storage",
                "An absolute APPDATA directory is required",
            )
        })?;
    Ok(appdata.join("LIA"))
}

#[cfg(target_os = "windows")]
fn protect(bytes: &[u8], decrypt: bool) -> Result<Zeroizing<Vec<u8>>, String> {
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{LocalFree, HLOCAL},
            Security::Cryptography::{
                CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
            },
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(|_| "Token storage is too large")?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // DPAPI owns the output allocation. Copy into a zeroizing Rust buffer, wipe
    // the native plaintext allocation, and free it with the matching allocator.
    unsafe {
        let result = if decrypt {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        result.map_err(|error| report_error("Protect Google token storage", error))?;
        let bytes = if output.cbData == 0 {
            Zeroizing::new(Vec::new())
        } else {
            if output.pbData.is_null() {
                return Err(report_error(
                    "Protect Google token storage",
                    "Windows returned an invalid allocation",
                ));
            }
            let native = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
            let bytes = Zeroizing::new(native.to_vec());
            native.zeroize();
            bytes
        };
        let remaining = LocalFree(HLOCAL(output.pbData.cast()));
        if !remaining.0.is_null() {
            report_error(
                "Free Google token buffer",
                "Windows could not release the DPAPI allocation",
            );
        }
        Ok(bytes)
    }
}

#[cfg(not(target_os = "windows"))]
fn protect(_: &[u8], _: bool) -> Result<Zeroizing<Vec<u8>>, String> {
    Err("Secure Google token storage currently requires Windows".into())
}

fn save_at(directory: &Path, token: &StoredToken) -> Result<(), String> {
    let plaintext = Zeroizing::new(
        serde_json::to_vec(token)
            .map_err(|_| report_error("Serialize Google token", "Invalid token storage"))?,
    );
    let encrypted = protect(&plaintext, false)?;
    fs::create_dir_all(directory)
        .map_err(|error| report_error("Create Google token directory", error))?;
    let destination = directory.join("tokens.dpapi");
    let temporary = directory.join("tokens.dpapi.tmp");
    // Only ciphertext ever reaches disk. Flush it before replacing the old file.
    let write = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&encrypted)?;
        file.sync_all()?;
        fs::rename(&temporary, &destination)
    })();
    if let Err(error) = write {
        let _ = fs::remove_file(&temporary);
        return Err(report_error("Save protected Google token", error));
    }
    Ok(())
}

fn remove_legacy(directory: &Path) -> Result<(), String> {
    match fs::remove_file(directory.join("tokens.json")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(report_error("Remove plaintext Google token", error)),
    }
}

fn load_at(directory: &Path) -> Result<Option<StoredToken>, String> {
    match fs::read(directory.join("tokens.dpapi")) {
        Ok(bytes) => {
            let plaintext = protect(&bytes, true)?;
            let stored = serde_json::from_slice(&plaintext).map_err(|_| {
                report_error("Read protected Google token", "Invalid token storage")
            })?;
            // Retry cleanup if an earlier migration saved ciphertext but failed
            // to remove the old file. Never reuse that plaintext copy.
            remove_legacy(directory)?;
            return Ok(Some(stored));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(report_error("Read protected Google token", error)),
    }
    let plaintext = match fs::read(directory.join("tokens.json")) {
        Ok(bytes) => Zeroizing::new(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(report_error("Read legacy Google token", error)),
    };
    #[derive(Deserialize)]
    struct LegacyToken {
        refresh_token: Option<String>,
    }
    let legacy: LegacyToken = serde_json::from_slice(&plaintext)
        .map_err(|_| report_error("Migrate Google token", "Invalid legacy token format"))?;
    let Some(refresh_token) = legacy.refresh_token.filter(|token| !token.is_empty()) else {
        remove_legacy(directory)?;
        return Ok(None);
    };
    // Empty scope marks an old grant that must be verified before it is used.
    let stored = StoredToken {
        refresh_token,
        scope: String::new(),
    };
    save_at(directory, &stored)?;
    remove_legacy(directory)?;
    Ok(Some(stored))
}

pub(crate) fn load() -> Result<Option<StoredToken>, String> {
    load_at(&storage_directory()?)
}
pub(crate) fn save(token: &StoredToken) -> Result<(), String> {
    save_at(&storage_directory()?, token)
}
pub(crate) fn clear() -> Result<(), String> {
    let directory = storage_directory()?;
    match fs::remove_file(directory.join("tokens.dpapi")) {
        Ok(()) => remove_legacy(&directory),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => remove_legacy(&directory),
        Err(error) => Err(report_error("Remove protected Google token", error)),
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let mut random = [0; 16];
            getrandom::fill(&mut random).unwrap();
            let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = std::env::temp_dir().join(format!("lia-token-test-{name}"));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            for name in ["tokens.json", "tokens.dpapi", "tokens.dpapi.tmp"] {
                let _ = fs::remove_file(self.0.join(name));
            }
            let _ = fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn dpapi_roundtrip_replacement_and_tamper_rejection() {
        let directory = TestDirectory::new();
        let mut token = StoredToken {
            refresh_token: "test-refresh-secret".into(),
            scope: "test-scope".into(),
        };
        save_at(&directory.0, &token).unwrap();
        let encrypted = fs::read(directory.0.join("tokens.dpapi")).unwrap();
        assert!(!encrypted
            .windows(token.refresh_token.len())
            .any(|part| part == token.refresh_token.as_bytes()));
        assert_eq!(
            load_at(&directory.0).unwrap().unwrap().refresh_token,
            token.refresh_token
        );
        token.refresh_token = "replacement-secret".into();
        save_at(&directory.0, &token).unwrap();
        assert_eq!(
            load_at(&directory.0).unwrap().unwrap().refresh_token,
            "replacement-secret"
        );
        fs::write(directory.0.join("tokens.dpapi"), b"tampered").unwrap();
        assert!(load_at(&directory.0).is_err());
    }

    #[test]
    fn legacy_token_is_encrypted_before_plaintext_is_removed() {
        let directory = TestDirectory::new();
        fs::write(
            directory.0.join("tokens.json"),
            br#"{"refresh_token":"legacy-test-secret"}"#,
        )
        .unwrap();
        let stored = load_at(&directory.0).unwrap().unwrap();
        assert_eq!(stored.refresh_token, "legacy-test-secret");
        assert!(stored.scope.is_empty());
        assert!(!directory.0.join("tokens.json").exists());
        assert!(directory.0.join("tokens.dpapi").exists());
    }

    #[test]
    fn failed_migration_preserves_legacy_and_corruption_does_not_fall_back() {
        let directory = TestDirectory::new();
        let legacy_path = directory.0.join("tokens.json");
        fs::write(&legacy_path, br#"{"refresh_token":"legacy-test-secret"}"#).unwrap();
        // Block the ciphertext write; migration must retain the original token.
        let temporary = directory.0.join("tokens.dpapi.tmp");
        fs::create_dir(&temporary).unwrap();
        assert!(load_at(&directory.0).is_err());
        assert!(legacy_path.exists());
        fs::remove_dir(temporary).unwrap();
        fs::write(directory.0.join("tokens.dpapi"), b"corrupt").unwrap();
        assert!(load_at(&directory.0).is_err());
        assert!(legacy_path.exists());
    }
}
