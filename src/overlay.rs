//! Lightshot-style fullscreen crop overlay with floating action toolbar.
//!
//! Features:
//! - Instant double-buffered GDI rendering (bright original + dimmed backdrop, zero flicker)
//! - Drag to select region, drag inside to move, drag 8 corner/edge handles to resize
//! - Dimensions badge (`W × H`) above selection
//! - Floating toolbar on release: Copiar (`Ctrl+C`/`Enter`), Salvar (`Ctrl+S`), Tela Toda (`F`), Cancelar (`Esc`)
//! - Double-click inside selection copies immediately

use std::cell::RefCell;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection,
    CreateFontW, CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint,
    FillRect, InvalidateRect, Rectangle, RoundRect, SelectObject, SetBkMode, SetTextColor,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
    DEFAULT_QUALITY, DIB_RGB_COLORS, DT_CENTER, DT_SINGLELINE, DT_VCENTER, FF_SWISS, FW_SEMIBOLD,
    HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, HPEN, NULL_BRUSH, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, SetFocus, VK_C, VK_CONTROL, VK_ESCAPE, VK_F,
    VK_RETURN, VK_S,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, LoadCursorW, RegisterClassW,
    SetCursor, SetForegroundWindow, SetWindowPos, ShowWindow, CREATESTRUCTW, CS_DBLCLKS,
    CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HWND_TOPMOST, IDC_ARROW, IDC_CROSS, IDC_HAND,
    IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, SWP_SHOWWINDOW, SW_SHOW,
    WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW};

