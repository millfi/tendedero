//! One-shot Windows.Graphics.Capture snapshots in physical desktop coordinates.
//! The UI keeps its synchronous snapshot contract; all capture objects and D3D
//! readback live on a temporary MTA thread, with free-threaded frame delivery.
use crate::model::AppResult;
use image::{DynamicImage, Rgba, RgbaImage};
use std::{
    sync::{
        OnceLock,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};
use windows::{
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{
            Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
            GraphicsCaptureSession,
        },
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::{FreeLibrary, HMODULE, LPARAM, RECT},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
            Direct3D11::*,
            Dxgi::{Common::DXGI_FORMAT_B8G8R8A8_UNORM, IDXGIDevice},
            Gdi::{EnumDisplayMonitors, HDC, HMONITOR},
        },
        System::{
            LibraryLoader::{
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN,
                GetModuleHandleExW, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
            },
            WinRT::{
                Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
                Graphics::Capture::IGraphicsCaptureItemInterop,
                RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize,
            },
        },
        UI::HiDpi::{
            DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            SetThreadDpiAwarenessContext,
        },
    },
    core::{BOOL, Interface, PCWSTR, Result, factory, w},
};

const FRAME_TIMEOUT: Duration = Duration::from_secs(3);

fn pin_capture_runtime() -> Result<()> {
    static PINNED: OnceLock<Result<()>> = OnceLock::new();
    PINNED
        .get_or_init(|| unsafe {
            // WGC can still run internal callbacks after Close. RoUninitialize can
            // unload GraphicsCapture.dll underneath them on some Windows builds.
            // Pin only the system DLL for this process, while still closing every
            // capture resource and balancing apartment initialization normally.
            // https://github.com/robmikh/Win32CaptureSample/issues/99
            let module = LoadLibraryExW(
                w!("GraphicsCapture.dll"),
                None,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )?;
            let mut pinned = HMODULE::default();
            let result = GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                PCWSTR(module.0.cast()),
                &mut pinned,
            );
            let _ = FreeLibrary(module);
            result
        })
        .clone()
}

pub fn capture_rect(rect: RECT) -> AppResult<DynamicImage> {
    dimensions(rect)?;
    // Box<dyn Error> is local to the UI, so only pixels or an error string cross
    // this boundary. No WinRT or D3D objects escape their initialized apartment.
    std::thread::Builder::new()
        .name("screen-capture".into())
        .spawn(move || snapshot(rect).map_err(|e| e.to_string()))?
        .join()
        .map_err(|_| "キャプチャ処理が終了しました")?
        .map_err(Into::into)
}

struct Apartment;
impl Apartment {
    fn new() -> Result<Self> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

struct DpiContext(DPI_AWARENESS_CONTEXT);
impl DpiContext {
    fn new() -> Result<Self> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.0.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        Ok(Self(previous))
    }
}
impl Drop for DpiContext {
    fn drop(&mut self) {
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

fn dimensions(rect: RECT) -> AppResult<(u32, u32)> {
    let w = i64::from(rect.right) - i64::from(rect.left);
    let h = i64::from(rect.bottom) - i64::from(rect.top);
    if w <= 0 || h <= 0 || w > 16384 || h > 16384 || w * h > 128 * 1024 * 1024 {
        return Err("キャプチャ範囲が大きすぎるか不正です".into());
    }
    Ok((w as u32, h as u32))
}

fn intersection(a: RECT, b: RECT) -> Option<RECT> {
    let r = RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (r.left < r.right && r.top < r.bottom).then_some(r)
}

struct Monitor {
    handle: HMONITOR,
    bounds: RECT,
}

fn monitors(rect: RECT) -> AppResult<Vec<Monitor>> {
    unsafe extern "system" fn enumerate(
        handle: HMONITOR,
        _: HDC,
        bounds: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // EnumDisplayMonitors invokes this synchronously; data and bounds are
        // valid for the call. With no HDC, bounds are desktop pixel coordinates.
        unsafe {
            let monitors = &mut *(data.0 as *mut Vec<Monitor>);
            monitors.push(Monitor {
                handle,
                bounds: *bounds,
            });
        }
        BOOL(1)
    }
    let mut monitors = Vec::<Monitor>::new();
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(enumerate),
            LPARAM((&mut monitors as *mut Vec<Monitor>) as isize),
        )
        .ok()?;
    }
    monitors.retain(|m| intersection(rect, m.bounds).is_some());
    if monitors.is_empty() {
        return Err("キャプチャ範囲にモニターがありません".into());
    }
    Ok(monitors)
}

