//! A transparent desktop Composition target. WinUI owns the settings and editors;
//! this HWND contains only the rope and individually animated photographs.
use crate::{
    model::Item,
    platform::{self, NativeEvent},
};
use image::{Rgba, RgbaImage};
use std::{
    cell::RefCell,
    collections::HashMap,
    mem::size_of,
    time::{Duration, Instant},
};
use windows::{
    Foundation::{Size, TimeSpan},
    Graphics::DirectX::*,
    System::DispatcherQueueController,
    UI::Composition::*,
    Win32::{
        Foundation::*,
        Graphics::{
            Direct2D::{Common::*, *},
            Direct3D::*,
            Direct3D11::*,
            DirectWrite::*,
            Dxgi::{Common::*, *},
            Gdi::ScreenToClient,
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            WinRT::{Composition::*, *},
        },
        UI::{
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
            WindowsAndMessaging::*,
        },
    },
    core::*,
};
use windows_numerics::{Vector2, Vector3};

#[derive(Clone)]
pub enum Event {
    Copy(String),
    Open(String),
    Edit(String),
    Context(String),
    Dismiss(String),
    Drag(String),
    Page(i32),
    Settings,
    Hide,
}
fn send(e: Event) {
    platform::emit(NativeEvent::Overlay(e));
}
fn v3(x: f32, y: f32, z: f32) -> Vector3 {
    Vector3 { X: x, Y: y, Z: z }
}

