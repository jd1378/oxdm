//! Borderless-window integration for Windows.
//!
//! eframe runs the main viewport with `decorations: false`, which
//! removes the OS-provided non-client frame. Without that frame the
//! desktop manager no longer:
//!   1. lets you resize by grabbing the edges, and
//!   2. sets the diagonal/horizontal/vertical resize cursors when the
//!      pointer enters those edges.
//!
//! Both behaviours are driven by `WM_NCHITTEST`: the window subclass
//! installed here intercepts that message and returns the appropriate
//! `HT*` code for thin bands along each edge / corner. From the OS's
//! point of view those bands are non-client area, so DWM handles the
//! resize loop and the cursor for free — exactly the behaviour
//! Tauri's `wry` runtime ships for its borderless windows.
//!
//! Everything is best-effort: if the window handle cannot be obtained
//! or the Win32 calls fail we leave the window untouched and the
//! existing egui overlay (`utils::resize`) keeps working as a
//! fallback.

use std::ptr;
use std::sync::atomic::{AtomicIsize, Ordering};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{KEYEVENTF_KEYUP, VK_MENU, keybd_event};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CallWindowProcW, GWLP_WNDPROC, GetClientRect, HTBOTTOM, HTBOTTOMLEFT,
    HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, IsIconic, IsZoomed, SW_RESTORE,
    SW_SHOW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, WM_NCHITTEST,
};

/// Width in *physical* pixels of the resize hit-band on each edge.
/// Matches the visual constants in `utils::resize` so the user
/// experience is identical between platforms.
const EDGE: i32 = 6;
/// Side length of the corner squares (priority over edge bands).
const CORNER: i32 = 14;

/// Original WNDPROC, stored so we can chain the default behaviour.
/// `0` means "not installed" — we never call through a null pointer.
static ORIG_WNDPROC: AtomicIsize = AtomicIsize::new(0);
/// HWND we subclassed. We only support a single main window and use
/// this to ignore install attempts for other handles.
static OWNED_HWND: AtomicIsize = AtomicIsize::new(0);

/// Install the borderless-resize subclass on the given window's HWND.
/// Idempotent: subsequent calls for the same HWND are no-ops, calls
/// for a different HWND are ignored. All failures are logged and
/// swallowed so the caller can rely on the egui overlay fallback.
pub fn install_resize_subclass(handle: &impl HasWindowHandle) {
    let hwnd = match resolve_hwnd(handle) {
        Some(h) => h,
        None => return,
    };

    let owned = OWNED_HWND.load(Ordering::Acquire);
    if owned == hwnd as isize {
        return; // already installed for this window
    }
    if owned != 0 {
        // Different HWND — likely a secondary viewport. Leave it alone.
        return;
    }

    // SAFETY: `subclass_proc` is `unsafe extern "system"` and matches
    // the WNDPROC ABI. `SetWindowLongPtrW` returns the previous proc;
    // we keep it for chaining. A return of 0 with `GetLastError != 0`
    // would mean failure, but distinguishing that from a window with
    // no prior proc is unreliable, so we simply guard the chain at
    // call time by checking for a non-null pointer.
    let new_proc = subclass_proc as usize as isize;
    let prev = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, new_proc) };
    if prev == 0 {
        tracing::warn!(
            "WM_NCHITTEST subclass install: SetWindowLongPtrW returned 0; falling back to egui overlay"
        );
        return;
    }

    ORIG_WNDPROC.store(prev, Ordering::Release);
    OWNED_HWND.store(hwnd as isize, Ordering::Release);
    tracing::debug!("installed borderless resize subclass on HWND {hwnd:?}");
}

