//! Windows caption hit-testing: without this, the outer pixels of the custom
//! min/max/close buttons fall inside the resize border, so clicking the very
//! top-right corner resizes or does nothing instead of closing the window.

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWCP_DONOTROUND, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, HTCLIENT};

const WM_NCHITTEST: u32 = 0x0084;
const SUBCLASS_ID: usize = 0x5752_4B31; // "WRK1"

/// Make the window square-cornered where supported (Windows 11) and treat the
/// top-right caption strip as client area. Everything else defers to the
/// previous handler, so edge/corner resizing still works.
pub fn install(hwnd: isize) {
    unsafe {
        let hwnd = HWND(hwnd as _);
        // Windows 10 has no rounded corners and rejects the attribute; both
        // outcomes are fine.
        let pref = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&pref) as u32,
        );
        let _ = SetWindowSubclass(hwnd, Some(caption_proc), SUBCLASS_ID, 0);
    }
}

unsafe extern "system" fn caption_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_NCHITTEST {
        let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            let scaled = |v: i32| v * dpi / 96;
            // Settings button + min/max/close + the 1px frame.
            let width = scaled(180);
            let height = scaled(40);
            if x >= rect.right - width && y < rect.top + height {
                return LRESULT(HTCLIENT as isize);
            }
        }
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}
