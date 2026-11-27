//! Global low-level keyboard hook (`WH_KEYBOARD_LL`) for `PrintScreen` and `Win+Shift+S`,
//! plus system tray icon & single-instance daemon loop.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod clipboard;
mod overlay;
mod ui_draw;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU16, Ordering};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RSHIFT, VK_RWIN, VK_S, VK_SHIFT, VK_SNAPSHOT,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreateIconIndirect, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyIcon, DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW,
    PostMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow, SetWindowsHookExW,
    TrackPopupMenu, TranslateMessage, UnhookWindowsHookEx, HHOOK, HICON, ICONINFO, KBDLLHOOKSTRUCT,
    MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, WH_KEYBOARD_LL, WM_APP, WM_COMMAND, WM_DESTROY, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONUP, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW,
};

const WM_TRAY_ICON: u32 = WM_APP + 1;
const WM_TRIGGER_SNIP: u32 = WM_APP + 2;

const CMD_CAPTURE_NOW: usize = 1001;
const CMD_TOGGLE_PAUSE: usize = 1002;
const CMD_EXIT: usize = 1003;

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static OVERLAY_OPEN: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
static SWALLOWED_VK: AtomicU16 = AtomicU16::new(0);

fn is_key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0 }
}

/// Sends an unassigned virtual key (`0xE8`) so releasing `Win` after intercepting `Win+Shift+S`
/// does NOT pop up the Windows Start Menu (same standard technique used by PowerToys/AutoHotkey).
fn mask_win_key_start_menu() {
    let inputs = [
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0xE8),
                    ..Default::default()
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0xE8),
                    dwFlags: KEYEVENTF_KEYUP,
                    ..Default::default()
                },
            },
        },
    ];
    unsafe {
        let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

unsafe extern "system" fn ll_keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wparam.0 as u32;
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk = VIRTUAL_KEY(kb.vkCode as u16);

        // Swallow matching KEYUP for a hotkey we just consumed so foreground apps never see orphan KEYUP
        if (msg == WM_KEYUP || msg == WM_SYSKEYUP) && vk.0 != 0 {
            if SWALLOWED_VK
                .compare_exchange(vk.0, 0, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                return LRESULT(1);
            }
        }

        if !PAUSED.load(Ordering::Relaxed)
            && !OVERLAY_OPEN.load(Ordering::Relaxed)
            && (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN)
        {
            let alt_down = is_key_down(VK_MENU);
            let shift_down =
                is_key_down(VK_SHIFT) || is_key_down(VK_LSHIFT) || is_key_down(VK_RSHIFT);
            let win_down = is_key_down(VK_LWIN) || is_key_down(VK_RWIN);

            // 1) PrintScreen (without Alt — keep Win+Alt+PrtScn free for Xbox Game Bar if desired)
            let is_print_screen = vk == VK_SNAPSHOT && !alt_down;
            // 2) Win + Shift + S
            let is_win_shift_s = vk == VK_S && win_down && shift_down && !alt_down;

            if is_print_screen || is_win_shift_s {
                SWALLOWED_VK.store(vk.0, Ordering::SeqCst);
                if is_win_shift_s {
                    mask_win_key_start_menu();
                }
                let raw_hwnd = MAIN_HWND.load(Ordering::Relaxed);
                if raw_hwnd != 0 {
                    let _ = PostMessageW(
                        HWND(raw_hwnd as *mut _),
                        WM_TRIGGER_SNIP,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
                return LRESULT(1); // Swallow key so Snipping Tool / Lightshot does not also fire
            }
        }
    }
    let hook = HHOOK(HOOK_HANDLE.load(Ordering::Relaxed) as *mut _);
    CallNextHookEx(hook, code, wparam, lparam)
}

/// Builds a crisp 16×16 tray icon in memory (no external .ico file needed).
fn create_tray_icon() -> HICON {
    let w = 16usize;
    let h = 16usize;
    let mut bgra = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let border = x <= 1 || x >= w - 2 || y <= 1 || y >= h - 2;
            let inner_frame = (x == 4 || x == w - 5) && (y >= 4 && y <= h - 5)
                || (y == 4 || y == h - 5) && (x >= 4 && x <= w - 5);
            let center_dot = (x >= 7 && x <= 8) && (y >= 7 && y <= 8);
            if center_dot {
                // Warm gold dot (HDR badge)
                bgra[i] = 0x24;
                bgra[i + 1] = 0xBF;
                bgra[i + 2] = 0xFB;
                bgra[i + 3] = 255;
            } else if inner_frame {
                // Bright white crop frame
                bgra[i] = 0xFF;
                bgra[i + 1] = 0xFF;
                bgra[i + 2] = 0xFF;
                bgra[i + 3] = 255;
            } else if border {
                // Sky blue border
                bgra[i] = 0xF8;
                bgra[i + 1] = 0xBD;
                bgra[i + 2] = 0x38;
                bgra[i + 3] = 255;
            } else {
                // Deep navy fill
                bgra[i] = 0x3A;
                bgra[i + 1] = 0x1E;
                bgra[i + 2] = 0x15;
                bgra[i + 3] = 255;
            }
        }
    }
    unsafe {
        let color_bmp = CreateBitmap(w as i32, h as i32, 1, 32, Some(bgra.as_ptr() as *const _));
        let mask_bits = vec![0u8; w * h / 8];
        let mask_bmp = CreateBitmap(w as i32, h as i32, 1, 1, Some(mask_bits.as_ptr() as *const _));
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask_bmp,
            hbmColor: color_bmp,
        };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color_bmp);
        let _ = DeleteObject(mask_bmp);
        icon
    }
}

