use tauri::WebviewWindow;

#[cfg(target_os = "windows")]
use windows::{
    core::{w, PCWSTR},
    Win32::Foundation::{BOOL, HWND, LPARAM, RECT, WPARAM},
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
            Err(_) => return,
        };

        let mut result: usize = 0;
        let _ = SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0xD),
            LPARAM(0),
            SMTO_NORMAL,
            1000,
            Some(&mut result as *mut usize),
        );

        let _ = EnumWindows(Some(enum_windows_proc), LPARAM(0));

        let target_parent = if !WORKERW_HWND.0.is_null() {
            WORKERW_HWND
        } else {
            progman
        };

        if let Ok(tauri_hwnd) = window.hwnd() {
            let hwnd = HWND(tauri_hwnd.0 as *mut _);

            let mut work_area = RECT::default();
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut work_area as *mut _ as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );

            let screen_width = GetSystemMetrics(SM_CXSCREEN);
            let work_height = work_area.bottom - work_area.top;

            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            let new_style = (style & !(WS_POPUP.0 as isize)) | (WS_CHILD.0 as isize) | (WS_VISIBLE.0 as isize);
            let _ = SetWindowLongPtrW(hwnd, GWL_STYLE, new_style);

            let _ = SetParent(hwnd, target_parent);

            let _ = SetWindowPos(
                hwnd,
                HWND::default(),
                0,
                0,
                screen_width,
                work_height,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}