struct Device {
    native: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
}
impl Device {
    fn new() -> Result<Self> {
        let mut native = None;
        let mut context = None;
        let mut create = |driver| unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut native),
                None,
                Some(&mut context),
            )
        };
        if create(D3D_DRIVER_TYPE_HARDWARE).is_err() {
            create(D3D_DRIVER_TYPE_WARP)?;
        }
        let native = native.ok_or_else(windows::core::Error::from_thread)?;
        let dxgi: IDXGIDevice = native.cast()?;
        let winrt = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()? };
        Ok(Self {
            native,
            context: context.ok_or_else(windows::core::Error::from_thread)?,
            winrt,
        })
    }

    fn read(&self, frame: &Direct3D11CaptureFrame, bounds: RECT) -> AppResult<RgbaImage> {
        let (w, h) = dimensions(bounds)?;
        let content = frame.ContentSize()?;
        if content.Width != w as i32 || content.Height != h as i32 {
            return Err("撮影中にモニターのサイズが変わりました。もう一度撮影してください".into());
        }
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        if desc.Width < w || desc.Height < h || desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err("キャプチャ画像のサイズまたは形式が不正です".into());
        }
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        unsafe {
            self.native
                .CreateTexture2D(&desc, None, Some(&mut staging))?
        };
        let staging = staging.ok_or("キャプチャ画像用メモリを確保できません")?;
        unsafe { self.context.CopyResource(&staging, &texture) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?
        };
        // Always unmap, including validation failures in the row converter.
        let result = (|| {
            if mapped.pData.is_null() || mapped.RowPitch < w * 4 {
                return Err("キャプチャ画像の行サイズが不正です".into());
            }
            let size = mapped.RowPitch as usize * (h as usize - 1) + w as usize * 4;
            let bytes = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), size) };
            bgra_to_rgba(bytes, mapped.RowPitch as usize, w, h)
        })();
        unsafe { self.context.Unmap(&staging, 0) };
        result
    }
}

// WinRT Close releases capture resources deterministically on every error path,
// rather than waiting for the final COM reference held by an event delegate.
struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: Option<GraphicsCaptureSession>,
    token: Option<i64>,
    frames: Receiver<Result<Frame>>,
    bounds: RECT,
}
struct Frame(Direct3D11CaptureFrame);
impl Drop for Frame {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}
impl Capture {
    fn new(device: &IDirect3DDevice, monitor: Monitor) -> AppResult<Self> {
        let interop: IGraphicsCaptureItemInterop = factory::<GraphicsCaptureItem, _>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor.handle)? };
        let size = item.Size()?;
        let (w, h) = dimensions(monitor.bounds)?;
        if size.Width != w as i32 || size.Height != h as i32 {
            return Err("モニターのサイズが変わりました。もう一度撮影してください".into());
        }
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            1,
            size,
        )?;
        let (sender, frames) = mpsc::sync_channel(1);
        let mut capture = Self {
            pool,
            session: None,
            token: None,
            frames,
            bounds: monitor.bounds,
        };
        capture.session = Some(capture.pool.CreateCaptureSession(&item)?);
        capture
            .session
            .as_ref()
            .unwrap()
            .SetIsCursorCaptureEnabled(false)?;
        capture.token = Some(capture.pool.FrameArrived(&TypedEventHandler::<
            Direct3D11CaptureFramePool,
            windows::core::IInspectable,
        >::new(move |pool, _| {
            let frame = pool
                .as_ref()
                .ok_or_else(windows::core::Error::from_thread)
                .and_then(|p| p.TryGetNextFrame())
                .map(Frame);
            // Never block the frame-pool thread. Extra frames are closed by
            // Frame's guard if the single-frame channel is already full.
            let _ = sender.try_send(frame);
            Ok(())
        }))?);
        Ok(capture)
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            let _ = self.pool.RemoveFrameArrived(token);
        }
        if let Some(session) = &self.session {
            let _ = session.Close();
        }
        // Close any queued frames before returning their buffers to the pool.
        while let Ok(frame) = self.frames.try_recv() {
            drop(frame);
        }
        let _ = self.pool.Close();
    }
}

fn snapshot(rect: RECT) -> AppResult<DynamicImage> {
    pin_capture_runtime()?;
    let _apartment = Apartment::new()?;
    let _dpi = DpiContext::new()?;
    if !GraphicsCaptureSession::IsSupported()? {
        return Err("この環境は Windows.Graphics.Capture に対応していません".into());
    }
    let monitors = monitors(rect)?;
    let device = Device::new()?;
    let captures = monitors
        .into_iter()
        .map(|m| Capture::new(&device.winrt, m))
        .collect::<AppResult<Vec<_>>>()?;
    let deadline = Instant::now() + FRAME_TIMEOUT;
    // Start every monitor before waiting so the frozen desktop is assembled
    // from their first frames, rather than capturing displays sequentially.
    for capture in &captures {
        capture.session.as_ref().unwrap().StartCapture()?;
    }
    let (w, h) = dimensions(rect)?;
    let mut image = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
    for capture in &captures {
        let frame = capture
            .frames
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| "画面の取得がタイムアウトしました。もう一度撮影してください")??;
        let pixels = device.read(&frame.0, capture.bounds)?;
        paste_monitor(&mut image, rect, &pixels, capture.bounds)?;
    }
    Ok(DynamicImage::ImageRgba8(image))
}

