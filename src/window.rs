//! Windows borderless custom chrome (same model as PinkCode).
//!
//! The viewport is created with `decorations(false)` so DWM does not paint a
//! system caption, caption buttons, or the residual 1px top frame line that
//! appears when only `WS_CAPTION` is stripped from a decorated window.
//!
//! Title drag / min / max / close are drawn by the app. Edge resize uses egui
//! interact zones + `ViewportCommand::BeginResize`. The top edge middle is left
//! clear so the toolbar title-drag region keeps StartDrag / double-click zoom.
//!
//! egui-winit treats every `WindowEvent::Moved` as "repaint now". A title-bar
//! drag therefore rebuilds the editor + preview on each mouse sample. The
//! WndProc filter drops move-only `WM_WINDOWPOSCHANGED` so DWM slides the last
//! frame; size-changing moves still reach winit (snap / edge resize).

#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[cfg(target_os = "windows")]
use eframe::egui::{
    self, CursorIcon, Id, Order, Pos2, Rect, ResizeDirection, Sense, Vec2, ViewportCommand,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};

#[cfg(target_os = "windows")]
static CHROME_CONFIGURED: AtomicBool = AtomicBool::new(false);

/// Previous WndProc as `usize`; `0` until [`install_move_filter`] succeeds.
#[cfg(target_os = "windows")]
static ORIG_WNDPROC: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_os = "windows")]
type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// Edge strip thickness (logical points).
#[cfg(target_os = "windows")]
const RESIZE_BORDER: f32 = 6.0;

/// Corner grab size; larger than the edge strip for easier diagonal resize.
/// Top-middle is intentionally not a resize zone (see module docs).
#[cfg(target_os = "windows")]
const RESIZE_CORNER: f32 = 12.0;

/// One entry for Windows chrome: DWM polish, move-repaint filter, resize zones.
///
/// Call once per frame from `App::update` **before** main UI so edge zones
/// participate in the same interact pass as the toolbar.
#[cfg(target_os = "windows")]
pub fn frame_chrome(ctx: &egui::Context, window: &impl raw_window_handle::HasWindowHandle) {
    configure_chrome_once(window);
    handle_resize(ctx);
}

#[cfg(target_os = "windows")]
fn configure_chrome_once(window: &impl raw_window_handle::HasWindowHandle) {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };

    if CHROME_CONFIGURED.load(Ordering::Relaxed) {
        return;
    }

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as *mut core::ffi::c_void;

    // SAFETY: hwnd comes from eframe's live window handle; callers run on the UI thread.
    unsafe {
        let corner_preference = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&corner_preference as *const _) as *const core::ffi::c_void,
            std::mem::size_of_val(&corner_preference) as u32,
        );

        // Hide any system window border stroke (Win11).
        let border_color = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            (&border_color as *const _) as *const core::ffi::c_void,
            std::mem::size_of_val(&border_color) as u32,
        );

        let dark_mode: i32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&dark_mode as *const _) as *const core::ffi::c_void,
            std::mem::size_of_val(&dark_mode) as u32,
        );
    }

    install_move_filter(hwnd);
    CHROME_CONFIGURED.store(true, Ordering::Relaxed);
}

/// Register invisible edge/corner drag zones for OS-style resize.
///
/// Top edge: only NW/NE corners — the center stays free for title-bar drag.
#[cfg(target_os = "windows")]
fn handle_resize(ctx: &egui::Context) {
    if ctx.input(|input| input.viewport().maximized.unwrap_or(false)) {
        return;
    }

    let screen = ctx.screen_rect();
    let b = RESIZE_BORDER;
    let c = RESIZE_CORNER;
    let left = screen.left();
    let right = screen.right();
    let top = screen.top();
    let bottom = screen.bottom();
    let width = screen.width();
    let height = screen.height();

    // Corners first so they win over adjacent edge strips.
    resize_zone(
        ctx,
        "resize-nw",
        Rect::from_min_size(Pos2::new(left, top), Vec2::splat(c)),
        ResizeDirection::NorthWest,
        CursorIcon::ResizeNwSe,
    );
    resize_zone(
        ctx,
        "resize-ne",
        Rect::from_min_size(Pos2::new(right - c, top), Vec2::splat(c)),
        ResizeDirection::NorthEast,
        CursorIcon::ResizeNeSw,
    );
    resize_zone(
        ctx,
        "resize-sw",
        Rect::from_min_size(Pos2::new(left, bottom - c), Vec2::splat(c)),
        ResizeDirection::SouthWest,
        CursorIcon::ResizeNeSw,
    );
    resize_zone(
        ctx,
        "resize-se",
        Rect::from_min_size(Pos2::new(right - c, bottom - c), Vec2::splat(c)),
        ResizeDirection::SouthEast,
        CursorIcon::ResizeNwSe,
    );

    // Side edges (between corners).
    let side_height = (height - 2.0 * c).max(0.0);
    if side_height > 0.0 {
        resize_zone(
            ctx,
            "resize-w",
            Rect::from_min_size(Pos2::new(left, top + c), Vec2::new(b, side_height)),
            ResizeDirection::West,
            CursorIcon::ResizeHorizontal,
        );
        resize_zone(
            ctx,
            "resize-e",
            Rect::from_min_size(Pos2::new(right - b, top + c), Vec2::new(b, side_height)),
            ResizeDirection::East,
            CursorIcon::ResizeHorizontal,
        );
    }

    // Bottom edge only — top-middle is reserved for title drag / double-click zoom.
    let bottom_width = (width - 2.0 * c).max(0.0);
    if bottom_width > 0.0 {
        resize_zone(
            ctx,
            "resize-s",
            Rect::from_min_size(Pos2::new(left + c, bottom - b), Vec2::new(bottom_width, b)),
            ResizeDirection::South,
            CursorIcon::ResizeVertical,
        );
    }
}

