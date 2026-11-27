use std::sync::OnceLock;
use std::time::{Duration, Instant};

use half::f16;
use windows::core::{Interface, Result};
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{BOOL, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFOEXW,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

const LUT_MAX: usize = 16384;

fn linear_to_srgb_lut() -> &'static [u8; LUT_MAX + 1] {
    static LUT: OnceLock<[u8; LUT_MAX + 1]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut table = [0u8; LUT_MAX + 1];
        for (i, slot) in table.iter_mut().enumerate() {
            let x = (i as f32) / (LUT_MAX as f32);
            let srgb = if x <= 0.0031308 {
                x * 12.92
            } else {
                1.055 * x.powf(1.0 / 2.4) - 0.055
            };
            *slot = (srgb * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        table
    })
}

#[derive(Debug, Clone)]
pub struct DisplayHdrInfo {
    #[allow(dead_code)]
    pub hdr_supported: bool,
    pub hdr_enabled: bool,
    pub sdr_white_mult: f32,
    pub sdr_white_nits: f32,
    #[allow(dead_code)]
    pub device_name: String,
}

impl Default for DisplayHdrInfo {
    fn default() -> Self {
        Self {
            hdr_supported: false,
            hdr_enabled: false,
            sdr_white_mult: 1.0,
            sdr_white_nits: 80.0,
            device_name: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct CapturedScreen {
    pub width: u32,
    pub height: u32,
    pub monitor_rect: RECT,

    pub bgra: Vec<u8>,
    pub hdr_info: DisplayHdrInfo,
    pub capture_ms: f64,
}

impl CapturedScreen {
    pub fn crop_rgba(&self, rect: RECT) -> (u32, u32, Vec<u8>) {
        let x0 = rect.left.clamp(0, self.width as i32) as u32;
        let y0 = rect.top.clamp(0, self.height as i32) as u32;
        let x1 = rect.right.clamp(0, self.width as i32) as u32;
        let y1 = rect.bottom.clamp(0, self.height as i32) as u32;
        let cw = x1.saturating_sub(x0).max(1);
        let ch = y1.saturating_sub(y0).max(1);
        let mut rgba = Vec::with_capacity((cw * ch * 4) as usize);
        let stride = (self.width * 4) as usize;
        for y in y0..(y0 + ch) {
            let row_start = (y as usize) * stride + (x0 as usize) * 4;
            let row = &self.bgra[row_start..row_start + (cw as usize) * 4];
            for px in row.chunks_exact(4) {
                rgba.push(px[2]);
                rgba.push(px[1]);
                rgba.push(px[0]);
                rgba.push(255);
            }
        }
        (cw, ch, rgba)
    }

    pub fn crop_bgra(&self, rect: RECT) -> (u32, u32, Vec<u8>) {
        let x0 = rect.left.clamp(0, self.width as i32) as u32;
        let y0 = rect.top.clamp(0, self.height as i32) as u32;
        let x1 = rect.right.clamp(0, self.width as i32) as u32;
        let y1 = rect.bottom.clamp(0, self.height as i32) as u32;
        let cw = x1.saturating_sub(x0).max(1);
        let ch = y1.saturating_sub(y0).max(1);
        let mut out = Vec::with_capacity((cw * ch * 4) as usize);
        let stride = (self.width * 4) as usize;
        for y in y0..(y0 + ch) {
            let row_start = (y as usize) * stride + (x0 as usize) * 4;
            out.extend_from_slice(&self.bgra[row_start..row_start + (cw as usize) * 4]);
        }
        (cw, ch, out)
    }
}

fn monitor_info_for_handle(hmon: HMONITOR) -> (RECT, String) {
    unsafe {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, &mut info.monitorInfo as *mut _ as *mut _).as_bool() {
            let len = info
                .szDevice
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(info.szDevice.len());
            let name = String::from_utf16_lossy(&info.szDevice[..len]);
            return (info.monitorInfo.rcMonitor, name);
        }
    }
    (
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        },
        String::new(),
    )
}

pub fn target_monitor_under_cursor() -> (HMONITOR, RECT, String) {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        if !hmon.is_invalid() {
            let (rect, name) = monitor_info_for_handle(hmon);
            return (hmon, rect, name);
        }

        let mut first: Option<HMONITOR> = None;
        unsafe extern "system" fn cb(hm: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
            let slot = &mut *(data.0 as *mut Option<HMONITOR>);
            if slot.is_none() {
                *slot = Some(hm);
            }
            BOOL(0)
        }
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(cb),
            LPARAM(&mut first as *mut _ as isize),
        );
        let hm = first.unwrap_or_default();
        let (rect, name) = monitor_info_for_handle(hm);
        (hm, rect, name)
    }
}