fn add_tray_icon(hwnd: HWND, icon: HICON) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY_ICON,
        hIcon: icon,
        ..Default::default()
    };
    let tip = "HDR Snip — PrintScreen / Win+Shift+S";
    for (dst, ch) in nid.szTip.iter_mut().zip(tip.encode_utf16()) {
        *dst = ch;
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

fn remove_tray_icon(hwnd: HWND) {
    let nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn show_tray_menu(hwnd: HWND) {
    unsafe {
        if let Ok(menu) = CreatePopupMenu() {
            let (_, _, dev) = capture::target_monitor_under_cursor();
            let info = capture::query_display_hdr_info(&dev);
            let status_txt = if info.hdr_enabled {
                format!(
                    "HDR ON · {:.0} nits ({:.2}x → sRGB exato)\0",
                    info.sdr_white_nits, info.sdr_white_mult
                )
            } else {
                "Monitor SDR (captura padrão)\0".to_string()
            };
            let status_w: Vec<u16> = status_txt.encode_utf16().collect();
            let cap_w: Vec<u16> = "Capturar agora (PrintScreen / Win+Shift+S)\0"
                .encode_utf16()
                .collect();
            let pause_w: Vec<u16> = "Pausar atalhos\0".encode_utf16().collect();
            let exit_w: Vec<u16> = "Sair\0".encode_utf16().collect();

            let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(status_w.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_CAPTURE_NOW, PCWSTR(cap_w.as_ptr()));
            let pause_flags = MF_STRING
                | if PAUSED.load(Ordering::Relaxed) {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                };
            let _ = AppendMenuW(menu, pause_flags, CMD_TOGGLE_PAUSE, PCWSTR(pause_w.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_EXIT, PCWSTR(exit_w.as_ptr()));

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(hwnd);
            let _ = TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, None);
            let _ = DestroyMenu(menu);
        }
    }
}

fn trigger_snip() {
    if OVERLAY_OPEN.swap(true, Ordering::SeqCst) {
        return;
    }
    match capture::capture_active_monitor() {
        Ok(screen) => {
            overlay::show_overlay(screen, || {
                OVERLAY_OPEN.store(false, Ordering::SeqCst);
            });
        }
        Err(_) => {
            OVERLAY_OPEN.store(false, Ordering::SeqCst);
        }
    }
}

unsafe extern "system" fn main_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRIGGER_SNIP => {
            trigger_snip();
            LRESULT(0)
        }
        WM_TRAY_ICON => {
            let ev = (lparam.0 & 0xFFFF) as u32;
            if ev == WM_RBUTTONUP {
                show_tray_menu(hwnd);
            } else if ev == WM_LBUTTONUP {
                trigger_snip();
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd = wparam.0 & 0xFFFF;
            match cmd {
                CMD_CAPTURE_NOW => trigger_snip(),
                CMD_TOGGLE_PAUSE => {
                    let prev = PAUSED.load(Ordering::Relaxed);
                    PAUSED.store(!prev, Ordering::Relaxed);
                }
                CMD_EXIT => {
                    remove_tray_icon(hwnd);
                    PostQuitMessage(0);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            remove_tray_icon(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    // CLI self-test mode: `hdr-snip.exe --test-capture <output.png>` (captures full screen and exits)
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--test-ui" {
        let out_path = PathBuf::from(&args[2]);
        match capture::capture_active_monitor() {
            Ok(screen) => {
                if let Err(e) = overlay::render_ui_preview_png(screen, &out_path) {
                    eprintln!("PREVIEW_ERR: {e}");
                    std::process::exit(2);
                }
                println!("OK_PREVIEW {}", out_path.display());
                return;
            }
            Err(e) => {
                eprintln!("CAPTURE_ERR: {e:?}");
                std::process::exit(1);
            }
        }
    }
    if args.len() >= 3 && args[1] == "--test-capture" {
        let out_path = PathBuf::from(&args[2]);
        match capture::capture_active_monitor() {
            Ok(screen) => {
                let full = RECT {
                    left: 0,
                    top: 0,
                    right: screen.width as i32,
                    bottom: screen.height as i32,
                };
                if let Err(e) = clipboard::save_crop_png(&screen, full, &out_path) {
                    eprintln!("SAVE_ERR: {e}");
                    std::process::exit(2);
                }
                println!(
                    "OK {}x{} hdr={} sdr_nits={:.0} mult={:.2}x ms={:.1}",
                    screen.width,
                    screen.height,
                    screen.hdr_info.hdr_enabled,
                    screen.hdr_info.sdr_white_nits,
                    screen.hdr_info.sdr_white_mult,
                    screen.capture_ms
                );
                return;
            }
            Err(e) => {
                eprintln!("CAPTURE_ERR: {e:?}");
                std::process::exit(1);
            }
        }
    }

    // Per-user single instance guard (allows multiple Windows users logged in simultaneously!)
    let mutex_name: Vec<u16> = "Local\\HdrSnipSingleInstanceMutex\0".encode_utf16().collect();
    let _mutex = unsafe { CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return;
    }

    let hinst: HINSTANCE = unsafe { GetModuleHandleW(None).unwrap_or_default().into() };
    let class_name: Vec<u16> = "HdrSnipMainHiddenWnd\0".encode_utf16().collect();
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(main_wnd_proc),
            hInstance: hinst,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);
    }

    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            Default::default(),
            0,
            0,
            0,
            0,
            HWND::default(),
            None,
            hinst,
            None,
        )
        .expect("Failed to create hidden message window")
    };
    MAIN_HWND.store(hwnd.0 as isize, Ordering::SeqCst);

    let icon = create_tray_icon();
    add_tray_icon(hwnd, icon);

    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_keyboard_proc), hinst, 0) }
        .expect("Failed to install keyboard hook");
    HOOK_HANDLE.store(hook.0 as isize, Ordering::SeqCst);

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(hook);
        let _ = DestroyIcon(icon);
    }
}