use crate::capture::CapturedScreen;
use crate::clipboard::{copy_crop_to_clipboard, prompt_save_png_dialog, save_crop_png};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragMode {
    None,
    Creating,
    Moving,
    ResizeNW,
    ResizeN,
    ResizeNE,
    ResizeE,
    ResizeSE,
    ResizeS,
    ResizeSW,
    ResizeW,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolbarBtn {
    Copy,
    Save,
    Full,
    Cancel,
}

struct ButtonRect {
    kind: ToolbarBtn,
    rect: RECT,
    label: &'static str,
}

struct OverlayState {
    screen: CapturedScreen,
    bmp_bright: HBITMAP,
    bmp_dim: HBITMAP,
    font_ui: HFONT,
    has_selection: bool,
    sel: RECT,
    drag_mode: DragMode,
    anchor: POINT,
    orig_sel: RECT,
    hover_btn: Option<ToolbarBtn>,
    buttons: Vec<ButtonRect>,
    on_close_cb: Option<Box<dyn FnOnce()>>,
}

impl OverlayState {
    fn normalized_sel(&self) -> RECT {
        RECT {
            left: self.sel.left.min(self.sel.right).clamp(0, self.screen.width as i32),
            top: self.sel.top.min(self.sel.bottom).clamp(0, self.screen.height as i32),
            right: self.sel.left.max(self.sel.right).clamp(0, self.screen.width as i32),
            bottom: self.sel.top.max(self.sel.bottom).clamp(0, self.screen.height as i32),
        }
    }

    fn sel_valid(&self) -> bool {
        if !self.has_selection {
            return false;
        }
        let r = self.normalized_sel();
        (r.right - r.left) >= 4 && (r.bottom - r.top) >= 4
    }

    fn hit_handle(&self, x: i32, y: i32) -> DragMode {
        if !self.sel_valid() {
            return DragMode::None;
        }
        let r = self.normalized_sel();
        let pad = 8;
        let near_l = (x - r.left).abs() <= pad;
        let near_r = (x - r.right).abs() <= pad;
        let near_t = (y - r.top).abs() <= pad;
        let near_b = (y - r.bottom).abs() <= pad;
        let in_x = x >= r.left - pad && x <= r.right + pad;
        let in_y = y >= r.top - pad && y <= r.bottom + pad;

        if near_l && near_t {
            DragMode::ResizeNW
        } else if near_r && near_t {
            DragMode::ResizeNE
        } else if near_l && near_b {
            DragMode::ResizeSW
        } else if near_r && near_b {
            DragMode::ResizeSE
        } else if near_t && in_x {
            DragMode::ResizeN
        } else if near_b && in_x {
            DragMode::ResizeS
        } else if near_l && in_y {
            DragMode::ResizeW
        } else if near_r && in_y {
            DragMode::ResizeE
        } else if x >= r.left && x <= r.right && y >= r.top && y <= r.bottom {
            DragMode::Moving
        } else {
            DragMode::None
        }
    }

    fn compute_toolbar(&mut self) {
        self.buttons.clear();
        if !self.sel_valid() || self.drag_mode != DragMode::None {
            return;
        }
        let r = self.normalized_sel();
        let sw = self.screen.width as i32;
        let sh = self.screen.height as i32;

        let specs = [
            (ToolbarBtn::Copy, "Copiar (Ctrl+C)", 126),
            (ToolbarBtn::Save, "Salvar (Ctrl+S)", 122),
            (ToolbarBtn::Full, "Tela toda (F)", 104),
            (ToolbarBtn::Cancel, "Cancelar (Esc)", 110),
        ];
        let gap = 6;
        let pad_x = 8;
        let pad_y = 6;
        let btn_h = 30;
        let total_w: i32 = specs.iter().map(|s| s.2).sum::<i32>()
            + gap * (specs.len() as i32 - 1)
            + pad_x * 2;
        let bar_h = btn_h + pad_y * 2;

        // Place right below selection, or above if near bottom edge, or inside if full screen
        let mut bar_x = (r.right - total_w).max(r.left);
        bar_x = bar_x.clamp(10, (sw - total_w - 10).max(10));
        let bar_y = if r.bottom + 8 + bar_h <= sh - 8 {
            r.bottom + 8
        } else if r.top - 8 - bar_h >= 8 {
            r.top - 8 - bar_h
        } else {
            (r.bottom - bar_h - 12).max(12)
        };

        let mut cx = bar_x + pad_x;
        let cy = bar_y + pad_y;
        for (kind, label, w) in specs {
            self.buttons.push(ButtonRect {
                kind,
                rect: RECT {
                    left: cx,
                    top: cy,
                    right: cx + w,
                    bottom: cy + btn_h,
                },
                label,
            });
            cx += w + gap;
        }
    }

    fn hit_toolbar(&self, x: i32, y: i32) -> Option<ToolbarBtn> {
        for b in &self.buttons {
            if x >= b.rect.left && x <= b.rect.right && y >= b.rect.top && y <= b.rect.bottom {
                return Some(b.kind);
            }
        }
        None
    }
}

impl Drop for OverlayState {
    fn drop(&mut self) {
        unsafe {
            if !self.bmp_bright.is_invalid() {
                let _ = DeleteObject(self.bmp_bright);
            }
            if !self.bmp_dim.is_invalid() {
                let _ = DeleteObject(self.bmp_dim);
            }
            if !self.font_ui.is_invalid() {
                let _ = DeleteObject(self.font_ui);
            }
        }
        if let Some(cb) = self.on_close_cb.take() {
            cb();
        }
    }
}

fn create_topdown_dib(width: u32, height: u32, bgra: &[u8]) -> HBITMAP {
    unsafe {
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let hbmp = CreateDIBSection(HDC::default(), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .unwrap_or_default();
        if !hbmp.is_invalid() && !bits.is_null() {
            std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
        }
        hbmp
    }
}

fn make_dimmed_bgra(bgra: &[u8]) -> Vec<u8> {
    let mut dim = vec![0u8; bgra.len()];
    for (src, dst) in bgra.chunks_exact(4).zip(dim.chunks_exact_mut(4)) {
        // Darken to ~45% brightness for clean Lightshot-style backdrop
        dst[0] = ((src[0] as u16 * 115) >> 8) as u8;
        dst[1] = ((src[1] as u16 * 115) >> 8) as u8;
        dst[2] = ((src[2] as u16 * 115) >> 8) as u8;
        dst[3] = 255;
    }
    dim
}

thread_local! {
    static CLASS_REGISTERED: RefCell<bool> = const { RefCell::new(false) };
}

fn ensure_overlay_class(hinst: HINSTANCE) {
    CLASS_REGISTERED.with(|flag| {
        if *flag.borrow() {
            return;
        }
        let class_name: Vec<u16> = "HdrSnipOverlayWnd\0".encode_utf16().collect();
        unsafe {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
                lpfnWndProc: Some(overlay_wnd_proc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            RegisterClassW(&wc);
        }
        *flag.borrow_mut() = true;
    });
}

/// Opens the fullscreen Lightshot-style crop overlay for `screen`.
pub fn show_overlay(screen: CapturedScreen, on_close: impl FnOnce() + 'static) {
    let hinst: HINSTANCE = unsafe { GetModuleHandleW(None).unwrap_or_default().into() };
    ensure_overlay_class(hinst);

    let bmp_bright = create_topdown_dib(screen.width, screen.height, &screen.bgra);
    let dim_bgra = make_dimmed_bgra(&screen.bgra);
    let bmp_dim = create_topdown_dib(screen.width, screen.height, &dim_bgra);

    let font_face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
    let font_ui = unsafe {
        let mut face = [0u16; 32];
        let n = font_face.len().min(32);
        face[..n].copy_from_slice(&font_face[..n]);
        CreateFontW(
            -13,
            0,
            0,
            0,
            FW_SEMIBOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            DEFAULT_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_SWISS.0) as u32,
            PCWSTR(face.as_ptr()),
        )
    };

    let mon = screen.monitor_rect;
    let state = Box::new(OverlayState {
        screen,
        bmp_bright,
        bmp_dim,
        font_ui,
        has_selection: false,
        sel: RECT::default(),
        drag_mode: DragMode::None,
        anchor: POINT::default(),
        orig_sel: RECT::default(),
        hover_btn: None,
        buttons: Vec::new(),
        on_close_cb: Some(Box::new(on_close)),
    });
    let state_ptr = Box::into_raw(state);

    let class_name: Vec<u16> = "HdrSnipOverlayWnd\0".encode_utf16().collect();
    let title: Vec<u16> = "HDR Snip\0".encode_utf16().collect();
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            mon.left,
            mon.top,
            mon.right - mon.left,
            mon.bottom - mon.top,
            HWND::default(),
            None,
            hinst,
            Some(state_ptr as *const core::ffi::c_void),
        );
        if let Ok(hwnd) = hwnd {
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                mon.left,
                mon.top,
                mon.right - mon.left,
                mon.bottom - mon.top,
                SWP_SHOWWINDOW,
            );
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(hwnd);
        } else {
            // Reclaim state if window creation failed
            let _ = Box::from_raw(state_ptr);
        }
    }
}

