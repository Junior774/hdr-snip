//! Keycap Minimal fullscreen crop overlay with native anti-aliased SDF rendering.
//!
//! Features:
//! - Cached 32-bit DIB double-buffer (zero allocation on drag, zero flicker)
//! - Pure-Rust analytical SDF anti-aliasing for rounded capsules, soft drop shadows,
//!   double-contrast borders, L-bracket corner handles, and Lucide-style vector icons
//! - Per-Monitor DPI scaling (stays crisp at 100%, 125%, 150%, 200% on any resolution)
//! - Floating Keycap Minimal toolbar: Copiar [↵] · Salvar [S] · Tela [F] | ✕ [Esc]
//! - Direct shortcuts (`Enter`/`C`/`Ctrl+C`, `S`/`Ctrl+S`, `F`, `Esc`) + double-click to copy

use std::cell::RefCell;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject,
    DrawTextW, EndPaint, GdiFlush, GetTextExtentPoint32W, InvalidateRect, MonitorFromPoint,
    SelectObject, SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER, DT_LEFT,
    DT_SINGLELINE, DT_VCENTER, FF_MODERN, FF_SWISS, FIXED_PITCH, FW_MEDIUM, FW_SEMIBOLD, HBITMAP,
    HDC, HFONT, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, VK_C, VK_ESCAPE, VK_F, VK_RETURN, VK_S,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos,
    GetWindowLongPtrW, LoadCursorW, RegisterClassW, SetCursor, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, CREATESTRUCTW, CS_DBLCLKS, CS_HREDRAW,
    CS_VREDRAW, GWLP_USERDATA, HWND_TOPMOST, IDC_ARROW, IDC_CROSS, IDC_HAND, IDC_SIZEALL,
    IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOW,
    WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use crate::capture::CapturedScreen;
use crate::clipboard::{copy_crop_to_clipboard, prompt_save_png_dialog, save_crop_png};
use crate::ui_draw::{Canvas, Rgba};

// Keycap Minimal palette
const COL_SURFACE: Rgba = Rgba::new(17, 19, 23, 244); // #111317 @ 96%
const COL_SURFACE_BORDER: Rgba = Rgba::new(255, 255, 255, 52); // white/20%
const COL_OUTER_HALO: Rgba = Rgba::new(0, 0, 0, 145);
const COL_DIVIDER: Rgba = Rgba::new(255, 255, 255, 42);

const COL_PRIMARY: Rgba = Rgba::rgb(5, 150, 105); // #059669 emerald-600
const COL_PRIMARY_HOVER: Rgba = Rgba::rgb(16, 185, 129); // #10B981 emerald-500
const COL_PRIMARY_RIM: Rgba = Rgba::new(255, 255, 255, 46);
const COL_PRIMARY_KBD_BG: Rgba = Rgba::new(0, 0, 0, 68);
const COL_PRIMARY_KBD_BD: Rgba = Rgba::new(255, 255, 255, 72);

const COL_SEC_HOVER_BG: Rgba = Rgba::new(255, 255, 255, 32);
const COL_SEC_HOVER_BD: Rgba = Rgba::new(255, 255, 255, 26);
const COL_SEC_KBD_BG: Rgba = Rgba::new(255, 255, 255, 26);
const COL_SEC_KBD_BD: Rgba = Rgba::new(255, 255, 255, 40);

const COL_CANCEL_HOVER_BG: Rgba = Rgba::new(244, 63, 94, 58);
const COL_CANCEL_HOVER_BD: Rgba = Rgba::new(251, 113, 133, 96);
const COL_CANCEL_KBD_HOVER_BG: Rgba = Rgba::new(244, 63, 94, 46);
const COL_CANCEL_KBD_HOVER_BD: Rgba = Rgba::new(253, 164, 175, 96);

const COL_ICON_WHITE: Rgba = Rgba::rgb(255, 255, 255);
const COL_ICON_SEC: Rgba = Rgba::rgb(236, 239, 244);
const COL_ICON_MUTED: Rgba = Rgba::rgb(203, 213, 225);
const COL_ICON_ROSE: Rgba = Rgba::rgb(253, 164, 175);

const COL_FRAME_LIGHT: Rgba = Rgba::new(229, 231, 235, 245); // #E5E7EB
const COL_FRAME_DARK: Rgba = Rgba::new(0, 0, 0, 170);
const COL_HDR_DOT: Rgba = Rgba::rgb(16, 185, 129); // #10B981

