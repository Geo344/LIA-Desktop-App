use tauri::WebviewWindow;
use crate::diagnostics::report_error;

#[cfg(target_os = "windows")]
use windows::{
    core::{w, PCWSTR},
    Win32::Foundation::{BOOL, HWND, LPARAM, RECT, WPARAM, GetLastError, SetLastError, WIN32_ERROR},
    Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowExW, FindWindowW, GetSystemMetrics, GetWindowLongPtrW,
        SendMessageTimeoutW, SetParent, SetWindowLongPtrW, SetWindowPos, SystemParametersInfoW,
        GWL_STYLE, SMTO_NORMAL, SM_CXSCREEN, SPI_GETWORKAREA, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WS_CHILD,
        WS_POPUP, WS_VISIBLE,
    },
};

// Attach Window Handle to Windows WorkerW wallpaper layer
#[cfg(target_os = "windows")]
static mut WORKERW_HWND: HWND = HWND(std::ptr::null_mut());

#[cfg(target_os = "windows")]
unsafe extern "system" fn enum_windows_proc(hwnd: HWND, _: LPARAM) -> BOOL {
    if let Ok(shell_view) = FindWindowExW(hwnd, HWND::default(), w!("SHELLDLL_DefView"), PCWSTR::null()) {
        if !shell_view.0.is_null() {
            if let Ok(workerw) = FindWindowExW(HWND::default(), hwnd, w!("WorkerW"), PCWSTR::null()) {
                if !workerw.0.is_null() {
                    WORKERW_HWND = workerw;
                    return BOOL(0);
                }
            }
        }
    }
    BOOL(1)
}

#[cfg(target_os = "windows")]
pub fn attach_to_workerw(window: &WebviewWindow) {
    unsafe {
        let progman = match FindWindowW(w!("Progman"), PCWSTR::null()) {
            Ok(hwnd) => hwnd,
            Err(error) => { report_error("Find desktop shell window", error); return; },
        };

        let mut result: usize = 0;
        let sent = SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0xD),
            LPARAM(0),
            SMTO_NORMAL,
            1000,
            Some(&mut result as *mut usize),
        );
        if sent.0 == 0 {
            report_error("Request wallpaper host window", "Windows message failed or timed out");
        }

        if let Err(error) = EnumWindows(Some(enum_windows_proc), LPARAM(0)) {
            if WORKERW_HWND.0.is_null() { report_error("Find wallpaper host window", error); }
        }

        let target_parent = if !WORKERW_HWND.0.is_null() {
            WORKERW_HWND
        } else {
            progman
        };

        if let Ok(tauri_hwnd) = window.hwnd()
            .inspect_err(|error| { report_error("Get application window handle", error); }) {
            let hwnd = HWND(tauri_hwnd.0 as *mut _);

            let mut work_area = RECT::default();
            if let Err(error) = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut work_area as *mut _ as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            ) {
                report_error("Get desktop work area", error);
                return;
            }

            let screen_width = GetSystemMetrics(SM_CXSCREEN);
            let work_height = work_area.bottom - work_area.top;

            SetLastError(WIN32_ERROR(0));
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            if style == 0 && GetLastError().0 != 0 {
                report_error("Get wallpaper window style", windows::core::Error::from_win32());
                return;
            }
            let new_style = (style & !(WS_POPUP.0 as isize)) | (WS_CHILD.0 as isize) | (WS_VISIBLE.0 as isize);
            SetLastError(WIN32_ERROR(0));
            if SetWindowLongPtrW(hwnd, GWL_STYLE, new_style) == 0 && GetLastError().0 != 0 {
                report_error("Set wallpaper window style", windows::core::Error::from_win32());
                return;
            }

            // A null previous parent is valid for a top-level window. The Windows
            // wrapper reports that as Err, so consult last-error as Win32 requires.
            SetLastError(WIN32_ERROR(0));
            if let Err(error) = SetParent(hwnd, target_parent) {
                if GetLastError().0 != 0 {
                    report_error("Attach wallpaper window", error);
                    return;
                }
            }

            if let Err(error) = SetWindowPos(
                hwnd,
                HWND::default(),
                0,
                0,
                screen_width,
                work_height,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            ) { report_error("Position wallpaper window", error); }
        }
    }
}
