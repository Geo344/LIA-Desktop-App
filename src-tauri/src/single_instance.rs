//! Share one app instance across debug and release builds in a Windows session.
//! The OS releases the handle even if the application crashes.
#[cfg(target_os = "windows")]
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, SetLastError, ERROR_ALREADY_EXISTS, HANDLE, WIN32_ERROR,
        },
        System::Threading::CreateMutexW,
    },
};

#[cfg(target_os = "windows")]
pub(crate) struct InstanceGuard(HANDLE);

#[cfg(target_os = "windows")]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // No mutex ownership is acquired: the existence of the named object,
        // kept alive by this handle, is the cross-process guard.
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            crate::diagnostics::report_error("Release LIA instance guard", error);
        }
    }
}

#[cfg(target_os = "windows")]
fn acquire_named(name: &str) -> Result<Option<InstanceGuard>, String> {
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    unsafe {
        SetLastError(WIN32_ERROR(0));
        let handle = CreateMutexW(None, false, PCWSTR(name.as_ptr()))
            .map_err(|error| format!("Could not create the LIA instance guard: {error}"))?;
        let exists = GetLastError() == ERROR_ALREADY_EXISTS;
        let guard = InstanceGuard(handle);
        if exists {
            // Close only this launch's handle; the running app keeps its own.
            drop(guard);
            Ok(None)
        } else {
            Ok(Some(guard))
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn acquire() -> Result<Option<InstanceGuard>, String> {
    acquire_named(r"Local\com.desktop.lia.application")
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn duplicate_launch_is_rejected_and_guard_is_released_on_exit() {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let name = format!(r"Local\lia-instance-test-{suffix}");
        let first = acquire_named(&name).unwrap().expect("First instance");
        assert!(acquire_named(&name).unwrap().is_none());
        drop(first);
        assert!(acquire_named(&name).unwrap().is_some());
    }
}