fn get_state<'a>(hwnd: HWND) -> Option<&'a mut OverlayState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut OverlayState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

fn execute_action(hwnd: HWND, st: &mut OverlayState, action: ToolbarBtn) {
    match action {
        ToolbarBtn::Copy => {
            if st.sel_valid() {
                let r = st.normalized_sel();
                let _ = copy_crop_to_clipboard(&st.screen, r);
            }
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        ToolbarBtn::Save => {
            if st.sel_valid() {
                let r = st.normalized_sel();
                // Hide topmost overlay momentarily so Save dialog is unobstructed
                unsafe {
                    let _ = ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
                }
                if let Some(path) = prompt_save_png_dialog(HWND::default()) {
                    let _ = save_crop_png(&st.screen, r, &path);
                    let _ = copy_crop_to_clipboard(&st.screen, r);
                }
            }
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        ToolbarBtn::Full => {
            st.has_selection = true;
            st.sel = RECT {
                left: 0,
                top: 0,
                right: st.screen.width as i32,
                bottom: st.screen.height as i32,
            };
            st.drag_mode = DragMode::None;
            st.compute_toolbar();
            unsafe {
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
        ToolbarBtn::Cancel => unsafe {
            let _ = DestroyWindow(hwnd);
        },
    }
}

fn paint_overlay(hwnd: HWND, st: &OverlayState) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let w = st.screen.width as i32;
        let h = st.screen.height as i32;

        // Offscreen double buffer (zero flicker)
        let mem_dc = CreateCompatibleDC(hdc);
        let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_mem = SelectObject(mem_dc, mem_bmp);

        let src_dc = CreateCompatibleDC(hdc);
        // 1) Dimmed full screen
        let old_src = SelectObject(src_dc, st.bmp_dim);
        let _ = BitBlt(mem_dc, 0, 0, w, h, src_dc, 0, 0, SRCCOPY);

        // 2) Bright cropped selection
        if st.sel_valid() {
            let r = st.normalized_sel();
            SelectObject(src_dc, st.bmp_bright);
            let _ = BitBlt(
                mem_dc,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                src_dc,
                r.left,
                r.top,
                SRCCOPY,
            );

            // Selection border (crisp sky-blue + white inner highlight)
            let pen_outer: HPEN = CreatePen(PS_SOLID, 2, COLORREF(0x00F8BD38)); // #38BDF8 sky
            let old_pen = SelectObject(mem_dc, pen_outer);
            let old_brush = SelectObject(
                mem_dc,
                windows::Win32::Graphics::Gdi::GetStockObject(NULL_BRUSH),
            );
            let _ = Rectangle(mem_dc, r.left, r.top, r.right, r.bottom);
            SelectObject(mem_dc, old_brush);
            SelectObject(mem_dc, old_pen);
            let _ = DeleteObject(pen_outer);

            // 8 resize handles
            let handle_brush: HBRUSH = CreateSolidBrush(COLORREF(0x00FFFFFF));
            let handle_pen: HPEN = CreatePen(PS_SOLID, 1, COLORREF(0x00E5464F));
            let op = SelectObject(mem_dc, handle_pen);
            let ob = SelectObject(mem_dc, handle_brush);
            let mx = (r.left + r.right) / 2;
            let my = (r.top + r.bottom) / 2;
            for (hx, hy) in [
                (r.left, r.top),
                (mx, r.top),
                (r.right, r.top),
                (r.right, my),
                (r.right, r.bottom),
                (mx, r.bottom),
                (r.left, r.bottom),
                (r.left, my),
            ] {
                let _ = Rectangle(mem_dc, hx - 4, hy - 4, hx + 5, hy + 5);
            }
            SelectObject(mem_dc, ob);
            SelectObject(mem_dc, op);
            let _ = DeleteObject(handle_brush);
            let _ = DeleteObject(handle_pen);

            // Dimensions pill (e.g. "842 × 416 · HDR 240 nits")
            let dim_text = if st.screen.hdr_info.hdr_enabled {
                format!(
                    "{} × {}  ·  HDR→sRGB",
                    r.right - r.left,
                    r.bottom - r.top
                )
            } else {
                format!("{} × {}", r.right - r.left, r.bottom - r.top)
            };
            let pill_w = 172;
            let pill_h = 24;
            let px = r.left.clamp(8, (w - pill_w - 8).max(8));
            let py = if r.top >= pill_h + 8 {
                r.top - pill_h - 6
            } else {
                r.top + 6
            };
            let pill_rect = RECT {
                left: px,
                top: py,
                right: px + pill_w,
                bottom: py + pill_h,
            };
            let pill_bg = CreateSolidBrush(COLORREF(0x00221811));
            let pill_pen = CreatePen(PS_SOLID, 1, COLORREF(0x00524133));
            let op2 = SelectObject(mem_dc, pill_pen);
            let ob2 = SelectObject(mem_dc, pill_bg);
            let _ = RoundRect(mem_dc, pill_rect.left, pill_rect.top, pill_rect.right, pill_rect.bottom, 8, 8);
            SelectObject(mem_dc, ob2);
            SelectObject(mem_dc, op2);
            let _ = DeleteObject(pill_bg);
            let _ = DeleteObject(pill_pen);

            let old_font = SelectObject(mem_dc, st.font_ui);
            SetBkMode(mem_dc, TRANSPARENT);
            SetTextColor(mem_dc, COLORREF(0x00F5EFEA));
            let mut dim_wide: Vec<u16> = dim_text.encode_utf16().collect();
            let mut tr = pill_rect;
            DrawTextW(mem_dc, &mut dim_wide, &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            SelectObject(mem_dc, old_font);
        } else {
            // Top hint bar when no selection yet
            let hint = "Arraste para recortar  ·  F = Tela toda  ·  Esc = Cancelar";
            let hw = 430;
            let hh = 34;
            let hx = (w - hw) / 2;
            let hy = 22;
            let hr = RECT {
                left: hx,
                top: hy,
                right: hx + hw,
                bottom: hy + hh,
            };
            let bg = CreateSolidBrush(COLORREF(0x00241A12));
            let pen = CreatePen(PS_SOLID, 1, COLORREF(0x00F8BD38));
            let op = SelectObject(mem_dc, pen);
            let ob = SelectObject(mem_dc, bg);
            let _ = RoundRect(mem_dc, hr.left, hr.top, hr.right, hr.bottom, 10, 10);
            SelectObject(mem_dc, ob);
            SelectObject(mem_dc, op);
            let _ = DeleteObject(bg);
            let _ = DeleteObject(pen);

            let old_font = SelectObject(mem_dc, st.font_ui);
            SetBkMode(mem_dc, TRANSPARENT);
            SetTextColor(mem_dc, COLORREF(0x00FFFFFF));
            let mut wide: Vec<u16> = hint.encode_utf16().collect();
            let mut tr = hr;
            DrawTextW(mem_dc, &mut wide, &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            SelectObject(mem_dc, old_font);
        }

        // 3) Floating Lightshot-style toolbar
        if !st.buttons.is_empty() {
            let min_l = st.buttons.iter().map(|b| b.rect.left).min().unwrap_or(0) - 8;
            let min_t = st.buttons.iter().map(|b| b.rect.top).min().unwrap_or(0) - 6;
            let max_r = st.buttons.iter().map(|b| b.rect.right).max().unwrap_or(0) + 8;
            let max_b = st.buttons.iter().map(|b| b.rect.bottom).max().unwrap_or(0) + 6;

            let bar_bg = CreateSolidBrush(COLORREF(0x001E1711)); // dark slate
            let bar_pen = CreatePen(PS_SOLID, 1, COLORREF(0x00584534));
            let op = SelectObject(mem_dc, bar_pen);
            let ob = SelectObject(mem_dc, bar_bg);
            let _ = RoundRect(mem_dc, min_l, min_t, max_r, max_b, 10, 10);
            SelectObject(mem_dc, ob);
            SelectObject(mem_dc, op);
            let _ = DeleteObject(bar_bg);
            let _ = DeleteObject(bar_pen);

            let old_font = SelectObject(mem_dc, st.font_ui);
            SetBkMode(mem_dc, TRANSPARENT);

            for b in &st.buttons {
                let hovered = st.hover_btn == Some(b.kind);
                let fill_col = match (b.kind, hovered) {
                    (ToolbarBtn::Copy, true) => COLORREF(0x00F16663),  // indigo bright
                    (ToolbarBtn::Copy, false) => COLORREF(0x00E5464F), // indigo primary
                    (ToolbarBtn::Cancel, true) => COLORREF(0x003D3399),
                    (_, true) => COLORREF(0x00463729),
                    (_, false) => COLORREF(0x0031251B),
                };
                let bb = CreateSolidBrush(fill_col);
                let bp = CreatePen(PS_SOLID, 1, COLORREF(0x00665240));
                let op2 = SelectObject(mem_dc, bp);
                let ob2 = SelectObject(mem_dc, bb);
                let _ = RoundRect(mem_dc, b.rect.left, b.rect.top, b.rect.right, b.rect.bottom, 7, 7);
                SelectObject(mem_dc, ob2);
                SelectObject(mem_dc, op2);
                let _ = DeleteObject(bb);
                let _ = DeleteObject(bp);

                SetTextColor(mem_dc, COLORREF(0x00FFFFFF));
                let mut wide: Vec<u16> = b.label.encode_utf16().collect();
                let mut tr = b.rect;
                DrawTextW(mem_dc, &mut wide, &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            }
            SelectObject(mem_dc, old_font);
        }

        // Copy backbuffer to screen in 1 blit
        let _ = BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);

        SelectObject(src_dc, old_src);
        let _ = DeleteDC(src_dc);
        SelectObject(mem_dc, old_mem);
        let _ = DeleteObject(mem_bmp);
        let _ = DeleteDC(mem_dc);
        let _ = FillRect(HDC::default(), &RECT::default(), HBRUSH::default());
        let _ = EndPaint(hwnd, &ps);
    }
}

fn mouse_xy(lp: LPARAM) -> (i32, i32) {
    let x = (lp.0 & 0xFFFF) as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            if let Some(st) = get_state(hwnd) {
                paint_overlay(hwnd, st);
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if let Some(st) = get_state(hwnd) {
                let mut pt = POINT::default();
                let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
                let x = pt.x - st.screen.monitor_rect.left;
                let y = pt.y - st.screen.monitor_rect.top;
                let cur_id = if st.hit_toolbar(x, y).is_some() {
                    IDC_HAND
                } else {
                    match st.hit_handle(x, y) {
                        DragMode::ResizeNW | DragMode::ResizeSE => IDC_SIZENWSE,
                        DragMode::ResizeNE | DragMode::ResizeSW => IDC_SIZENESW,
                        DragMode::ResizeN | DragMode::ResizeS => IDC_SIZENS,
                        DragMode::ResizeE | DragMode::ResizeW => IDC_SIZEWE,
                        DragMode::Moving => IDC_SIZEALL,
                        _ => {
                            if st.has_selection {
                                IDC_ARROW
                            } else {
                                IDC_CROSS
                            }
                        }
                    }
                };
                if let Ok(cur) = LoadCursorW(None, cur_id) {
                    SetCursor(cur);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDOWN => {
            if let Some(st) = get_state(hwnd) {
                let (x, y) = mouse_xy(lparam);
                if let Some(btn) = st.hit_toolbar(x, y) {
                    execute_action(hwnd, st, btn);
                    return LRESULT(0);
                }
                let hit = st.hit_handle(x, y);
                if hit != DragMode::None {
                    st.drag_mode = hit;
                    st.anchor = POINT { x, y };
                    st.orig_sel = st.normalized_sel();
                    st.sel = st.orig_sel;
                    st.buttons.clear();
                } else {
                    st.has_selection = true;
                    st.drag_mode = DragMode::Creating;
                    st.anchor = POINT { x, y };
                    st.sel = RECT {
                        left: x,
                        top: y,
                        right: x,
                        bottom: y,
                    };
                    st.buttons.clear();
                }
                SetCapture(hwnd);
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some(st) = get_state(hwnd) {
                let (x, y) = mouse_xy(lparam);
                let sw = st.screen.width as i32;
                let sh = st.screen.height as i32;
                let cx = x.clamp(0, sw);
                let cy = y.clamp(0, sh);
                match st.drag_mode {
                    DragMode::None => {
                        let hov = st.hit_toolbar(x, y);
                        if hov != st.hover_btn {
                            st.hover_btn = hov;
                            let _ = InvalidateRect(hwnd, None, false);
                        }
                    }
                    DragMode::Creating => {
                        st.sel.right = cx;
                        st.sel.bottom = cy;
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    DragMode::Moving => {
                        let dx = cx - st.anchor.x;
                        let dy = cy - st.anchor.y;
                        let rw = st.orig_sel.right - st.orig_sel.left;
                        let rh = st.orig_sel.bottom - st.orig_sel.top;
                        let nl = (st.orig_sel.left + dx).clamp(0, (sw - rw).max(0));
                        let nt = (st.orig_sel.top + dy).clamp(0, (sh - rh).max(0));
                        st.sel = RECT {
                            left: nl,
                            top: nt,
                            right: nl + rw,
                            bottom: nt + rh,
                        };
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    DragMode::ResizeNW => {
                        st.sel.left = cx;
                        st.sel.top = cy;
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    DragMode::ResizeN => {
                        st.sel.top = cy;
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    DragMode::ResizeNE => {
                        st.sel.right = cx;
                        st.sel.top = cy;
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    DragMode::ResizeE => {
                        st.sel.right = cx;
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    DragMode::ResizeSE => {
                        st.sel.right = cx;
                        st.sel.bottom = cy;
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    DragMode::ResizeS => {
                        st.sel.bottom = cy;
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    DragMode::ResizeSW => {
                        st.sel.left = cx;
                        st.sel.bottom = cy;
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    DragMode::ResizeW => {
                        st.sel.left = cx;
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(st) = get_state(hwnd) {
                if st.drag_mode != DragMode::None {
                    let _ = ReleaseCapture();
                    st.sel = st.normalized_sel();
                    st.drag_mode = DragMode::None;
                    if !st.sel_valid() {
                        st.has_selection = false;
                        st.buttons.clear();
                    } else {
                        st.compute_toolbar();
                    }
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            if let Some(st) = get_state(hwnd) {
                let (x, y) = mouse_xy(lparam);
                if st.sel_valid() && st.hit_toolbar(x, y).is_none() {
                    let r = st.normalized_sel();
                    if x >= r.left && x <= r.right && y >= r.top && y <= r.bottom {
                        execute_action(hwnd, st, ToolbarBtn::Copy);
                    }
                }
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if let Some(st) = get_state(hwnd) {
                let vk = wparam.0 as u16;
                let ctrl = (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
                if vk == VK_ESCAPE.0 {
                    execute_action(hwnd, st, ToolbarBtn::Cancel);
                } else if vk == VK_RETURN.0 || (ctrl && vk == VK_C.0) {
                    if st.sel_valid() {
                        execute_action(hwnd, st, ToolbarBtn::Copy);
                    }
                } else if ctrl && vk == VK_S.0 {
                    if st.sel_valid() {
                        execute_action(hwnd, st, ToolbarBtn::Save);
                    }
                } else if vk == VK_F.0 {
                    execute_action(hwnd, st, ToolbarBtn::Full);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayState;
            if !ptr.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let _ = Box::from_raw(ptr);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[allow(dead_code)]
fn _unused_gdi_marker(_: HGDIOBJ) {}