pub fn query_display_hdr_info(target_device: &str) -> DisplayHdrInfo {
    unsafe {
        let mut num_paths = 0u32;
        let mut num_modes = 0u32;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut num_paths, &mut num_modes)
            .is_err()
            || num_paths == 0
        {
            return DisplayHdrInfo::default();
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); num_paths as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); num_modes.max(1) as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut num_paths,
            paths.as_mut_ptr(),
            &mut num_modes,
            modes.as_mut_ptr(),
            None,
        )
        .is_err()
        {
            return DisplayHdrInfo::default();
        }

        let mut fallback: Option<DisplayHdrInfo> = None;
        for path in &paths[..num_paths as usize] {
            let mut src_name = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: path.sourceInfo.adapterId,
                    id: path.sourceInfo.id,
                },
                ..Default::default()
            };
            let gdi_name = if DisplayConfigGetDeviceInfo(&mut src_name.header) == 0 {
                let len = src_name
                    .viewGdiDeviceName
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(src_name.viewGdiDeviceName.len());
                String::from_utf16_lossy(&src_name.viewGdiDeviceName[..len])
            } else {
                String::new()
            };

            let mut adv = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                    size: std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
                    adapterId: path.targetInfo.adapterId,
                    id: path.targetInfo.id,
                },
                ..Default::default()
            };
            let adv_ok = DisplayConfigGetDeviceInfo(&mut adv.header) == 0;

            let mut sdr = DISPLAYCONFIG_SDR_WHITE_LEVEL {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                    size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                    adapterId: path.targetInfo.adapterId,
                    id: path.targetInfo.id,
                },
                ..Default::default()
            };
            let sdr_ok = DisplayConfigGetDeviceInfo(&mut sdr.header) == 0;

            let val = if adv_ok { adv.Anonymous.value } else { 0 };
            let hdr_supported = (val & 1) != 0;
            let hdr_enabled = (val & 2) != 0;
            let mult = if sdr_ok && sdr.SDRWhiteLevel > 0 {
                (sdr.SDRWhiteLevel as f32 / 1000.0).max(0.5)
            } else {
                1.0
            };
            let info = DisplayHdrInfo {
                hdr_supported,
                hdr_enabled,
                sdr_white_mult: mult,
                sdr_white_nits: (mult * 80.0).max(40.0),
                device_name: gdi_name.clone(),
            };
            if !target_device.is_empty() && gdi_name.eq_ignore_ascii_case(target_device) {
                return info;
            }
            if fallback.is_none() {
                fallback = Some(info);
            }
        }
        fallback.unwrap_or_default()
    }
}