const fn colorref(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

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

struct ButtonLayout {
    kind: ToolbarBtn,
    rect: RECT,
    label: &'static str,
    keycap: &'static str,
    icon_cx: f32,
    text_x: i32,
    keycap_rect: RECT,
}

struct OverlayState {
    screen: CapturedScreen,
    scale: f32,
    bmp_bright: HBITMAP,
    bmp_dim: HBITMAP,
    bmp_back: HBITMAP,
    back_bits: *mut u8,
    font_ui_bold: HFONT,
    font_ui_med: HFONT,
    font_mono: HFONT,
    font_keycap: HFONT,
    has_selection: bool,
    sel: RECT,
    drag_mode: DragMode,
    anchor: POINT,
    orig_sel: RECT,
    hover_btn: Option<ToolbarBtn>,
    bar_rect: RECT,
    divider_x: i32,
    buttons: Vec<ButtonLayout>,
    on_close_cb: Option<Box<dyn FnOnce()>>,
}

impl OverlayState {
    #[inline]
    fn px(&self, logical: f32) -> i32 {
        (logical * self.scale).round() as i32
    }

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
        let pad = self.px(9.0).max(8);
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

        // Measure labels with the exact GDI fonts for crisp, zero-clipping alignment
        let tmp_dc = unsafe { CreateCompatibleDC(HDC::default()) };
        let w_copy = measure_text_w(tmp_dc, self.font_ui_bold, "Copiar");
        let w_save = measure_text_w(tmp_dc, self.font_ui_med, "Salvar");
        let w_full = measure_text_w(tmp_dc, self.font_ui_med, "Tela");
        let w_kbd_s = measure_text_w(tmp_dc, self.font_keycap, "S");
        let w_kbd_f = measure_text_w(tmp_dc, self.font_keycap, "F");
        let w_kbd_esc = measure_text_w(tmp_dc, self.font_keycap, "Esc");
        unsafe {
            let _ = DeleteDC(tmp_dc);
        }

        let pad_outer = self.px(5.0);
        let btn_h = self.px(34.0);
        let gap = self.px(4.0);
        let div_slot = self.px(11.0);
        let icon_w = self.px(14.0);
        let kbd_h = self.px(19.0);
        let kbd_pad_x = self.px(6.0);

        struct Spec {
            kind: ToolbarBtn,
            label: &'static str,
            keycap: &'static str,
            label_w: i32,
            kbd_w: i32,
            pad_l: i32,
            pad_r: i32,
            gap_icon_lbl: i32,
            gap_lbl_kbd: i32,
        }

        let specs = [
            Spec {
                kind: ToolbarBtn::Copy,
                label: "Copiar",
                keycap: "", // vector return arrow
                label_w: w_copy,
                kbd_w: self.px(21.0),
                pad_l: self.px(11.0),
                pad_r: self.px(9.0),
                gap_icon_lbl: self.px(7.0),
                gap_lbl_kbd: self.px(8.0),
            },
            Spec {
                kind: ToolbarBtn::Save,
                label: "Salvar",
                keycap: "S",
                label_w: w_save,
                kbd_w: (w_kbd_s + kbd_pad_x * 2).max(self.px(19.0)),
                pad_l: self.px(10.0),
                pad_r: self.px(9.0),
                gap_icon_lbl: self.px(6.0),
                gap_lbl_kbd: self.px(7.0),
            },
            Spec {
                kind: ToolbarBtn::Full,
                label: "Tela",
                keycap: "F",
                label_w: w_full,
                kbd_w: (w_kbd_f + kbd_pad_x * 2).max(self.px(19.0)),
                pad_l: self.px(10.0),
                pad_r: self.px(9.0),
                gap_icon_lbl: self.px(6.0),
                gap_lbl_kbd: self.px(7.0),
            },
            Spec {
                kind: ToolbarBtn::Cancel,
                label: "",
                keycap: "Esc",
                label_w: 0,
                kbd_w: (w_kbd_esc + kbd_pad_x * 2).max(self.px(28.0)),
                pad_l: self.px(9.0),
                pad_r: self.px(9.0),
                gap_icon_lbl: 0,
                gap_lbl_kbd: self.px(6.0),
            },
        ];

        let btn_widths: Vec<i32> = specs
            .iter()
            .map(|sp| {
                sp.pad_l
                    + icon_w
                    + if sp.label_w > 0 {
                        sp.gap_icon_lbl + sp.label_w
                    } else {
                        0
                    }
                    + sp.gap_lbl_kbd
                    + sp.kbd_w
                    + sp.pad_r
            })
            .collect();

        // Copy + gap + Save + gap + Full + div_slot + Cancel
        let inner_w = btn_widths[0] + gap + btn_widths[1] + gap + btn_widths[2] + div_slot + btn_widths[3];
        let total_w = inner_w + pad_outer * 2;
        let bar_h = btn_h + pad_outer * 2;
        let margin = self.px(10.0);

        let mut bar_x = (r.right - total_w).max(r.left);
        bar_x = bar_x.clamp(margin, (sw - total_w - margin).max(margin));
        let bar_y = if r.bottom + margin + bar_h <= sh - margin {
            r.bottom + margin
        } else if r.top - margin - bar_h >= margin {
            r.top - margin - bar_h
        } else {
            (r.bottom - bar_h - self.px(14.0)).max(margin)
        };

        self.bar_rect = RECT {
            left: bar_x,
            top: bar_y,
            right: bar_x + total_w,
            bottom: bar_y + bar_h,
        };

        let mut cx = bar_x + pad_outer;
        let cy = bar_y + pad_outer;
        let kbd_y = cy + (btn_h - kbd_h) / 2;

        for (idx, sp) in specs.iter().enumerate() {
            let bw = btn_widths[idx];
            let icon_cx = (cx + sp.pad_l) as f32 + icon_w as f32 * 0.5;
            let text_x = cx + sp.pad_l + icon_w + sp.gap_icon_lbl;
            let kbd_x = cx + bw - sp.pad_r - sp.kbd_w;

            self.buttons.push(ButtonLayout {
                kind: sp.kind,
                rect: RECT {
                    left: cx,
                    top: cy,
                    right: cx + bw,
                    bottom: cy + btn_h,
                },
                label: sp.label,
                keycap: sp.keycap,
                icon_cx,
                text_x,
                keycap_rect: RECT {
                    left: kbd_x,
                    top: kbd_y,
                    right: kbd_x + sp.kbd_w,
                    bottom: kbd_y + kbd_h,
                },
            });

            if idx == 2 {
                self.divider_x = cx + bw + div_slot / 2;
                cx += bw + div_slot;
            } else {
                cx += bw + gap;
            }
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

    /// Renders the complete overlay frame into `self.bmp_back` using `mem_dc`.
    fn render_frame(&self, mem_dc: HDC, src_dc: HDC) {
        let w = self.screen.width as i32;
        let h = self.screen.height as i32;
        let s = self.scale;

        unsafe {
            // 1) Dimmed full screen backdrop
            let old_src = SelectObject(src_dc, self.bmp_dim);
            let _ = BitBlt(mem_dc, 0, 0, w, h, src_dc, 0, 0, SRCCOPY);

            // 2) Bright cropped region
            if self.sel_valid() {
                let r = self.normalized_sel();
                SelectObject(src_dc, self.bmp_bright);
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
            }
            SelectObject(src_dc, old_src);
            let _ = GdiFlush();
        }

        // 3) Pure-Rust anti-aliased SDF rendering pass on the 32-bit DIB backbuffer
        let slice = unsafe {
            std::slice::from_raw_parts_mut(self.back_bits, (w as usize) * (h as usize) * 4)
        };
        let mut canvas = Canvas::new(slice, w, h);

        // Precompute top-hint layout or dimension-pill layout so text pass uses exact rects
        let mut dim_pill_info: Option<(RECT, String, bool)> = None;
        let mut hint_info: Option<HintLayout> = None;

        if self.sel_valid() {
            let r = self.normalized_sel();
            let rw = r.right - r.left;
            let rh = r.bottom - r.top;

            // Selection border: outer dark contrast halo + crisp bright #E5E7EB inner frame
            canvas.fill_rect_i32(r.left - 1, r.top - 1, rw + 2, 1, COL_FRAME_DARK);
            canvas.fill_rect_i32(r.left - 1, r.bottom, rw + 2, 1, COL_FRAME_DARK);
            canvas.fill_rect_i32(r.left - 1, r.top, 1, rh, COL_FRAME_DARK);
            canvas.fill_rect_i32(r.right, r.top, 1, rh, COL_FRAME_DARK);

            let bt = self.px(1.5).max(1);
            canvas.fill_rect_i32(r.left, r.top, rw, bt, COL_FRAME_LIGHT);
            canvas.fill_rect_i32(r.left, r.bottom - bt, rw, bt, COL_FRAME_LIGHT);
            canvas.fill_rect_i32(r.left, r.top, bt, rh, COL_FRAME_LIGHT);
            canvas.fill_rect_i32(r.right - bt, r.top, bt, rh, COL_FRAME_LIGHT);

            // Sleek white corner L-brackets with contrast shadow
            let leg = self.px(13.0).min(rw / 3).min(rh / 3).max(self.px(5.0));
            let thick = self.px(2.5).max(2);
            let out = self.px(1.0).max(1);
            canvas.draw_corner_bracket(r.left - out, r.top - out, 1, 1, leg, thick);
            canvas.draw_corner_bracket(r.right + out - thick, r.top - out, -1, 1, leg, thick);
            canvas.draw_corner_bracket(r.left - out, r.bottom + out - thick, 1, -1, leg, thick);
            canvas.draw_corner_bracket(
                r.right + out - thick,
                r.bottom + out - thick,
                -1,
                -1,
                leg,
                thick,
            );

            // Subtle midpoint edge bars when selection is large enough
            let mid_len = self.px(12.0);
            let mid_t = self.px(2.4).max(2);
            if rw >= self.px(76.0) {
                let mx = (r.left + r.right - mid_len) / 2;
                canvas.fill_rect_i32(mx - 1, r.top - out - 1, mid_len + 2, mid_t + 2, COL_FRAME_DARK);
                canvas.fill_rect_i32(mx, r.top - out, mid_len, mid_t, COL_ICON_WHITE);
                canvas.fill_rect_i32(
                    mx - 1,
                    r.bottom + out - mid_t - 1,
                    mid_len + 2,
                    mid_t + 2,
                    COL_FRAME_DARK,
                );
                canvas.fill_rect_i32(mx, r.bottom + out - mid_t, mid_len, mid_t, COL_ICON_WHITE);
            }
            if rh >= self.px(76.0) {
                let my = (r.top + r.bottom - mid_len) / 2;
                canvas.fill_rect_i32(r.left - out - 1, my - 1, mid_t + 2, mid_len + 2, COL_FRAME_DARK);
                canvas.fill_rect_i32(r.left - out, my, mid_t, mid_len, COL_ICON_WHITE);
                canvas.fill_rect_i32(
                    r.right + out - mid_t - 1,
                    my - 1,
                    mid_t + 2,
                    mid_len + 2,
                    COL_FRAME_DARK,
                );
                canvas.fill_rect_i32(r.right + out - mid_t, my, mid_t, mid_len, COL_ICON_WHITE);
            }

            // Dimensions pill (auto-sized to text)
            let dim_str = format!("{} × {}", rw, rh);
            let hdr = self.screen.hdr_info.hdr_enabled;
            let dim_w = measure_text_w(mem_dc, self.font_mono, &dim_str);
            let hdr_w = if hdr {
                measure_text_w(mem_dc, self.font_ui_med, "HDR→sRGB")
            } else {
                0
            };
            let pad_x = self.px(10.0);
            let dot_slot = if hdr { self.px(16.0) } else { 0 };
            let pill_w = pad_x * 2 + dim_w + dot_slot + hdr_w;
            let pill_h = self.px(25.0);
            let margin = self.px(8.0);
            let px = r.left.clamp(margin, (w - pill_w - margin).max(margin));
            let py = if r.top >= pill_h + margin + self.px(2.0) {
                r.top - pill_h - margin
            } else {
                r.top + margin
            };
            let pr = RECT {
                left: px,
                top: py,
                right: px + pill_w,
                bottom: py + pill_h,
            };

            let pr_rad = 8.0 * s;
            canvas.draw_soft_shadow(
                pr.left as f32,
                pr.top as f32,
                pill_w as f32,
                pill_h as f32,
                pr_rad,
                3.0 * s,
                9.0 * s,
                0.42,
            );
            canvas.stroke_round_rect(
                pr.left as f32 - 1.0,
                pr.top as f32 - 1.0,
                pill_w as f32 + 2.0,
                pill_h as f32 + 2.0,
                pr_rad + 1.0,
                1.0,
                COL_OUTER_HALO,
            );
            canvas.fill_round_rect(
                pr.left as f32,
                pr.top as f32,
                pill_w as f32,
                pill_h as f32,
                pr_rad,
                COL_SURFACE,
            );
            canvas.stroke_round_rect(
                pr.left as f32,
                pr.top as f32,
                pill_w as f32,
                pill_h as f32,
                pr_rad,
                1.0,
                COL_SURFACE_BORDER,
            );

            if hdr {
                let dot_cx = (pr.left + pad_x + dim_w + self.px(8.0)) as f32;
                let dot_cy = pr.top as f32 + pill_h as f32 * 0.5;
                canvas.fill_circle(dot_cx, dot_cy, 2.8 * s, COL_HDR_DOT);
            }
            dim_pill_info = Some((pr, dim_str, hdr));
        } else {
            // Matching Keycap Minimal top hint pill before first drag
            let hl = compute_hint_layout(self, mem_dc, w);
            let hr = hl.bar;
            let hw = (hr.right - hr.left) as f32;
            let hh = (hr.bottom - hr.top) as f32;
            let rad = 11.0 * s;

            canvas.draw_soft_shadow(hr.left as f32, hr.top as f32, hw, hh, rad, 5.0 * s, 14.0 * s, 0.45);
            canvas.stroke_round_rect(
                hr.left as f32 - 1.0,
                hr.top as f32 - 1.0,
                hw + 2.0,
                hh + 2.0,
                rad + 1.0,
                1.0,
                COL_OUTER_HALO,
            );
            canvas.fill_round_rect(hr.left as f32, hr.top as f32, hw, hh, rad, COL_SURFACE);
            canvas.stroke_round_rect(hr.left as f32, hr.top as f32, hw, hh, rad, 1.0, COL_SURFACE_BORDER);

            // Keycap badges for [F] and [Esc] + subtle vertical dividers
            let div_h = self.px(14.0);
            let div_y = hr.top + (hr.bottom - hr.top - div_h) / 2;
            canvas.fill_rect_i32(hl.div1_x, div_y, 1, div_h, COL_DIVIDER);
            canvas.fill_rect_i32(hl.div2_x, div_y, 1, div_h, COL_DIVIDER);

            for kr in [hl.kbd_f, hl.kbd_esc] {
                let kw = (kr.right - kr.left) as f32;
                let kh = (kr.bottom - kr.top) as f32;
                canvas.fill_round_rect(kr.left as f32, kr.top as f32, kw, kh, 4.5 * s, COL_SEC_KBD_BG);
                canvas.stroke_round_rect(kr.left as f32, kr.top as f32, kw, kh, 4.5 * s, 1.0, COL_SEC_KBD_BD);
            }
            hint_info = Some(hl);
        }

        // 4) Keycap Minimal Floating Toolbar (AA capsule, buttons, icons, keycaps)
        if !self.buttons.is_empty() {
            let br = self.bar_rect;
            let bw = (br.right - br.left) as f32;
            let bh = (br.bottom - br.top) as f32;
            let bar_rad = 12.0 * s;

            // Elevation shadow + outer dark contrast ring + dark glass capsule + crisp inner 1px border
            canvas.draw_soft_shadow(br.left as f32, br.top as f32, bw, bh, bar_rad, 6.0 * s, 15.0 * s, 0.50);
            canvas.stroke_round_rect(
                br.left as f32 - 1.0,
                br.top as f32 - 1.0,
                bw + 2.0,
                bh + 2.0,
                bar_rad + 1.0,
                1.0,
                COL_OUTER_HALO,
            );
            canvas.fill_round_rect(br.left as f32, br.top as f32, bw, bh, bar_rad, COL_SURFACE);
            canvas.stroke_round_rect(
                br.left as f32,
                br.top as f32,
                bw,
                bh,
                bar_rad,
                1.0,
                COL_SURFACE_BORDER,
            );

            // Hairline divider before Cancel
            let div_h = self.px(18.0);
            let div_y = br.top + (br.bottom - br.top - div_h) / 2;
            canvas.fill_rect_i32(self.divider_x, div_y, 1, div_h, COL_DIVIDER);

            let btn_rad = 8.0 * s;
            let kbd_rad = 4.5 * s;

            for b in &self.buttons {
                let hovered = self.hover_btn == Some(b.kind);
                let rx = b.rect.left as f32;
                let ry = b.rect.top as f32;
                let rw = (b.rect.right - b.rect.left) as f32;
                let rh = (b.rect.bottom - b.rect.top) as f32;

                // Button surface
                match (b.kind, hovered) {
                    (ToolbarBtn::Copy, true) => {
                        canvas.fill_round_rect(rx, ry, rw, rh, btn_rad, COL_PRIMARY_HOVER);
                        canvas.stroke_round_rect(rx, ry, rw, rh, btn_rad, 1.0, COL_PRIMARY_RIM);
                    }
                    (ToolbarBtn::Copy, false) => {
                        canvas.fill_round_rect(rx, ry, rw, rh, btn_rad, COL_PRIMARY);
                        canvas.stroke_round_rect(rx, ry, rw, rh, btn_rad, 1.0, COL_PRIMARY_RIM);
                    }
                    (ToolbarBtn::Cancel, true) => {
                        canvas.fill_round_rect(rx, ry, rw, rh, btn_rad, COL_CANCEL_HOVER_BG);
                        canvas.stroke_round_rect(rx, ry, rw, rh, btn_rad, 1.0, COL_CANCEL_HOVER_BD);
                    }
                    (_, true) => {
                        canvas.fill_round_rect(rx, ry, rw, rh, btn_rad, COL_SEC_HOVER_BG);
                        canvas.stroke_round_rect(rx, ry, rw, rh, btn_rad, 1.0, COL_SEC_HOVER_BD);
                    }
                    _ => {}
                }

                // Vector icon
                let icon_cy = ry + rh * 0.5;
                match b.kind {
                    ToolbarBtn::Copy => canvas.draw_icon_copy(b.icon_cx, icon_cy, s, COL_ICON_WHITE),
                    ToolbarBtn::Save => canvas.draw_icon_save(b.icon_cx, icon_cy, s, COL_ICON_SEC),
                    ToolbarBtn::Full => {
                        canvas.draw_icon_fullscreen(b.icon_cx, icon_cy, s, COL_ICON_SEC)
                    }
                    ToolbarBtn::Cancel => {
                        let col = if hovered { COL_ICON_ROSE } else { COL_ICON_MUTED };
                        canvas.draw_icon_close(b.icon_cx, icon_cy, s, col);
                    }
                }

                // Keycap pill
                let kr = b.keycap_rect;
                let kw = (kr.right - kr.left) as f32;
                let kh = (kr.bottom - kr.top) as f32;
                let (kbg, kbd) = match (b.kind, hovered) {
                    (ToolbarBtn::Copy, _) => (COL_PRIMARY_KBD_BG, COL_PRIMARY_KBD_BD),
                    (ToolbarBtn::Cancel, true) => (COL_CANCEL_KBD_HOVER_BG, COL_CANCEL_KBD_HOVER_BD),
                    _ => (COL_SEC_KBD_BG, COL_SEC_KBD_BD),
                };
                canvas.fill_round_rect(kr.left as f32, kr.top as f32, kw, kh, kbd_rad, kbg);
                canvas.stroke_round_rect(kr.left as f32, kr.top as f32, kw, kh, kbd_rad, 1.0, kbd);

                if b.kind == ToolbarBtn::Copy {
                    canvas.draw_icon_return(
                        kr.left as f32 + kw * 0.5,
                        kr.top as f32 + kh * 0.5,
                        s,
                        COL_ICON_WHITE,
                    );
                }
            }
        }

        // 5) Crisp ClearType text pass via GDI
        unsafe {
            SetBkMode(mem_dc, TRANSPARENT);

            if let Some((pr, dim_str, hdr)) = dim_pill_info {
                let pad_x = self.px(10.0);
                let old_f = SelectObject(mem_dc, self.font_mono);
                SetTextColor(mem_dc, colorref(248, 250, 252));
                let mut tr = RECT {
                    left: pr.left + pad_x,
                    top: pr.top,
                    right: pr.right,
                    bottom: pr.bottom,
                };
                draw_str(mem_dc, &dim_str, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

                if hdr {
                    let dim_w = measure_text_w(mem_dc, self.font_mono, &dim_str);
                    SelectObject(mem_dc, self.font_ui_med);
                    SetTextColor(mem_dc, colorref(186, 200, 218));
                    let mut hr_t = RECT {
                        left: pr.left + pad_x + dim_w + self.px(16.0),
                        top: pr.top,
                        right: pr.right,
                        bottom: pr.bottom,
                    };
                    draw_str(mem_dc, "HDR→sRGB", &mut hr_t, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
                }
                SelectObject(mem_dc, old_f);
            } else if let Some(hl) = hint_info {
                let old_f = SelectObject(mem_dc, self.font_ui_med);
                SetTextColor(mem_dc, colorref(244, 246, 250));
                let mut r1 = hl.txt_drag;
                draw_str(mem_dc, "Arraste para recortar", &mut r1, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
                SetTextColor(mem_dc, colorref(203, 213, 225));
                let mut r2 = hl.txt_full;
                draw_str(mem_dc, "Tela toda", &mut r2, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
                let mut r3 = hl.txt_esc;
                draw_str(mem_dc, "Sair", &mut r3, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

                SelectObject(mem_dc, self.font_keycap);
                SetTextColor(mem_dc, colorref(226, 232, 240));
                let mut kf = hl.kbd_f;
                draw_str(mem_dc, "F", &mut kf, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                let mut ke = hl.kbd_esc;
                draw_str(mem_dc, "Esc", &mut ke, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                SelectObject(mem_dc, old_f);
            }

            if !self.buttons.is_empty() {
                let old_f = SelectObject(mem_dc, self.font_ui_med);
                for b in &self.buttons {
                    let hovered = self.hover_btn == Some(b.kind);
                    if !b.label.is_empty() {
                        if b.kind == ToolbarBtn::Copy {
                            SelectObject(mem_dc, self.font_ui_bold);
                            SetTextColor(mem_dc, colorref(255, 255, 255));
                        } else {
                            SelectObject(mem_dc, self.font_ui_med);
                            SetTextColor(mem_dc, colorref(236, 239, 244));
                        }
                        let mut tr = RECT {
                            left: b.text_x,
                            top: b.rect.top,
                            right: b.keycap_rect.left,
                            bottom: b.rect.bottom,
                        };
                        draw_str(mem_dc, b.label, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
                    }
                    if !b.keycap.is_empty() {
                        SelectObject(mem_dc, self.font_keycap);
                        let col = if b.kind == ToolbarBtn::Cancel && hovered {
                            colorref(254, 205, 211)
                        } else {
                            colorref(203, 213, 225)
                        };
                        SetTextColor(mem_dc, col);
                        let mut kr = b.keycap_rect;
                        draw_str(mem_dc, b.keycap, &mut kr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                    }
                }
                SelectObject(mem_dc, old_f);
            }
            let _ = GdiFlush();
        }
    }
}

struct HintLayout {
    bar: RECT,
    txt_drag: RECT,
    div1_x: i32,
    txt_full: RECT,
    kbd_f: RECT,
    div2_x: i32,
    txt_esc: RECT,
    kbd_esc: RECT,
}

fn compute_hint_layout(st: &OverlayState, dc: HDC, screen_w: i32) -> HintLayout {
    let w_drag = measure_text_w(dc, st.font_ui_med, "Arraste para recortar");
    let w_full = measure_text_w(dc, st.font_ui_med, "Tela toda");
    let w_esc = measure_text_w(dc, st.font_ui_med, "Sair");
    let w_kf = (measure_text_w(dc, st.font_keycap, "F") + st.px(12.0)).max(st.px(19.0));
    let w_ke = (measure_text_w(dc, st.font_keycap, "Esc") + st.px(12.0)).max(st.px(28.0));

    let pad_x = st.px(14.0);
    let div_gap = st.px(11.0);
    let kbd_gap = st.px(6.0);
    let bar_h = st.px(36.0);
    let kbd_h = st.px(19.0);

    let total_w = pad_x * 2
        + w_drag
        + div_gap * 2
        + 1
        + w_full
        + kbd_gap
        + w_kf
        + div_gap * 2
        + 1
        + w_esc
        + kbd_gap
        + w_ke;

    let bx = (screen_w - total_w) / 2;
    let by = st.px(22.0);
    let bar = RECT {
        left: bx,
        top: by,
        right: bx + total_w,
        bottom: by + bar_h,
    };
    let ky = by + (bar_h - kbd_h) / 2;

    let mut cx = bx + pad_x;
    let txt_drag = RECT { left: cx, top: by, right: cx + w_drag, bottom: by + bar_h };
    cx += w_drag + div_gap;
    let div1_x = cx;
    cx += 1 + div_gap;
    let txt_full = RECT { left: cx, top: by, right: cx + w_full, bottom: by + bar_h };
    cx += w_full + kbd_gap;
    let kbd_f = RECT { left: cx, top: ky, right: cx + w_kf, bottom: ky + kbd_h };
    cx += w_kf + div_gap;
    let div2_x = cx;
    cx += 1 + div_gap;
    let txt_esc = RECT { left: cx, top: by, right: cx + w_esc, bottom: by + bar_h };
    cx += w_esc + kbd_gap;
    let kbd_esc = RECT { left: cx, top: ky, right: cx + w_ke, bottom: ky + kbd_h };

    HintLayout {
        bar,
        txt_drag,
        div1_x,
        txt_full,
        kbd_f,
        div2_x,
        txt_esc,
        kbd_esc,
    }
}

impl Drop for OverlayState {
    fn drop(&mut self) {
        unsafe {
            for bmp in [self.bmp_bright, self.bmp_dim, self.bmp_back] {
                if !bmp.is_invalid() {
                    let _ = DeleteObject(bmp);
                }
            }
            for f in [
                self.font_ui_bold,
                self.font_ui_med,
                self.font_mono,
                self.font_keycap,
            ] {
                if !f.is_invalid() {
                    let _ = DeleteObject(f);
                }
            }
        }
        if let Some(cb) = self.on_close_cb.take() {
            cb();
        }
    }
}

fn create_topdown_dib(width: u32, height: u32, bgra: Option<&[u8]>) -> (HBITMAP, *mut u8) {
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
            if let Some(src) = bgra {
                std::ptr::copy_nonoverlapping(src.as_ptr(), bits as *mut u8, src.len());
            }
        }
        (hbmp, bits as *mut u8)
    }
}

fn make_dimmed_bgra(bgra: &[u8]) -> Vec<u8> {
    let mut dim = vec![0u8; bgra.len()];
    for (src, dst) in bgra.chunks_exact(4).zip(dim.chunks_exact_mut(4)) {
        // Darken to ~44% brightness so the bright crop & toolbar pop cleanly
        dst[0] = ((src[0] as u16 * 112) >> 8) as u8;
        dst[1] = ((src[1] as u16 * 112) >> 8) as u8;
        dst[2] = ((src[2] as u16 * 112) >> 8) as u8;
        dst[3] = 255;
    }
    dim
}

fn create_ui_font(face_name: &str, px_height: i32, weight: i32, monospace: bool) -> HFONT {
    let wide: Vec<u16> = face_name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut face = [0u16; 32];
    let n = wide.len().min(32);
    face[..n].copy_from_slice(&wide[..n]);
    let pitch_family = if monospace {
        (FIXED_PITCH.0 | FF_MODERN.0) as u32
    } else {
        (DEFAULT_PITCH.0 | FF_SWISS.0) as u32
    };
    unsafe {
        CreateFontW(
            -px_height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            pitch_family,
            PCWSTR(face.as_ptr()),
        )
    }
}

fn measure_text_w(dc: HDC, font: HFONT, text: &str) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut sz = SIZE::default();
    unsafe {
        let old = SelectObject(dc, font);
        let _ = GetTextExtentPoint32W(dc, &wide, &mut sz);
        SelectObject(dc, old);
    }
    sz.cx
}

unsafe fn draw_str(
    dc: HDC,
    text: &str,
    rect: &mut RECT,
    flags: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(dc, &mut wide, rect, flags);
}

fn query_monitor_scale(mon_rect: RECT) -> f32 {
    unsafe {
        let pt = POINT {
            x: mon_rect.left + (mon_rect.right - mon_rect.left) / 2,
            y: mon_rect.top + (mon_rect.bottom - mon_rect.top) / 2,
        };
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut dpi_x: u32 = 96;
        let mut dpi_y: u32 = 96;
        if GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_ok() && dpi_x >= 72
        {
            (dpi_x as f32 / 96.0).clamp(1.0, 3.0)
        } else {
            1.0
        }
    }
}

fn build_overlay_state(screen: CapturedScreen, on_close: Option<Box<dyn FnOnce()>>) -> OverlayState {
    let scale = query_monitor_scale(screen.monitor_rect);
    let (bmp_bright, _) = create_topdown_dib(screen.width, screen.height, Some(&screen.bgra));
    let dim_bgra = make_dimmed_bgra(&screen.bgra);
    let (bmp_dim, _) = create_topdown_dib(screen.width, screen.height, Some(&dim_bgra));
    let (bmp_back, back_bits) = create_topdown_dib(screen.width, screen.height, None);

    let font_ui_bold = create_ui_font(
        "Segoe UI",
        (14.0 * scale).round() as i32,
        FW_SEMIBOLD.0 as i32,
        false,
    );
    let font_ui_med = create_ui_font(
        "Segoe UI",
        (14.0 * scale).round() as i32,
        FW_MEDIUM.0 as i32,
        false,
    );
    let font_mono = create_ui_font(
        "Consolas",
        (13.0 * scale).round() as i32,
        FW_SEMIBOLD.0 as i32,
        true,
    );
    let font_keycap = create_ui_font(
        "Consolas",
        (11.5 * scale).round() as i32,
        FW_SEMIBOLD.0 as i32,
        true,
    );

    OverlayState {
        screen,
        scale,
        bmp_bright,
        bmp_dim,
        bmp_back,
        back_bits,
        font_ui_bold,
        font_ui_med,
        font_mono,
        font_keycap,
        has_selection: false,
        sel: RECT::default(),
        drag_mode: DragMode::None,
        anchor: POINT::default(),
        orig_sel: RECT::default(),
        hover_btn: None,
        bar_rect: RECT::default(),
        divider_x: 0,
        buttons: Vec::new(),
        on_close_cb: on_close,
    }
}

/// Offline UI render self-test: draws the real Keycap Minimal overlay with a sample
/// selection onto `screen` and saves it to `out_path` for visual inspection.
pub fn render_ui_preview_png(screen: CapturedScreen, out_path: &Path) -> Result<(), String> {
    let w = screen.width;
    let h = screen.height;
    let mut st = build_overlay_state(screen, None);
    st.has_selection = true;
    st.sel = RECT {
        left: (w as i32) / 5,
        top: (h as i32) / 4,
        right: (w as i32) * 3 / 4,
        bottom: (h as i32) * 2 / 3,
    };
    st.compute_toolbar();

    unsafe {
        let mem_dc = CreateCompatibleDC(HDC::default());
        let old_mem = SelectObject(mem_dc, st.bmp_back);
        let src_dc = CreateCompatibleDC(HDC::default());
        st.render_frame(mem_dc, src_dc);
        let _ = DeleteDC(src_dc);
        SelectObject(mem_dc, old_mem);
        let _ = DeleteDC(mem_dc);
    }

    let bgra = unsafe {
        std::slice::from_raw_parts(st.back_bits as *const u8, (w as usize) * (h as usize) * 4)
    };
    let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for px in bgra.chunks_exact(4) {
        rgb.push(px[2]);
        rgb.push(px[1]);
        rgb.push(px[0]);
    }
    let file = std::fs::File::create(out_path).map_err(|e| format!("create png: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| format!("png header: {e}"))?;
    writer
        .write_image_data(&rgb)
        .map_err(|e| format!("png write: {e}"))?;
    Ok(())
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

/// Opens the fullscreen Keycap Minimal crop overlay for `screen`.
pub fn show_overlay(screen: CapturedScreen, on_close: impl FnOnce() + 'static) {
    let hinst: HINSTANCE = unsafe { GetModuleHandleW(None).unwrap_or_default().into() };
    ensure_overlay_class(hinst);

    let mon = screen.monitor_rect;
    let state = Box::new(build_overlay_state(screen, Some(Box::new(on_close))));
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
                unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
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

        let mem_dc = CreateCompatibleDC(hdc);
        let old_mem = SelectObject(mem_dc, st.bmp_back);
        let src_dc = CreateCompatibleDC(hdc);

        st.render_frame(mem_dc, src_dc);

        let _ = BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);

        let _ = DeleteDC(src_dc);
        SelectObject(mem_dc, old_mem);
        let _ = DeleteDC(mem_dc);
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
                let _ = GetCursorPos(&mut pt);
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
                if vk == VK_ESCAPE.0 {
                    execute_action(hwnd, st, ToolbarBtn::Cancel);
                } else if vk == VK_RETURN.0 || vk == VK_C.0 {
                    if st.sel_valid() {
                        execute_action(hwnd, st, ToolbarBtn::Copy);
                    }
                } else if vk == VK_S.0 {
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