/// Force the given window to the foreground. Daemon-spawned GUI
/// subprocesses inherit no foreground rights on Windows, so eframe's
/// `ViewportCommand::Focus` is a no-op and the new window opens behind
/// whatever currently owns the foreground (taskbar flash only).
///
/// The daemon side calls `AllowSetForegroundWindow(child_pid)` before
/// spawn to grant the child permission; this helper is the matching
/// child-side call. Best-effort: failures are ignored, the user can
/// still click the taskbar entry.
pub fn bring_to_foreground(handle: &impl HasWindowHandle) {
    let Some(hwnd) = resolve_hwnd(handle) else {
        return;
    };
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        } else {
            ShowWindow(hwnd, SW_SHOW);
        }

        // Bypass focus-stealing prevention without coupling our input
        // queue to another process: synthesize a no-op ALT keypress so
        // GetLastInputInfo records this process as the most recent
        // input source, after which Windows allows us to call
        // SetForegroundWindow. AttachThreadInput would also work but
        // leaves drag / mouse-capture state shared with the foreground
        // thread, and on the download path that thread is the
        // just-evicted previous download window — once it terminates
        // before we detach, our window is left in a half-attached state
        // that breaks titlebar dragging until a minimize/restore cycle
        // resets it.
        keybd_event(VK_MENU as u8, 0, 0, 0);
        keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);

        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
    }
}

fn resolve_hwnd(handle: &impl HasWindowHandle) -> Option<HWND> {
    let h = handle.window_handle().ok()?;
    match h.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get() as HWND),
        _ => None,
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCHITTEST
        && let Some(code) = unsafe { hit_test(hwnd, lparam) }
    {
        return code;
    }
    unsafe { chain(hwnd, msg, wparam, lparam) }
}

unsafe fn chain(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let prev = ORIG_WNDPROC.load(Ordering::Acquire);
    if prev == 0 {
        // Should never happen — subclass_proc only runs after install
        // wrote a non-zero value — but stay defensive.
        return 0;
    }
    // SAFETY: `prev` is the WNDPROC value Windows returned from
    // SetWindowLongPtrW, so transmuting it back to a WNDPROC option
    // is sound. `CallWindowProcW` accepts `WNDPROC` (Option<fn>) and
    // forwards correctly even for class-default procs.
    unsafe {
        let prev_proc: windows_sys::Win32::UI::WindowsAndMessaging::WNDPROC =
            std::mem::transmute(prev);
        CallWindowProcW(prev_proc, hwnd, msg, wparam, lparam)
    }
}

unsafe fn hit_test(hwnd: HWND, lparam: LPARAM) -> Option<LRESULT> {
    // Don't interfere with maximised windows: edges are clamped to
    // the work area and a "resize" there confuses DWM.
    if unsafe { IsZoomed(hwnd) } != 0 {
        return None;
    }

    // LPARAM packs (y << 16) | x as signed 16-bit screen coords.
    let x = (lparam & 0xFFFF) as i16 as i32;
    let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
    let mut pt = POINT { x, y };
    if unsafe { ScreenToClient(hwnd, &mut pt) } == 0 {
        return None;
    }
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetClientRect(hwnd, &mut rc) } == 0 {
        return None;
    }
    let w = rc.right;
    let h = rc.bottom;

    let on_left = pt.x < EDGE;
    let on_right = pt.x >= w - EDGE;
    let on_top = pt.y < EDGE;
    let on_bottom = pt.y >= h - EDGE;
    let near_left = pt.x < CORNER;
    let near_right = pt.x >= w - CORNER;
    let near_top = pt.y < CORNER;
    let near_bottom = pt.y >= h - CORNER;

    // Corners take priority. The corner zone extends `CORNER` pixels
    // along each edge so the diagonal grip is comfortable to grab,
    // matching the egui overlay sizing.
    let code = if (on_top && near_left) || (on_left && near_top) {
        HTTOPLEFT
    } else if (on_top && near_right) || (on_right && near_top) {
        HTTOPRIGHT
    } else if (on_bottom && near_left) || (on_left && near_bottom) {
        HTBOTTOMLEFT
    } else if (on_bottom && near_right) || (on_right && near_bottom) {
        HTBOTTOMRIGHT
    } else if on_top {
        HTTOP
    } else if on_bottom {
        HTBOTTOM
    } else if on_left {
        HTLEFT
    } else if on_right {
        HTRIGHT
    } else {
        return None;
    };

    Some(code as LRESULT)
}

// Silence unused-import lint when the subclass is compiled but
// never wired (e.g. another bin in the workspace).
#[allow(dead_code)]
fn _unused_ptr_silencer() {
    let _ = ptr::null::<()>();
}
