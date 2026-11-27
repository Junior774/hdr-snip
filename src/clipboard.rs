//! Clipboard (CF_DIB + PNG) and Save-As PNG dialog helpers.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::ptr::copy_nonoverlapping;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{HANDLE, HWND, RECT, SYSTEMTIME};
use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Controls::Dialogs::{
    GetSaveFileNameW, OFN_EXPLORER, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};

use crate::capture::CapturedScreen;

const CF_DIB: u32 = 8;

/// Encodes an RGBA8 image into an in-memory PNG byte buffer.
pub fn encode_png_bytes(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity((width * height) as usize);
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Saves a cropped region directly as PNG to `path`.
pub fn save_crop_png(screen: &CapturedScreen, rect: RECT, path: &PathBuf) -> Result<(), String> {
    let (w, h, rgba) = screen.crop_rgba(rect);
    let file = File::create(path).map_err(|e| e.to_string())?;
    let buf = BufWriter::new(file);
    let mut encoder = png::Encoder::new(buf, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    Ok(())
}

/// Places a cropped region onto the Windows clipboard as both `CF_DIB` (universal) and `"PNG"`.
pub fn copy_crop_to_clipboard(screen: &CapturedScreen, rect: RECT) -> Result<(), String> {
    let (w, h, bgra_topdown) = screen.crop_bgra(rect);
    let (_, _, rgba) = screen.crop_rgba(rect);
    let png_bytes = encode_png_bytes(w, h, &rgba)?;

    // Build bottom-up 32bpp BITMAPINFOHEADER + pixels for CF_DIB (maximum compatibility across apps)
    let hdr = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        biHeight: h as i32, // positive = bottom-up DIB
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: (w * h * 4) as u32,
        ..Default::default()
    };
    let hdr_size = std::mem::size_of::<BITMAPINFOHEADER>();
    let dib_total = hdr_size + (w as usize) * (h as usize) * 4;

    unsafe {
        if OpenClipboard(HWND::default()).is_err() {
            return Err("Falha ao abrir o clipboard do Windows".into());
        }
        let _ = EmptyClipboard();

        // 1) Set CF_DIB
        if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, dib_total) {
            let ptr = GlobalLock(hmem) as *mut u8;
            if !ptr.is_null() {
                copy_nonoverlapping(&hdr as *const _ as *const u8, ptr, hdr_size);
                let px_dst = ptr.add(hdr_size);
                let row_bytes = (w as usize) * 4;
                for y in 0..(h as usize) {
                    let src_row = (h as usize - 1 - y) * row_bytes;
                    let dst_row = y * row_bytes;
                    copy_nonoverlapping(
                        bgra_topdown.as_ptr().add(src_row),
                        px_dst.add(dst_row),
                        row_bytes,
                    );
                }
                let _ = GlobalUnlock(hmem);
                let _ = SetClipboardData(CF_DIB, HANDLE(hmem.0));
            }
        }

        // 2) Set registered "PNG" format for modern apps
        let fmt_name: Vec<u16> = "PNG\0".encode_utf16().collect();
        let cf_png = RegisterClipboardFormatW(PCWSTR(fmt_name.as_ptr()));
        if cf_png != 0 {
            if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, png_bytes.len()) {
                let ptr = GlobalLock(hmem) as *mut u8;
                if !ptr.is_null() {
                    copy_nonoverlapping(png_bytes.as_ptr(), ptr, png_bytes.len());
                    let _ = GlobalUnlock(hmem);
                    let _ = SetClipboardData(cf_png, HANDLE(hmem.0));
                }
            }
        }

        let _ = CloseClipboard();
    }
    Ok(())
}

/// Opens the native Windows Save-As dialog for PNG, defaulting to Pictures\Screenshots (or Pictures).
pub fn prompt_save_png_dialog(owner: HWND) -> Option<PathBuf> {
    let default_name = unsafe {
        let st: SYSTEMTIME = GetLocalTime();
        format!(
            "Print_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}.png",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        )
    };

    let mut file_buf = vec![0u16; 1024];
    for (dst, src) in file_buf.iter_mut().zip(default_name.encode_utf16()) {
        *dst = src;
    }

    let filter: Vec<u16> = "Imagem PNG (*.png)\0*.png\0Todos os arquivos (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let def_ext: Vec<u16> = "png\0".encode_utf16().collect();

    let init_dir: Option<Vec<u16>> = std::env::var_os("USERPROFILE").map(|p| {
        let mut pb = PathBuf::from(p);
        pb.push("Pictures");
        let screenshots = pb.join("Screenshots");
        if screenshots.is_dir() {
            pb = screenshots;
        }
        let s = pb.to_string_lossy();
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.push(0);
        v
    });

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: PWSTR(file_buf.as_mut_ptr()),
        nMaxFile: file_buf.len() as u32,
        lpstrInitialDir: init_dir
            .as_ref()
            .map(|v| PCWSTR(v.as_ptr()))
            .unwrap_or(PCWSTR::null()),
        lpstrDefExt: PCWSTR(def_ext.as_ptr()),
        Flags: OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_OVERWRITEPROMPT,
        ..Default::default()
    };

    unsafe {
        if GetSaveFileNameW(&mut ofn).as_bool() {
            let len = file_buf.iter().position(|&c| c == 0).unwrap_or(file_buf.len());
            return Some(PathBuf::from(String::from_utf16_lossy(&file_buf[..len])));
        }
    }
    None
}