struct Card {
    factor: f32,
    id: String,
    visual: SpriteVisual,
    close: SpriteVisual,
    badge: SpriteVisual,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    tilt: f32,
}
struct State {
    cards: Vec<Card>,
    scale: f32,
    hovered: Option<usize>,
    pressed: Option<(usize, POINT, Instant)>,
    last_click: Option<(String, Instant)>,
    copied: Option<(usize, Instant)>,
}
pub struct Clothesline {
    hwnd: HWND,
    state: Box<State>,
    compositor: Compositor,
    graphics: CompositionGraphicsDevice,
    root: ContainerVisual,
    _scene: ContainerVisual,
    source: CompositionVisualSurface,
    regions: RefCell<crate::composition_regions::RegionWindows>,
    _queue: Option<DispatcherQueueController>,
    cache: HashMap<String, (String, SpriteVisual, SpriteVisual, SpriteVisual, f32, f32)>,
    signature: String,
    size: (i32, i32),
    hidden_at: Option<Instant>,
    region_at: Option<Instant>,
    shown: bool,
    capacity: usize,
    monitor_anchor: POINT,
}
pub struct Landing {
    pub factor: f32,
    pub visual: SpriteVisual,
    pub anchor: Vector3,
    pub scale: f32,
    pub width: f32,
    pub height: f32,
    pub tilt: f32,
}
impl Clothesline {
    pub fn compositor(&self) -> &Compositor {
        &self.compositor
    }
    pub fn set_monitor(&mut self, point: POINT) {
        self.monitor_anchor = point;
    }
    pub fn landing(&self, id: &str) -> Option<Landing> {
        let c = self.state.cards.iter().find(|c| c.id == id)?;
        let mut rect = RECT::default();
        unsafe {
            GetWindowRect(self.hwnd, &mut rect).ok()?;
        }
        let _ = c.visual.StopAnimation(h!("RotationAngleInDegrees"));
        let _ = c.visual.SetRotationAngleInDegrees(c.tilt);
        Some(Landing {
            factor: c.factor,
            visual: c.visual.clone(),
            anchor: v3(
                rect.left as f32 + (c.x + c.w / 2.) * self.state.scale,
                rect.top as f32 + (c.y + 20.) * self.state.scale,
                0.,
            ),
            scale: self.state.scale,
            width: c.w,
            height: c.h,
            tilt: c.tilt,
        })
    }
    pub fn new() -> Result<Self> {
        unsafe {
            let queue = if windows::System::DispatcherQueue::GetForCurrentThread().is_err() {
                Some(CreateDispatcherQueueController(DispatcherQueueOptions {
                    dwSize: size_of::<DispatcherQueueOptions>() as u32,
                    threadType: DQTYPE_THREAD_CURRENT,
                    apartmentType: DQTAT_COM_NONE,
                })?)
            } else {
                None
            };
            let compositor = Compositor::new()?;
            let mut d3d = None;
            let create = |driver, device: &mut Option<ID3D11Device>| {
                D3D11CreateDevice(
                    None,
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(device),
                    None,
                    None,
                )
            };
            if create(D3D_DRIVER_TYPE_HARDWARE, &mut d3d).is_err() {
                create(D3D_DRIVER_TYPE_WARP, &mut d3d)?;
            }
            let dxgi: IDXGIDevice = d3d.unwrap().cast()?;
            let d2d = D2D1CreateDevice(&dxgi, None)?;
            let interop: ICompositorInterop = compositor.cast()?;
            let graphics = interop.CreateGraphicsDevice(&d2d)?;
            let root = compositor.CreateContainerVisual()?;
            let scene = compositor.CreateContainerVisual()?;
            scene.Children()?.InsertAtTop(&root)?;
            let source = compositor.CreateVisualSurface()?;
            source.SetSourceVisual(&scene)?;
            let module = GetModuleHandleW(None)?;
            let class = w!("SnaplineTransparentClothesline");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: module.into(),
                lpszClassName: class,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                ..Default::default()
            });
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | if std::env::args().any(|a| a == "--ui-test") {
                        WS_EX_APPWINDOW
                    } else {
                        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
                    },
                class,
                w!("Snapline — Clothesline"),
                WS_POPUP,
                0,
                0,
                800,
                260,
                None,
                None,
                Some(module.into()),
                None,
            )?;
            // This controller only supplies coordinates, timer and mouse
            // capture. Separate Composition targets render the content.
            if let Err(error) = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA) {
                let _ = DestroyWindow(hwnd);
                return Err(error);
            }
            let mut state = Box::new(State {
                cards: vec![],
                scale: 1.,
                hovered: None,
                pressed: None,
                last_click: None,
                copied: None,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&mut *state as *mut State) as isize);
            SetTimer(Some(hwnd), 1, 30, None);
            Ok(Self {
                hwnd,
                state,
                compositor,
                graphics,
                root,
                _scene: scene,
                source,
                regions: RefCell::default(),
                _queue: queue,
                cache: HashMap::new(),
                signature: String::new(),
                size: (0, 0),
                hidden_at: None,
                region_at: None,
                shown: false,
                capacity: 1,
                monitor_anchor: platform::cursor(),
            })
        }
    }
    pub fn visible(&self) -> bool {
        self.shown
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn interacting(&self) -> bool {
        self.state.pressed.is_some()
    }
    pub fn show(&mut self) {
        self.hidden_at = None;
        if !self.shown {
            self.shown = true;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            let _ = self.set_region(true);
            self.region_at = Some(Instant::now() + Duration::from_millis(450));
            let _ = self.slide(-260., 0., 420);
        }
    }
    pub fn hide(&mut self) {
        if self.shown {
            self.shown = false;
            self.hidden_at = Some(Instant::now() + Duration::from_millis(230));
            let _ = self.set_region(true);
            let _ = self.slide(0., -260., 220);
        }
    }
    pub fn tick(&mut self) {
        if self.region_at.is_some_and(|t| Instant::now() > t) {
            let _ = self.set_region(false);
            self.region_at = None;
        }
        if self.hidden_at.is_some_and(|t| Instant::now() > t) {
            self.regions.borrow().hide();
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.hidden_at = None;
        }
    }
    fn slide(&self, from: f32, to: f32, ms: i64) -> Result<()> {
        let a = self.compositor.CreateScalarKeyFrameAnimation()?;
        a.InsertKeyFrame(0., from)?;
        let ease = self.compositor.CreateCubicBezierEasingFunction(
            Vector2 { X: 0.18, Y: 0.8 },
            Vector2 { X: 0.25, Y: 1. },
        )?;
        a.InsertKeyFrameWithEasingFunction(1., to, &ease)?;
        a.SetDuration(TimeSpan {
            Duration: ms * 10000,
        })?;
        self.root.StartAnimation(h!("Offset.Y"), &a)
    }
    pub fn contains_cursor(&self) -> bool {
        unsafe {
            let p = platform::cursor();
            let mut r = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut r);
            p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
        }
    }
    pub fn copied(&mut self, id: &str) {
        if let Some(n) = self.state.cards.iter().position(|c| c.id == id) {
            let c = &self.state.cards[n];
            let _ = c.badge.SetOpacity(1.);
            let _ = swing(&self.compositor, &c.visual, c.tilt, 3.);
            self.state.copied = Some((n, Instant::now()));
        }
    }
    pub fn sync(&mut self, items: &[Item], size: u32, page: usize) -> Result<()> {
        let monitor = platform::monitor_rect_at(self.monitor_anchor, true);
        // Move first so GetDpiForWindow reflects the destination monitor.
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                monitor.left,
                monitor.top,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
            )?;
        }
        let scale = unsafe { GetDpiForWindow(self.hwnd).max(96) } as f32 / 96.;
        let width = (monitor.right - monitor.left) as f32 / scale;
        // Preserve the original 150 × 104 pt photo area at the medium size.
        let factor = match size {
            0..=150 => 0.8,
            151..=200 => 1.,
            _ => 1.35,
        };
        let spacing = 150. * factor + 24.;
        let capacity = ((width - 80.) / spacing).floor().max(1.) as usize;
        self.capacity = capacity;
        let visible: Vec<_> = items
            .iter()
            .rev()
            .skip(page * capacity)
            .take(capacity)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let signature = format!(
            "{width}:{scale}:{size}:{page}:{}:{}:{}",
            monitor.left,
            monitor.top,
            visible
                .iter()
                .map(|i| format!(
                    "{}:{}:{:?}",
                    i.id,
                    i.pinned,
                    std::fs::metadata(&i.path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                ))
                .collect::<Vec<_>>()
                .join("|")
        );
        if signature == self.signature {
            return Ok(());
        }
        let height = (260. * scale) as i32;
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                monitor.left,
                monitor.top,
                monitor.right - monitor.left,
                height,
                SWP_NOACTIVATE,
            )?;
        }

        self.size = (monitor.right - monitor.left, height);
        self.state.scale = scale;
        self.root.SetScale(v3(scale, scale, 1.))?;
        self.root.Children()?.RemoveAll()?;
        self.state.cards.clear();
        self.state.hovered = None;
        self.state.pressed = None;
        self.state.copied = None;
        let rope = self.sprite(&rope_image(width), width, 64., None)?;
        self.root.Children()?.InsertAtTop(&rope)?;
        let count = visible.len();
        for (n, item) in visible.into_iter().enumerate() {
            let key = format!(
                "{}:{factor}:{}:{:?}",
                item.id,
                item.pinned,
                std::fs::metadata(&item.path)
                    .ok()
                    .and_then(|m| m.modified().ok())
            );
            let is_new = !self.cache.contains_key(&item.id);
            if !self.cache.get(&item.id).is_some_and(|v| v.0 == key) {
                let (pixels, w, h) = card_image(item, factor, true);
                let visual = self.sprite(&pixels, w, h, None)?;
                visual.SetCenterPoint(v3(w / 2., 20., 0.))?;
                let close = self.sprite(&pill_image(20., 20.), 20., 20., Some(("×", 16.)))?;
                close.SetOffset(v3(23., 37., 1.))?;
                close.SetOpacity(0.)?;
                let badge =
                    self.sprite(&pill_image(88., 28.), 88., 28., Some(("✓ コピー済み", 11.)))?;
                badge.SetOffset(v3((w - 88.) / 2., h - 30., 2.))?;
                badge.SetOpacity(0.)?;
                visual.Children()?.InsertAtTop(&close)?;
                visual.Children()?.InsertAtTop(&badge)?;
                self.cache
                    .insert(item.id.clone(), (key, visual, close, badge, w, h));
            }
            let (_, visual, close, badge, w, h) = self.cache[&item.id].clone();
            close.SetOpacity(0.)?;
            badge.SetOpacity(0.)?;
            let x = width / 2. + (n as f32 - (count as f32 - 1.) / 2.) * spacing;
            let y = rope_y(x, width) - 9.5;
            let tilt = (item.id.bytes().fold(0u32, |a, b| a.wrapping_add(b as u32)) % 17) as f32
                / 10.
                - 0.8;
            visual.SetOffset(v3(x - w / 2., y - 20., 0.))?;
            visual.SetRotationAngleInDegrees(tilt)?;
            visual.SetScale(v3(1., 1., 1.))?;
            self.root.Children()?.InsertAtTop(&visual)?;
            if is_new {
                swing(&self.compositor, &visual, tilt, 12.)?;
            }
            self.state.cards.push(Card {
                factor,
                id: item.id.clone(),
                visual,
                close,
                badge,
                x: x - w / 2.,
                y: y - 20.,
                w,
                h,
                tilt,
            });
        }
        self.cache.retain(|id, _| items.iter().any(|i| &i.id == id));
        if count == 0 {
            let hint = self.sprite(
                &pill_image(350., 32.),
                350.,
                32.,
                Some(("スクリーンショットを撮ると、ここに吊るされます", 12.)),
            )?;
            hint.SetOffset(v3((width - 350.) / 2., rope_y(width / 2., width) + 22., 0.))?;
            self.root.Children()?.InsertAtTop(&hint)?;
        }
        self.signature = signature;
        self.set_region(false)?;
        Ok(())
    }
    fn set_region(&self, swept: bool) -> Result<()> {
        let scale = self.state.scale;
        let width = self.size.0 as f32 / scale;
        let mut regions = Vec::new();
        for x in (0..self.size.0).step_by(8) {
            let y = (rope_y(x as f32 / scale, width) * scale) as i32;
            regions.push(RECT {
                left: x,
                top: if swept { 0 } else { y - 3 },
                right: x + 9,
                bottom: y + 5,
            });
        }
        for c in &self.state.cards {
            regions.push(RECT {
                left: ((c.x - 8.) * scale) as i32,
                top: if swept {
                    0
                } else {
                    ((c.y - 14.) * scale) as i32
                },
                right: ((c.x + c.w + 8.) * scale) as i32,
                bottom: ((c.y + c.h + 8.) * scale) as i32,
            });
        }
        if self.state.cards.is_empty() {
            regions.push(RECT {
                left: ((width - 350.) / 2. * scale) as i32,
                top: ((rope_y(width / 2., width) + 22.) * scale) as i32,
                right: ((width + 350.) / 2. * scale) as i32,
                bottom: ((rope_y(width / 2., width) + 54.) * scale) as i32,
            });
        }
        self.source.SetSourceSize(Vector2 {
            X: self.size.0 as f32,
            Y: self.size.1 as f32,
        })?;
        self.regions.borrow_mut().update(
            self.hwnd,
            &self.compositor,
            &self.source,
            self.size,
            &regions,
            self.shown || self.hidden_at.is_some(),
        )
    }
    pub(crate) fn sprite(
        &self,
        pixels: &RgbaImage,
        w: f32,
        h: f32,
        label: Option<(&str, f32)>,
    ) -> Result<SpriteVisual> {
        unsafe {
            let surface = self.graphics.CreateDrawingSurface(
                Size {
                    Width: pixels.width() as f32,
                    Height: pixels.height() as f32,
                },
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                DirectXAlphaMode::Premultiplied,
            )?;
            let interop: ICompositionDrawingSurfaceInterop = surface.cast()?;
            let mut offset = POINT::default();
            let dc: ID2D1DeviceContext = interop.BeginDraw(None, &mut offset)?;
            let rect = D2D_RECT_F {
                left: offset.x as f32,
                top: offset.y as f32,
                right: offset.x as f32 + pixels.width() as f32,
                bottom: offset.y as f32 + pixels.height() as f32,
            };
            // Composition can pack surfaces into one atlas. Clip every update,
            // including Clear, to the rectangle returned by BeginDraw.
            dc.PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_ALIASED);
            let drawn = (|| -> Result<()> {
                dc.Clear(Some(&D2D1_COLOR_F {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 0.,
                }));
                let mut data = Vec::with_capacity(pixels.as_raw().len());
                for p in pixels.pixels() {
                    let a = p[3] as u16;
                    data.extend([
                        ((p[2] as u16 * a + 127) / 255) as u8,
                        ((p[1] as u16 * a + 127) / 255) as u8,
                        ((p[0] as u16 * a + 127) / 255) as u8,
                        p[3],
                    ]);
                }
                let bitmap = dc.CreateBitmap(
                    D2D_SIZE_U {
                        width: pixels.width(),
                        height: pixels.height(),
                    },
                    Some(data.as_ptr().cast()),
                    pixels.width() * 4,
                    &D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.,
                        dpiY: 96.,
                        ..Default::default()
                    },
                )?;
                dc.DrawBitmap(
                    &bitmap,
                    Some(&rect),
                    1.,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
                if let Some((label, font)) = label {
                    let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
                    let format = factory.CreateTextFormat(
                        w!("Segoe UI"),
                        None,
                        DWRITE_FONT_WEIGHT_MEDIUM,
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        font * 2.,
                        w!("ja-JP"),
                    )?;
                    format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                    format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                    let brush = dc.CreateSolidColorBrush(
                        &D2D1_COLOR_F {
                            r: 0.16,
                            g: 0.17,
                            b: 0.18,
                            a: 1.,
                        },
                        None,
                    )?;
                    dc.DrawText(
                        &label.encode_utf16().collect::<Vec<_>>(),
                        &format,
                        &rect,
                        &brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
                Ok(())
            })();
            dc.PopAxisAlignedClip();
            let ended = interop.EndDraw();
            drawn?;
            ended?;
            let brush = self.compositor.CreateSurfaceBrushWithSurface(&surface)?;
            let sprite = self.compositor.CreateSpriteVisual()?;
            sprite.SetSize(Vector2 { X: w, Y: h })?;
            sprite.SetBrush(&brush)?;
            Ok(sprite)
        }
    }
}
impl Drop for Clothesline {
    fn drop(&mut self) {
        self.regions.borrow_mut().clear();
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = KillTimer(Some(self.hwnd), 1);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
fn swing(c: &Compositor, v: &SpriteVisual, tilt: f32, amount: f32) -> Result<()> {
    let a = c.CreateScalarKeyFrameAnimation()?;
    for (t, k) in [
        (0., 1.),
        (0.22, -0.48),
        (0.43, 0.24),
        (0.65, -0.1),
        (1., 0.),
    ] {
        a.InsertKeyFrame(t, tilt + amount * k)?;
    }
    a.SetDuration(TimeSpan {
        Duration: 16_000_000,
    })?;
    v.StartAnimation(h!("RotationAngleInDegrees"), &a)
}
fn rope_y(x: f32, width: f32) -> f32 {
    let f = x / width;
    10. + 4. * (width * 0.018).min(30.) * f * (1. - f)
}
fn sdf(x: f32, y: f32, l: f32, t: f32, w: f32, h: f32, r: f32) -> f32 {
    let qx = (x - l - w / 2.).abs() - (w / 2. - r);
    let qy = (y - t - h / 2.).abs() - (h / 2. - r);
    qx.max(0.).hypot(qy.max(0.)) + qx.max(qy).min(0.) - r
}
fn over(dst: &mut [f32; 4], rgb: [f32; 3], alpha: f32) {
    let a = alpha.clamp(0., 1.);
    for n in 0..3 {
        dst[n] = rgb[n] * a + dst[n] * (1. - a);
    }
    dst[3] = a + dst[3] * (1. - a);
}
fn rgba(p: [f32; 4]) -> Rgba<u8> {
    if p[3] < 0.001 {
        Rgba([0, 0, 0, 0])
    } else {
        Rgba([
            (p[0] / p[3]).clamp(0., 255.) as u8,
            (p[1] / p[3]).clamp(0., 255.) as u8,
            (p[2] / p[3]).clamp(0., 255.) as u8,
            (p[3] * 255.).clamp(0., 255.) as u8,
        ])
    }
}
fn rope_image(width: f32) -> RgbaImage {
    RgbaImage::from_fn((width * 2.).ceil() as u32, 128, |x, y| {
        let x = x as f32 / 2.;
        let y = y as f32 / 2.;
        let line = rope_y(x, width);
        let fade = (x / width / 0.08)
            .min((1. - x / width) / 0.08)
            .clamp(0., 1.);
        let mut p = [0.; 4];
        over(
            &mut p,
            [0.; 3],
            (-((y - line - 1.2) / 1.4).powi(2)).exp() * 0.15 * fade,
        );
        over(
            &mut p,
            [140.; 3],
            (1. - (y - line).abs()).clamp(0., 1.) * 0.8 * fade,
        );
        over(
            &mut p,
            [255.; 3],
            (0.4 - (y - line + 0.5).abs()).clamp(0., 1.) * 0.6 * fade,
        );
        rgba(p)
    })
}
pub(crate) fn pill_image(w: f32, h: f32) -> RgbaImage {
    RgbaImage::from_fn((w * 2.) as u32, (h * 2.) as u32, |x, y| {
        let d = sdf(
            x as f32 / 2.,
            y as f32 / 2.,
            0.5,
            0.5,
            w - 1.,
            h - 1.,
            h / 2. - 1.,
        );
        let a = (0.5 - d).clamp(0., 1.);
        Rgba([238, 240, 243, (a * 0.94 * 255.) as u8])
    })
}
pub(crate) fn card_image(item: &Item, factor: f32, include_photo: bool) -> (RgbaImage, f32, f32) {
    let source = if item.video {
        None
    } else {
        crate::model::read_image(&item.path).ok()
    };
    let iw = source.as_ref().map(|i| i.width()).unwrap_or(160).max(1) as f32;
    let ih = source.as_ref().map(|i| i.height()).unwrap_or(100).max(1) as f32;
    let ratio = ((150. * factor - 14.) / iw).min(104. * factor / ih);
    let pw = (iw * ratio).round().max(1.);
    let ph = (ih * ratio).round().max(1.);
    let photo = source.map(|i| {
        i.resize_exact(
            (pw * 2.) as u32,
            (ph * 2.) as u32,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8()
    });
    let w = pw + 48.;
    let h = ph + 74.;
    let output = RgbaImage::from_fn((w * 2.) as u32, (h * 2.) as u32, |xx, yy| {
        let x = xx as f32 / 2.;
        let y = yy as f32 / 2.;
        let mut p = [0.; 4];
        let shadow = sdf(x, y, 20., 39., pw + 8., ph + 8., 16.);
        over(
            &mut p,
            [0.; 3],
            (-shadow.max(0.).powi(2) / 60.).exp() * 0.20,
        );
        let outer = sdf(x, y, 20., 34., pw + 8., ph + 8., 16.);
        let inside = (0.5 - outer).clamp(0., 1.);
        over(&mut p, [223., 226., 233.], inside * 0.78);
        over(
            &mut p,
            [255.; 3],
            (0.65 - outer.abs()).clamp(0., 1.)
                * (0.85 - 0.4 * ((y - 34.) / (ph + 8.)).clamp(0., 1.)),
        );
        let inner = sdf(x, y, 24., 38., pw, ph, 12.);
        let pa = (0.5 - inner).clamp(0., 1.);
        if !include_photo {
            for channel in &mut p {
                *channel *= 1. - pa;
            }
        }
        if pa > 0. && include_photo {
            if let Some(photo) = &photo {
                let px = ((x - 24.) * 2.).clamp(0., photo.width() as f32 - 1.) as u32;
                let py = ((y - 38.) * 2.).clamp(0., photo.height() as f32 - 1.) as u32;
                let q = photo.get_pixel(px, py);
                over(
                    &mut p,
                    [q[0] as f32, q[1] as f32, q[2] as f32],
                    pa * q[3] as f32 / 255.,
                );
            } else {
                over(&mut p, [72., 83., 99.], pa);
                let tx = x - w / 2.;
                let ty = y - (38. + ph / 2.);
                if (-4. ..8.).contains(&tx) && ty.abs() < (8. - tx) * 0.72 {
                    over(&mut p, [255.; 3], 0.94);
                }
            }
        }
        let pegshadow = sdf(x, y, w / 2. - 4.5, 21.5, 9., 26., 3.5);
        over(
            &mut p,
            [0.; 3],
            (-pegshadow.max(0.).powi(2) / 4.).exp() * 0.27,
        );
        let peg = sdf(x, y, w / 2. - 4.5, 20., 9., 26., 3.5);
        let metal = [178., 237., 209., 158.];
        let t = ((x - (w / 2. - 4.5)) / 9.).clamp(0., 0.999) * 3.;
        let n = t as usize;
        let v = metal[n] + (metal[n + 1] - metal[n]) * (t - n as f32);
        over(
            &mut p,
            if item.pinned {
                [v, v * 0.87, v * 0.58]
            } else {
                [v; 3]
            },
            (0.5 - peg).clamp(0., 1.),
        );
        over(&mut p, [255.; 3], (0.5 - peg.abs()).clamp(0., 1.) * 0.55);
        let slot = sdf(x, y, w / 2. - 2.5, 28.5, 5., 1.4, 0.7);
        over(&mut p, [0.; 3], (0.4 - slot).clamp(0., 1.) * 0.32);
        rgba(p)
    });
    (output, w, h)
}
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        // ReleaseCapture synchronously reenters the window procedure. Do not
        // borrow State again while the initiating mouse handler still uses it.
        if msg == WM_CAPTURECHANGED {
            return LRESULT(0);
        }
        if msg == WM_ERASEBKGND {
            return LRESULT(1);
        }
        if msg == WM_MOUSEACTIVATE {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        if !matches!(
            msg,
            WM_LBUTTONDOWN
                | WM_MOUSEMOVE
                | WM_LBUTTONUP
                | WM_RBUTTONUP
                | WM_MOUSEWHEEL
                | WM_KEYDOWN
                | WM_TIMER
                | WM_CLOSE
        ) {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        let state = &mut *ptr;
        let point = POINT {
            x: l.0 as i16 as i32,
            y: (l.0 >> 16) as i16 as i32,
        };
        let hit = |s: &State, p: POINT| {
            s.cards.iter().position(|c| {
                let x = p.x as f32 / s.scale;
                let y = p.y as f32 / s.scale;
                x >= c.x + 20. && x <= c.x + c.w - 20. && y >= c.y + 20. && y <= c.y + c.h - 26.
            })
        };
        match msg {
            WM_LBUTTONDOWN => {
                if let Some(n) = hit(state, point) {
                    SetCapture(hwnd);
                    state.pressed = Some((n, point, Instant::now()));
                    let _ = state.cards[n].visual.SetScale(v3(0.96, 0.96, 1.));
                }
                return LRESULT(0);
            }
            WM_MOUSEMOVE => {
                if let Some((n, start, _)) = state.pressed
                    && (point.x - start.x).abs() + (point.y - start.y).abs() > 12
                {
                    state.pressed = None;
                    let _ = ReleaseCapture();
                    let _ = state.cards[n].visual.SetScale(v3(1., 1., 1.));
                    send(Event::Drag(state.cards[n].id.clone()));
                }
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                let pressed = state.pressed.take();
                let _ = ReleaseCapture();
                if let Some((n, _, _)) = pressed {
                    let c = &state.cards[n];
                    let _ = c.visual.SetScale(v3(1., 1., 1.));
                    if hit(state, point) == Some(n) {
                        if point.x as f32 / state.scale < c.x + 46.
                            && point.y as f32 / state.scale < c.y + 59.
                        {
                            send(Event::Dismiss(c.id.clone()));
                        } else {
                            let double = state.last_click.as_ref().is_some_and(|(id, t)| {
                                id == &c.id && t.elapsed() < Duration::from_millis(450)
                            });
                            send(if double {
                                Event::Open(c.id.clone())
                            } else {
                                Event::Copy(c.id.clone())
                            });
                            state.last_click = Some((c.id.clone(), Instant::now()));
                        }
                    }
                }
                return LRESULT(0);
            }
            WM_RBUTTONUP => {
                send(
                    hit(state, point)
                        .map(|n| Event::Context(state.cards[n].id.clone()))
                        .unwrap_or(Event::Settings),
                );
                return LRESULT(0);
            }
            WM_MOUSEWHEEL => {
                send(Event::Page(if (w.0 >> 16) as i16 > 0 { -1 } else { 1 }));
                return LRESULT(0);
            }
            WM_KEYDOWN if w.0 == 27 => {
                send(Event::Hide);
                return LRESULT(0);
            }
            WM_TIMER => {
                let mut p = platform::cursor();
                let _ = ScreenToClient(hwnd, &mut p);
                let hover = hit(state, p);
                if hover != state.hovered {
                    for (n, c) in state.cards.iter().enumerate() {
                        let _ = c.close.SetOpacity(if hover == Some(n) { 1. } else { 0. });
                        if state.pressed.is_none() {
                            let s = if hover == Some(n) { 1.035 } else { 1. };
                            let _ = c.visual.SetScale(v3(s, s, 1.));
                        }
                    }
                    state.hovered = hover;
                }
                if let Some((n, _, t)) = state.pressed
                    && t.elapsed() > Duration::from_millis(650)
                {
                    state.pressed = None;
                    let _ = ReleaseCapture();
                    send(Event::Edit(state.cards[n].id.clone()));
                }
                if state
                    .copied
                    .as_ref()
                    .is_some_and(|(_, t)| t.elapsed() > Duration::from_millis(1400))
                    && let Some((n, _)) = state.copied.take()
                    && let Some(c) = state.cards.get(n)
                {
                    let _ = c.badge.SetOpacity(0.);
                }
                return LRESULT(0);
            }
            WM_CLOSE => return LRESULT(0),
            _ => (),
        }
        DefWindowProcW(hwnd, msg, w, l)
    }
}