#[cfg(target_os = "windows")]
fn resize_zone(
    ctx: &egui::Context,
    id: &'static str,
    rect: Rect,
    direction: ResizeDirection,
    cursor: CursorIcon,
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }

    let response = egui::Area::new(Id::new(id))
        .fixed_pos(rect.min)
        .order(Order::Foreground)
        .sense(Sense::drag())
        .default_size(rect.size())
        .constrain(false)
        .show(ctx, |ui| ui.allocate_exact_size(rect.size(), Sense::drag()).1)
        .inner;

    if response.hovered() || response.is_pointer_button_down_on() {
        ctx.set_cursor_icon(cursor);
    }
    if response.drag_started() {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

/// True when `WM_WINDOWPOSCHANGED` is a move that does not change size.
///
/// Those are the events that make egui-winit request a full frame during
/// title drag. Resize-with-move (snap, edge drag) must still reach winit so
/// the GL surface is resized and the window does not flicker.
#[cfg(target_os = "windows")]
fn is_move_only(windowpos_flags: u32) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SWP_NOMOVE, SWP_NOSIZE};
    windowpos_flags & SWP_NOMOVE == 0 && windowpos_flags & SWP_NOSIZE != 0
}

#[cfg(target_os = "windows")]
fn install_move_filter(hwnd: HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWLP_WNDPROC,
    };

    // Store the current proc *before* swapping. A message that arrives the
    // instant after SetWindowLongPtrW can then still chain to winit.
    // SAFETY: hwnd is eframe's live window.
    let prev = unsafe { GetWindowLongPtrW(hwnd, GWLP_WNDPROC) };
    if prev == 0 {
        return;
    }
    ORIG_WNDPROC.store(prev as usize, Ordering::Release);
    let replaced = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, move_filter_wndproc as isize) };
    if replaced == 0 {
        ORIG_WNDPROC.store(0, Ordering::Release);
        return;
    }
    if replaced as usize != prev as usize {
        ORIG_WNDPROC.store(replaced as usize, Ordering::Release);
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn move_filter_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, WM_WINDOWPOSCHANGED, WINDOWPOS,
    };

    if msg == WM_WINDOWPOSCHANGED && lparam != 0 {
        // SAFETY: lparam is the WINDOWPOS* Windows documents for this message.
        let flags = unsafe { (*(lparam as *const WINDOWPOS)).flags };
        if is_move_only(flags) {
            // Skip winit so it never emits WindowEvent::Moved.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
    }

    call_orig(hwnd, msg, wparam, lparam)
}

#[cfg(target_os = "windows")]
fn call_orig(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CallWindowProcW, DefWindowProcW};

    let orig = ORIG_WNDPROC.load(Ordering::Acquire);
    if orig == 0 {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // SAFETY: orig is the WndProc GetWindowLongPtrW returned before we swapped.
    unsafe { CallWindowProcW(Some(std::mem::transmute::<usize, WndProc>(orig)), hwnd, msg, wparam, lparam) }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::is_move_only;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SWP_NOMOVE, SWP_NOSIZE};

    #[test]
    fn suppresses_only_move_without_resize() {
        assert!(is_move_only(SWP_NOSIZE));
        assert!(!is_move_only(0));
        assert!(!is_move_only(SWP_NOMOVE));
        assert!(!is_move_only(SWP_NOSIZE | SWP_NOMOVE));
    }
}
