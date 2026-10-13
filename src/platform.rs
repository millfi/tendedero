#![allow(unsafe_op_in_unsafe_fn)]
use crate::model::AppResult;
use std::{
    cell::RefCell,
    mem::size_of,
    path::{Path, PathBuf},
};
use std::{
    hash::{Hash, Hasher},
    sync::atomic::{AtomicU32, Ordering},
};
static SHOW_MESSAGE: AtomicU32 = AtomicU32::new(0);
pub struct Instance(HANDLE);
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
pub fn single_instance(root: &Path) -> AppResult<Option<Instance>> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    root.canonicalize()?
        .to_string_lossy()
        .to_lowercase()
        .hash(&mut hash);
    let name = format!("Local\\Snapline-{:x}", hash.finish());
    unsafe {
        let message = RegisterWindowMessageW(PCWSTR(wide(&name).as_ptr()));
        SHOW_MESSAGE.store(message, Ordering::Relaxed);
        let handle = CreateMutexW(None, false, PCWSTR(wide(name).as_ptr()))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = PostMessageW(Some(HWND_BROADCAST), message, WPARAM(0), LPARAM(0));
            let _ = CloseHandle(handle);
            return Ok(None);
        }
        Ok(Some(Instance(handle)))
    }
}
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        Storage::FileSystem::*,
        System::{Com::*, DataExchange::*, Memory::*, Ole::*, SystemServices::*, Threading::*},
        UI::{Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
    },
    core::*,
};

pub fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
pub fn pictures_folder() -> PathBuf {
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None) {
            let result = PathBuf::from(p.to_string().unwrap_or_default());
            CoTaskMemFree(Some(p.0.cast()));
            return result;
        }
    }
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Pictures")
}
pub fn replace_file(from: &Path, to: &Path) -> AppResult<()> {
    unsafe {
        MoveFileExW(
            PCWSTR(wide(from).as_ptr()),
            PCWSTR(wide(to).as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}
pub fn open(path: impl AsRef<std::ffi::OsStr>) -> AppResult<()> {
    unsafe {
        let h = ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide(path).as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
        if h.0 as usize <= 32 {
            return Err("ファイルを開けませんでした".into());
        }
    }
    Ok(())
}
pub fn edit(path: &Path) -> AppResult<()> {
    // Paint receives the private copy. Shell parameter quoting preserves paths with spaces.
    let arg = format!("\"{}\"", path.display());
    unsafe {
        let h = ShellExecuteW(
            None,
            w!("open"),
            w!("mspaint.exe"),
            PCWSTR(wide(arg).as_ptr()),
            None,
            SW_SHOWNORMAL,
        );
        if h.0 as usize <= 32 {
            return Err("ペイントを起動できませんでした".into());
        }
    }
    Ok(())
}
pub fn pick_files(owner: HWND, folder: bool) -> AppResult<Vec<PathBuf>> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let flags = if folder {
            FOS_PICKFOLDERS
        } else {
            FOS_ALLOWMULTISELECT
        };
        dialog.SetOptions(FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST | flags)?;
        if let Err(e) = dialog.Show(Some(owner)) {
            if e.code() == HRESULT(0x800704c7u32 as i32) {
                return Ok(vec![]);
            }
            return Err(e.into());
        }
        let items = dialog.GetResults()?;
        let mut result = vec![];
        for i in 0..items.GetCount()? {
            let value = items.GetItemAt(i)?.GetDisplayName(SIGDN_FILESYSPATH)?;
            result.push(PathBuf::from(value.to_string()?));
            CoTaskMemFree(Some(value.0.cast()));
        }
        Ok(result)
    }
}
pub fn save_as(owner: HWND, source: &Path) -> AppResult<bool> {
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT)?;
        dialog.SetFileName(PCWSTR(
            wide(source.file_name().unwrap_or_default()).as_ptr(),
        ))?;
        dialog.SetDefaultExtension(PCWSTR(
            wide(source.extension().unwrap_or_default()).as_ptr(),
        ))?;
        if let Err(e) = dialog.Show(Some(owner)) {
            if e.code() == HRESULT(0x800704c7u32 as i32) {
                return Ok(false);
            }
            return Err(e.into());
        }
        let p = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = PathBuf::from(p.to_string()?);
        CoTaskMemFree(Some(p.0.cast()));
        if path != source {
            std::fs::copy(source, path)?;
        }
        Ok(true)
    }
}