fn bgra_to_rgba(bytes: &[u8], pitch: usize, w: u32, h: u32) -> AppResult<RgbaImage> {
    let row = w as usize * 4;
    if w == 0 || h == 0 || pitch < row || bytes.len() < pitch * (h as usize - 1) + row {
        return Err("キャプチャ画像の行サイズが不正です".into());
    }
    let mut image = RgbaImage::new(w, h);
    for (y, out) in image.as_mut().chunks_exact_mut(row).enumerate() {
        for (src, dst) in bytes[y * pitch..y * pitch + row]
            .as_chunks::<4>()
            .0
            .iter()
            .zip(out.as_chunks_mut::<4>().0)
        {
            dst.copy_from_slice(&[src[2], src[1], src[0], 255]);
        }
    }
    Ok(image)
}

fn paste_monitor(
    image: &mut RgbaImage,
    rect: RECT,
    pixels: &RgbaImage,
    bounds: RECT,
) -> AppResult<()> {
    if image.dimensions() != dimensions(rect)? || pixels.dimensions() != dimensions(bounds)? {
        return Err("キャプチャ画像のサイズが不正です".into());
    }
    if let Some(part) = intersection(rect, bounds) {
        let width = (part.right - part.left) as u32;
        let height = (part.bottom - part.top) as u32;
        let cropped = image::imageops::crop_imm(
            pixels,
            (part.left - bounds.left) as u32,
            (part.top - bounds.top) as u32,
            width,
            height,
        );
        image::imageops::replace(
            image,
            &cropped.to_image(),
            i64::from(part.left) - i64::from(rect.left),
            i64::from(part.top) - i64::from(rect.top),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn padded_bgra_rows_keep_orientation_and_use_opaque_rgba() {
        let bytes = [
            3, 2, 1, 0, 6, 5, 4, 42, 99, 99, 99, 99, 9, 8, 7, 128, 12, 11, 10, 255,
        ];
        let image = bgra_to_rgba(&bytes, 12, 2, 2).unwrap();
        assert_eq!(
            image.into_raw(),
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
        assert!(bgra_to_rgba(&bytes[..19], 12, 2, 2).is_err());
        assert!(bgra_to_rgba(&bytes, 7, 2, 2).is_err());
    }

    #[test]
    fn stitch_crops_negative_origins_and_keeps_desktop_gaps_black() {
        let bounds = rect(-2, -1, 3, 2);
        let mut image = RgbaImage::from_pixel(5, 3, Rgba([0, 0, 0, 255]));
        let left = RgbaImage::from_fn(3, 3, |x, y| Rgba([x as u8, y as u8, 10, 255]));
        paste_monitor(&mut image, bounds, &left, rect(-3, -2, 0, 1)).unwrap();
        let right = RgbaImage::from_pixel(3, 2, Rgba([20, 30, 40, 255]));
        paste_monitor(&mut image, bounds, &right, rect(1, 0, 4, 2)).unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [1, 1, 10, 255]);
        assert_eq!(image.get_pixel(1, 1).0, [2, 2, 10, 255]);
        assert_eq!(image.get_pixel(2, 1).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(3, 0).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(4, 2).0, [20, 30, 40, 255]);
    }

    #[test]
    fn invalid_and_overflowing_bounds_are_rejected_before_capture() {
        assert!(dimensions(rect(0, 0, 0, 1)).is_err());
        assert!(dimensions(rect(0, 2, 1, 1)).is_err());
        assert!(dimensions(rect(i32::MIN, 0, i32::MAX, 1)).is_err());
        assert!(dimensions(rect(0, 0, 16384, 16384)).is_err());
        assert_eq!(
            dimensions(rect(-1920, -1080, 1920, 1080)).unwrap(),
            (3840, 2160)
        );
    }

    #[test]
    #[ignore = "requires an unlocked interactive Windows desktop; briefly shows capture borders"]
    fn native_snapshot_reads_monitor_and_desktop() {
        // The test runner does not initialize Reactor's process DPI awareness.
        let _dpi = DpiContext::new().unwrap();
        let monitor = crate::platform::monitor_rect(false);
        let cropped = rect(
            monitor.left + 1,
            monitor.top + 1,
            monitor.right - 1,
            monitor.bottom - 1,
        );
        // Repeated apartment teardown is a regression check for the runtime
        // unloading race; also exercise partial-monitor GPU readback/cropping.
        for _ in 0..3 {
            for bounds in [monitor, crate::platform::desktop_rect(), cropped] {
                let image = capture_rect(bounds).unwrap().to_rgba8();
                assert_eq!(image.dimensions(), dimensions(bounds).unwrap());
                assert!(image.pixels().all(|p| p[3] == 255));
                assert!(image.pixels().any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0));
            }
        }
    }
}