pub fn capture_active_monitor() -> Result<CapturedScreen> {
    let t0 = Instant::now();
    let (hmon, mon_rect, dev_name) = target_monitor_under_cursor();
    let hdr_info = query_display_hdr_info(&dev_name);

    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }

    let mut d3d_dev: Option<ID3D11Device> = None;
    let mut d3d_ctx: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d_dev),
            None,
            Some(&mut d3d_ctx),
        )?;
    }
    let d3d_dev = d3d_dev.unwrap();
    let d3d_ctx = d3d_ctx.unwrap();
    let dxgi_dev: IDXGIDevice = d3d_dev.cast()?;
    let winrt_dev: IDirect3DDevice =
        unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_dev)?.cast()? };

    let interop: IGraphicsCaptureItemInterop =
        windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(hmon)? };
    let size = item.Size()?;

    let pixel_format = if hdr_info.hdr_enabled {
        DirectXPixelFormat::R16G16B16A16Float
    } else {
        DirectXPixelFormat::B8G8R8A8UIntNormalized
    };

    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt_dev, pixel_format, 2, size)?;
    let session = pool.CreateCaptureSession(&item)?;
    let _ = session.SetIsBorderRequired(false);
    let _ = session.SetIsCursorCaptureEnabled(false);
    session.StartCapture()?;

    let deadline = Instant::now() + Duration::from_secs(3);
    let frame = loop {
        if let Ok(f) = pool.TryGetNextFrame() {
            break f;
        }
        if Instant::now() >= deadline {
            let _ = session.Close();
            let _ = pool.Close();
            return Err(windows::core::Error::from_win32());
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let surface = frame.Surface()?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
    let tex: ID3D11Texture2D = unsafe { access.GetInterface()? };

    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { tex.GetDesc(&mut desc) };

    let mut staging_desc = desc;
    staging_desc.Usage = D3D11_USAGE_STAGING;
    staging_desc.BindFlags = 0;
    staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    staging_desc.MiscFlags = 0;

    let mut staging: Option<ID3D11Texture2D> = None;
    unsafe { d3d_dev.CreateTexture2D(&staging_desc, None, Some(&mut staging))? };
    let staging = staging.unwrap();

    unsafe { d3d_ctx.CopyResource(&staging, &tex) };

    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { d3d_ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))? };

    let w = desc.Width as usize;
    let h = desc.Height as usize;
    let pitch = mapped.RowPitch as usize;
    let mut bgra = vec![0u8; w * h * 4];

    if hdr_info.hdr_enabled {
        let lut = linear_to_srgb_lut();
        let inv_mult = 1.0f32 / hdr_info.sdr_white_mult.max(0.5);
        let scale = LUT_MAX as f32;
        unsafe {
            let base = mapped.pData as *const u8;
            for y in 0..h {
                let row_ptr = base.add(y * pitch) as *const u16;
                let row_u16 = std::slice::from_raw_parts(row_ptr, w * 4);
                let dst = &mut bgra[y * w * 4..(y + 1) * w * 4];
                for x in 0..w {
                    let si = x * 4;
                    let di = x * 4;
                    let r_lin = (f16::from_bits(row_u16[si]).to_f32() * inv_mult).clamp(0.0, 1.0);
                    let g_lin =
                        (f16::from_bits(row_u16[si + 1]).to_f32() * inv_mult).clamp(0.0, 1.0);
                    let b_lin =
                        (f16::from_bits(row_u16[si + 2]).to_f32() * inv_mult).clamp(0.0, 1.0);
                    let r = lut[(r_lin * scale + 0.5) as usize];
                    let g = lut[(g_lin * scale + 0.5) as usize];
                    let b = lut[(b_lin * scale + 0.5) as usize];
                    dst[di] = b;
                    dst[di + 1] = g;
                    dst[di + 2] = r;
                    dst[di + 3] = 255;
                }
            }
        }
    } else {
        unsafe {
            let base = mapped.pData as *const u8;
            for y in 0..h {
                let src = std::slice::from_raw_parts(base.add(y * pitch), w * 4);
                let dst = &mut bgra[y * w * 4..(y + 1) * w * 4];
                dst.copy_from_slice(src);
                for px in dst.chunks_exact_mut(4) {
                    px[3] = 255;
                }
            }
        }
    }

    unsafe { d3d_ctx.Unmap(&staging, 0) };
    let _ = frame.Close();
    let _ = session.Close();
    let _ = pool.Close();

    Ok(CapturedScreen {
        width: desc.Width,
        height: desc.Height,
        monitor_rect: mon_rect,
        bgra,
        hdr_info,
        capture_ms: t0.elapsed().as_secs_f64() * 1000.0,
    })
}