pub fn clipboard_sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}
struct ClipboardGuard;
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}
fn clipboard_lock(owner: HWND) -> AppResult<ClipboardGuard> {
    unsafe {
        OpenClipboard(Some(owner))?;
    }
    Ok(ClipboardGuard)
}
unsafe fn clipboard_bytes(format: u32) -> AppResult<Vec<u8>> {
    let handle = GetClipboardData(format)?;
    let global = HGLOBAL(handle.0);
    let size = GlobalSize(global);
    if size == 0 || size > 256 * 1024 * 1024 {
        return Err("クリップボード画像のサイズが不正です".into());
    }
    let ptr = GlobalLock(global);
    if ptr.is_null() {
        return Err("クリップボードを読み取れません".into());
    }
    let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), size).to_vec();
    let _ = GlobalUnlock(global);
    Ok(bytes)
}
pub fn read_clipboard(owner: HWND) -> AppResult<Option<image::DynamicImage>> {
    let _lock = clipboard_lock(owner)?;
    unsafe {
        // Do not collect mixed text/file payloads or password-manager/private formats.
        for format in [1, 13, 15] {
            if IsClipboardFormatAvailable(format).is_ok() {
                return Ok(None);
            }
        }
        for name in [
            "ExcludeClipboardContentFromMonitorProcessing",
            "CanIncludeInClipboardHistory",
            "CanUploadToCloudClipboard",
            "Clipboard Viewer Ignore",
            "application/x-keepassxc-secret",
            "org.nspasteboard.ConcealedType",
        ] {
            let f = RegisterClipboardFormatW(PCWSTR(wide(name).as_ptr()));
            if IsClipboardFormatAvailable(f).is_ok() {
                if name.starts_with("Can") {
                    if clipboard_bytes(f).is_ok_and(|b| b.len() >= 4 && b[..4] == [0, 0, 0, 0]) {
                        return Ok(None);
                    }
                } else {
                    return Ok(None);
                }
            }
        }
        let png = RegisterClipboardFormatW(w!("PNG"));
        if IsClipboardFormatAvailable(png).is_ok() {
            return Ok(Some(image::load_from_memory_with_format(
                &clipboard_bytes(png)?,
                image::ImageFormat::Png,
            )?));
        }
        for format in [17, 8] {
            if IsClipboardFormatAvailable(format).is_ok() {
                return Ok(Some(decode_dib(&clipboard_bytes(format)?)?));
            }
        }
    }
    Ok(None)
}
pub fn decode_dib(dib: &[u8]) -> AppResult<image::DynamicImage> {
    if dib.len() < 40 {
        return Err("DIBヘッダーが不正です".into());
    }
    let header = u32::from_le_bytes(dib[0..4].try_into()?) as usize;
    if header < 40 || header > dib.len() {
        return Err("DIBヘッダーが不正です".into());
    }
    let bpp = u16::from_le_bytes(dib[14..16].try_into()?);
    let compression = u32::from_le_bytes(dib[16..20].try_into()?);
    let colors = u32::from_le_bytes(dib[32..36].try_into()?) as usize;
    let palette = if colors != 0 {
        colors
    } else if bpp <= 8 {
        1usize << bpp
    } else {
        0
    };
    let masks = if header == 40 && compression == 3 {
        12
    } else if header == 40 && compression == 6 {
        16
    } else {
        0
    };
    let offset = 14usize
        .checked_add(header)
        .and_then(|n| n.checked_add(palette.checked_mul(4)?))
        .and_then(|n| n.checked_add(masks))
        .ok_or("DIBサイズが不正です")?;
    if offset > dib.len() + 14 {
        return Err("DIBサイズが不正です".into());
    }
    let mut bmp = vec![0u8; 14];
    bmp[0..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    bmp[10..14].copy_from_slice(&(offset as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    Ok(image::load_from_memory_with_format(
        &bmp,
        image::ImageFormat::Bmp,
    )?)
}
unsafe fn set_clipboard_bytes(format: u32, bytes: &[u8]) -> AppResult<()> {
    let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
    let p = GlobalLock(h);
    if p.is_null() {
        let _ = GlobalFree(Some(h));
        return Err("画像用メモリを確保できません".into());
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast(), bytes.len());
    let _ = GlobalUnlock(h);
    if let Err(e) = SetClipboardData(format, Some(HANDLE(h.0))) {
        let _ = GlobalFree(Some(h));
        return Err(e.into());
    }
    Ok(())
}
pub fn copy_image(owner: HWND, path: &Path) -> AppResult<()> {
    let img = crate::model::read_image(path)?.to_rgb8();
    let mut bmp = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_to(&mut bmp, image::ImageFormat::Bmp)?;
    let png = std::fs::read(path)?;
    let _lock = clipboard_lock(owner)?;
    unsafe {
        EmptyClipboard()?;
        set_clipboard_bytes(8, &bmp.into_inner()[14..])?;
        set_clipboard_bytes(RegisterClipboardFormatW(w!("PNG")), &png)?;
    }
    Ok(())
}

pub fn capture_rect(rect: RECT) -> AppResult<image::DynamicImage> {
    unsafe {
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        if w <= 0 || h <= 0 || w > 16384 || h > 16384 || (w as u64 * h as u64) > 128 * 1024 * 1024 {
            return Err("キャプチャ範囲が大きすぎます".into());
        }
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err("画面を取得できません".into());
        }
        let dc = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        if dc.is_invalid() || bmp.is_invalid() {
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            return Err("キャプチャ用メモリを確保できません".into());
        }
        let old = SelectObject(dc, bmp.into());
        let capture = BitBlt(
            dc,
            0,
            0,
            w,
            h,
            Some(screen),
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        );
        SelectObject(dc, old);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        let lines = if capture.is_ok() {
            GetDIBits(
                dc,
                bmp,
                0,
                h as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        capture?;
        if lines != h {
            return Err("画面の読み取りに失敗しました".into());
        }
        for px in pixels.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
            px[3] = 255;
        }
        Ok(image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(w as u32, h as u32, pixels).ok_or("画像サイズが不正です")?,
        ))
    }
}
pub fn monitor_rect(work: bool) -> RECT {
    monitor_rect_at(cursor(), work)
}
pub fn desktop_rect() -> RECT {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        RECT {
            left: x,
            top: y,
            right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}
pub fn monitor_rect_at(p: POINT, work: bool) -> RECT {
    unsafe {
        let monitor = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        if work { info.rcWork } else { info.rcMonitor }
    }
}
pub fn cursor() -> POINT {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    p
}

pub fn fullscreen_foreground(own: HWND) -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground == own || foreground.is_invalid() || IsZoomed(foreground).as_bool() {
            return false;
        }
        let mut class = [0u16; 128];
        let n = GetClassNameW(foreground, &mut class);
        let class = String::from_utf16_lossy(&class[..n as usize]);
        if ["Progman", "WorkerW", "Shell_TrayWnd"].contains(&class.as_str()) {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(foreground, &mut r).is_err() {
            return false;
        }
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(
            MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST),
            &mut mi,
        );
        r.left <= mi.rcMonitor.left
            && r.top <= mi.rcMonitor.top
            && r.right >= mi.rcMonitor.right
            && r.bottom >= mi.rcMonitor.bottom
    }
}

#[derive(Clone)]
pub enum NativeEvent {
    Selected(RECT),
    CaptureCancelled,
    Overlay(crate::clothesline::Event),
    Show,
    Tick,
    Toggle,
    Settings,
    CloseSettings,
    Capture,
    Exit,
}
type NativeCallback = Box<dyn Fn(NativeEvent)>;
thread_local! { static CALLBACK: RefCell<Option<NativeCallback>> = RefCell::new(None); }
pub(crate) fn emit(event: NativeEvent) {
    CALLBACK.with(|c| {
        if let Some(f) = c.borrow().as_ref() {
            f(event);
        }
    });
}
const TRAY: u32 = WM_APP + 43;
unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == SHOW_MESSAGE.load(Ordering::Relaxed) && msg != 0 {
        emit(NativeEvent::Show);
        return LRESULT(0);
    }
    match msg {
        WM_TIMER if w.0 == 0x534e4150 => {
            emit(NativeEvent::Tick);
            return LRESULT(0);
        }
        WM_HOTKEY => {
            emit(if w.0 == 2 {
                NativeEvent::Capture
            } else {
                NativeEvent::Toggle
            });
            return LRESULT(0);
        }
        WM_CLOSE => {
            let _ = ShowWindow(hwnd, SW_HIDE);
            emit(NativeEvent::CloseSettings);
            return LRESULT(0);
        }
        TRAY => {
            match l.0 as u32 & 0xffff {
                WM_LBUTTONUP | NIN_SELECT => emit(NativeEvent::Toggle),
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    if let Ok(menu) = CreatePopupMenu() {
                        let _ = AppendMenuW(menu, MF_STRING, 1, w!("棚を表示 / 隠す"));
                        let _ = AppendMenuW(menu, MF_STRING, 2, w!("範囲を撮影"));
                        let _ = AppendMenuW(menu, MF_STRING, 3, w!("設定"));
                        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                        let _ = AppendMenuW(menu, MF_STRING, 4, w!("Snaplineを終了"));
                        let p = cursor();
                        let _ = SetForegroundWindow(hwnd);
                        let command = TrackPopupMenu(
                            menu,
                            TPM_RETURNCMD | TPM_RIGHTBUTTON,
                            p.x,
                            p.y,
                            None,
                            hwnd,
                            None,
                        )
                        .0;
                        let _ = DestroyMenu(menu);
                        match command {
                            1 => emit(NativeEvent::Toggle),
                            2 => emit(NativeEvent::Capture),
                            3 => emit(NativeEvent::Settings),
                            4 => emit(NativeEvent::Exit),
                            _ => (),
                        }
                    }
                }
                _ => (),
            }
            return LRESULT(0);
        }
        _ => (),
    }
    DefSubclassProc(hwnd, msg, w, l)
}
pub struct Host {
    pub hwnd: HWND,
    pub hotkey_ok: bool,
    pub line: RefCell<crate::clothesline::Clothesline>,
    settings: std::cell::Cell<bool>,
    icon: HICON,
}
pub fn icon_path() -> &'static str {
    static PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        std::env::current_exe()
            .unwrap_or_default()
            .with_file_name("snapline.ico")
            .to_string_lossy()
            .to_string()
    })
}
impl Host {
    pub fn attach(callback: impl Fn(NativeEvent) + 'static) -> AppResult<Self> {
        unsafe extern "system" fn find(hwnd: HWND, param: LPARAM) -> BOOL {
            let mut title = [0u16; 256];
            let n = unsafe { GetWindowTextW(hwnd, &mut title) };
            if String::from_utf16_lossy(&title[..n as usize]) == "Snapline" {
                unsafe {
                    *(param.0 as *mut HWND) = hwnd;
                }
                return false.into();
            }
            true.into()
        }
        unsafe {
            let mut hwnd = HWND::default();
            let _ = EnumThreadWindows(
                GetCurrentThreadId(),
                Some(find),
                LPARAM((&mut hwnd as *mut HWND) as isize),
            );
            if hwnd.is_invalid() {
                return Err("WinUIウィンドウが見つかりません".into());
            }
            CALLBACK.with(|c| *c.borrow_mut() = Some(Box::new(callback)));
            SetWindowSubclass(hwnd, Some(subclass), 1, 0).ok()?;
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            SetWindowLongPtrW(
                hwnd,
                GWL_STYLE,
                (style & !(WS_CAPTION.0 | WS_THICKFRAME.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0))
                    as isize,
            );
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                ((GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 | WS_EX_APPWINDOW.0)
                    & !WS_EX_TOOLWINDOW.0) as isize,
            );
            let hotkey_ok = RegisterHotKey(
                Some(hwnd),
                1,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                b'T' as u32,
            )
            .is_ok();
            let _ = RegisterHotKey(
                Some(hwnd),
                2,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                b'S' as u32,
            );
            SetTimer(Some(hwnd), 0x534e4150, 100, None);
            let icon = HICON(
                LoadImageW(
                    None,
                    PCWSTR(wide(icon_path()).as_ptr()),
                    IMAGE_ICON,
                    32,
                    32,
                    LR_LOADFROMFILE,
                )?
                .0,
            );
            let mut nid = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: TRAY,
                hIcon: icon,
                ..Default::default()
            };
            for (d, s) in nid
                .szTip
                .iter_mut()
                .zip("Snapline · Ctrl+Alt+T".encode_utf16())
            {
                *d = s;
            }
            Shell_NotifyIconW(NIM_ADD, &nid).ok()?;
            let line = crate::clothesline::Clothesline::new()?;
            let _ = ShowWindow(hwnd, SW_HIDE);
            Ok(Self {
                hwnd,
                hotkey_ok,
                icon,
                line: RefCell::new(line),
                settings: std::cell::Cell::new(false),
            })
        }
    }
    pub fn settings(&self, value: bool) {
        self.settings.set(value);
        if value {
            self.line.borrow_mut().hide();
        } else {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }
    pub fn place(&self, height: i32) {
        if !self.settings.get() {
            return;
        }
        unsafe {
            let r = monitor_rect(true);
            let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(self.hwnd).max(96) as f64 / 96.;
            let width = (r.right - r.left - (32. * scale) as i32).min((820. * scale) as i32);
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                r.left + (r.right - r.left - width) / 2,
                r.top + ((r.bottom - r.top - (height as f64 * scale) as i32) / 2).max(0),
                width,
                (height as f64 * scale) as i32,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
    pub fn show(&self) {
        if !self.settings.get() {
            self.line.borrow_mut().show();
            return;
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }
    pub fn hide(&self) {
        self.settings.set(false);
        self.line.borrow_mut().hide();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }
    pub fn visible(&self) -> bool {
        if !self.settings.get() {
            return self.line.borrow().visible();
        }
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }
    pub fn contains_cursor(&self) -> bool {
        if !self.settings.get() {
            return self.line.borrow().contains_cursor();
        }
        unsafe {
            let mut r = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut r);
            let p = cursor();
            p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), 0x534e4150);
            let _ = UnregisterHotKey(Some(self.hwnd), 1);
            let _ = UnregisterHotKey(Some(self.hwnd), 2);
            let nid = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            let _ = DestroyIcon(self.icon);
            let _ = RemoveWindowSubclass(self.hwnd, Some(subclass), 1);
            CALLBACK.with(|c| *c.borrow_mut() = None);
        }
    }
}

#[implement(IDropSource)]
struct DropSource;
impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }
    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}
pub fn drag_file(path: &Path) -> AppResult<()> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide(path).as_ptr()), None)?;
        let object: IDataObject = item.BindToHandler(None, &BHID_DataObject)?;
        let source: IDropSource = DropSource.into();
        let mut effect = DROPEFFECT_NONE;
        let hr = DoDragDrop(&object, &source, DROPEFFECT_COPY, &mut effect);
        if hr.is_err() {
            return Err(windows::core::Error::from_hresult(hr).into());
        }
        Ok(())
    }
}

/// Native popup avoids the preview Reactor runtime's nested context-flyout crash.
pub fn item_menu(owner: HWND, pinned: bool) -> Option<u32> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for (id, label) in [
            (1, "コピー"),
            (2, "マークアップ"),
            (3, "既定のアプリで開く"),
            (4, "名前を付けて保存…"),
            (
                5,
                if pinned {
                    "固定を解除"
                } else {
                    "棚に固定"
                },
            ),
            (6, "棚から外す"),
        ] {
            let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(wide(label).as_ptr()));
        }
        let p = cursor();
        let _ = SetForegroundWindow(owner);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            p.x,
            p.y,
            None,
            owner,
            None,
        )
        .0 as u32;
        let _ = DestroyMenu(menu);
        (command != 0).then_some(command)
    }
}
