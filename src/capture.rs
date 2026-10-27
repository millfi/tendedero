//! Frozen-desktop rectangle selection and the Composition flight to the line.
//! All geometry is physical pixels in virtual-desktop coordinates, including
//! negative monitor origins. There is no clipboard round trip or screen hook.
use crate::{
    clothesline::{self, Clothesline, Landing},
    model::{AppResult, Item},
    platform::{self, NativeEvent},
};
use image::DynamicImage;
use std::time::{Duration, Instant};
use windows::{
    Foundation::TimeSpan,
    UI::{
        Color,
        Composition::{Desktop::DesktopWindowTarget, *},
    },
    Win32::{
        Foundation::*,
        System::{LibraryLoader::GetModuleHandleW, WinRT::Composition::ICompositorDesktopInterop},
        UI::{
            Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
            WindowsAndMessaging::*,
        },
    },
    core::*,
};
use windows_numerics::{Vector2, Vector3};
fn vec3(x: f32, y: f32, z: f32) -> Vector3 {
    Vector3 { X: x, Y: y, Z: z }
}

struct OverlayWindow {
    hwnd: HWND,
    root: ContainerVisual,
    _target: DesktopWindowTarget,
}
impl OverlayWindow {
    fn new(compositor: &Compositor, bounds: RECT, flight: bool) -> Result<Self> {
        unsafe {
            let module = GetModuleHandleW(None)?;
            let class = if flight {
                w!("SnaplineFlight")
            } else {
                w!("SnaplineRectangleCapture")
            };
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(if flight { flight_proc } else { capture_proc }),
                hInstance: module.into(),
                lpszClassName: class,
                hCursor: LoadCursorW(None, if flight { IDC_ARROW } else { IDC_CROSS })?,
                ..Default::default()
            });
            let test = std::env::args().any(|a| a == "--ui-test");
            let styles = WS_EX_NOREDIRECTIONBITMAP
                | WS_EX_TOPMOST
                | if test {
                    WS_EX_APPWINDOW
                } else {
                    WS_EX_TOOLWINDOW
                }
                | if flight {
                    WS_EX_TRANSPARENT
                        | if test {
                            WINDOW_EX_STYLE(0)
                        } else {
                            WS_EX_NOACTIVATE
                        }
                } else {
                    WINDOW_EX_STYLE(0)
                };
            let hwnd = CreateWindowExW(
                styles,
                class,
                if flight {
                    w!("Snapline — Capture flight")
                } else {
                    w!("Snapline — Select a rectangle")
                },
                WS_POPUP,
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                None,
                None,
                Some(module.into()),
                None,
            )?;
            // Keep a guard alive while the Composition target is being created.
            let created = (|| -> Result<Self> {
                let interop: ICompositorDesktopInterop = compositor.cast()?;
                let target = interop.CreateDesktopWindowTarget(hwnd, true)?;
                let root = compositor.CreateContainerVisual()?;
                target.SetRoot(&root)?;
                Ok(Self {
                    hwnd,
                    root,
                    _target: target,
                })
            })();
            if created.is_err() {
                let _ = DestroyWindow(hwnd);
            }
            created
        }
    }
}
impl Drop for OverlayWindow {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

pub struct Selection {
    window: OverlayWindow,
    _state: Box<SelectionState>,
    image: DynamicImage,
    bounds: RECT,
    previous: HWND,
}
struct SelectionState {
    bounds: RECT,
    start: Option<POINT>,
    shades: Vec<SpriteVisual>,
    edges: Vec<SpriteVisual>,
    done: bool,
}
impl Selection {
    pub fn start(line: &Clothesline) -> AppResult<Self> {
        let bounds = platform::desktop_rect();
        let image = platform::capture_rect(bounds)?;
        let previous = unsafe { GetForegroundWindow() };
        let window = OverlayWindow::new(line.compositor(), bounds, false)?;
        let w = (bounds.right - bounds.left) as f32;
        let h = (bounds.bottom - bounds.top) as f32;
        let background = line.sprite(
            image.as_rgba8().ok_or("画面の画像形式を読み込めません")?,
            w,
            h,
            None,
        )?;
        window.root.Children()?.InsertAtTop(&background)?;
        let mut shades = vec![];
        let mut edges = vec![];
        for _ in 0..4 {
            let s = solid(
                line.compositor(),
                Color {
                    A: 112,
                    R: 0,
                    G: 0,
                    B: 0,
                },
            )?;
            window.root.Children()?.InsertAtTop(&s)?;
            shades.push(s);
        }
        for _ in 0..4 {
            let s = solid(
                line.compositor(),
                Color {
                    A: 255,
                    R: 255,
                    G: 255,
                    B: 255,
                },
            )?;
            window.root.Children()?.InsertAtTop(&s)?;
            edges.push(s);
        }
        let monitor = platform::monitor_rect_at(platform::cursor(), true);
        let ui_scale =
            unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(window.hwnd) }.max(96) as f32 / 96.;
        let label = line.sprite(
            &clothesline::pill_image(460., 36.),
            460. * ui_scale,
            36. * ui_scale,
            Some((
                "ドラッグして撮影範囲を選択  ·  Esc / 右クリックでキャンセル",
                12.,
            )),
        )?;
        label.SetOffset(vec3(
            (monitor.left + monitor.right) as f32 / 2. - bounds.left as f32 - 230. * ui_scale,
            (monitor.top - bounds.top) as f32 + 24. * ui_scale,
            0.,
        ))?;
        window.root.Children()?.InsertAtTop(&label)?;
        let mut state = Box::new(SelectionState {
            bounds,
            start: None,
            shades,
            edges,
            done: false,
        });
        state.draw(None)?;
        unsafe {
            SetWindowLongPtrW(
                window.hwnd,
                GWLP_USERDATA,
                (&mut *state as *mut SelectionState) as isize,
            );
            let _ = ShowWindow(window.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(window.hwnd);
        }
        Ok(Self {
            window,
            _state: state,
            image,
            bounds,
            previous,
        })
    }
    pub fn crop(&self, rect: RECT) -> AppResult<DynamicImage> {
        let local = local_crop(rect, self.bounds).ok_or("選択範囲が画面外です")?;
        Ok(self.image.crop_imm(local.0, local.1, local.2, local.3))
    }
}
impl Drop for Selection {
    fn drop(&mut self) {
        unsafe {
            if GetForegroundWindow() == self.window.hwnd {
                let _ = ShowWindow(self.window.hwnd, SW_HIDE);
                if IsWindow(Some(self.previous)).as_bool() {
                    let _ = SetForegroundWindow(self.previous);
                }
            }
        }
    }
}
fn solid(c: &Compositor, color: Color) -> Result<SpriteVisual> {
    let s = c.CreateSpriteVisual()?;
    s.SetBrush(&c.CreateColorBrushWithColor(color)?)?;
    Ok(s)
}
fn box_at(s: &SpriteVisual, x: f32, y: f32, w: f32, h: f32) -> Result<()> {
    s.SetOffset(vec3(x, y, 0.))?;
    s.SetSize(Vector2 {
        X: w.max(0.),
        Y: h.max(0.),
    })
}
impl SelectionState {
    fn draw(&self, selection: Option<RECT>) -> Result<()> {
        let w = (self.bounds.right - self.bounds.left) as f32;
        let h = (self.bounds.bottom - self.bounds.top) as f32;
        let r = selection.unwrap_or_default();
        let (x, y, right, bottom) = (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        box_at(&self.shades[0], 0., 0., w, y)?;
        box_at(&self.shades[1], 0., bottom, w, h - bottom)?;
        box_at(&self.shades[2], 0., y, x, bottom - y)?;
        box_at(&self.shades[3], right, y, w - right, bottom - y)?;
        for edge in &self.edges {
            edge.SetOpacity(if selection.is_some() { 1. } else { 0. })?;
        }
        box_at(&self.edges[0], x, y, right - x, 1.)?;
        box_at(&self.edges[1], x, (bottom - 1.).max(y), right - x, 1.)?;
        box_at(&self.edges[2], x, y, 1., bottom - y)?;
        box_at(&self.edges[3], (right - 1.).max(x), y, 1., bottom - y)?;
        Ok(())
    }
    fn finish(&mut self, event: NativeEvent) {
        if !self.done {
            self.done = true;
            unsafe {
                let _ = ReleaseCapture();
            }
            platform::emit(event);
        }
    }
}
pub fn selection_rect(start: POINT, end: POINT, width: i32, height: i32) -> Option<RECT> {
    let r = RECT {
        left: start.x.min(end.x).clamp(0, width),
        top: start.y.min(end.y).clamp(0, height),
        right: start.x.max(end.x).clamp(0, width),
        bottom: start.y.max(end.y).clamp(0, height),
    };
    (r.right - r.left >= 2 && r.bottom - r.top >= 2).then_some(r)
}
fn local_crop(rect: RECT, bounds: RECT) -> Option<(u32, u32, u32, u32)> {
    if rect.left < bounds.left
        || rect.top < bounds.top
        || rect.right > bounds.right
        || rect.bottom > bounds.bottom
        || rect.right <= rect.left
        || rect.bottom <= rect.top
    {
        return None;
    }
    Some((
        (rect.left - bounds.left) as u32,
        (rect.top - bounds.top) as u32,
        (rect.right - rect.left) as u32,
        (rect.bottom - rect.top) as u32,
    ))
}
unsafe extern "system" fn capture_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_CAPTURECHANGED {
            return LRESULT(0);
        }
        if msg == WM_ERASEBKGND {
            return LRESULT(1);
        }
        if !matches!(
            msg,
            WM_LBUTTONDOWN
                | WM_MOUSEMOVE
                | WM_LBUTTONUP
                | WM_RBUTTONUP
                | WM_KEYDOWN
                | WM_CLOSE
                | WM_ACTIVATE
                | WM_DISPLAYCHANGE
        ) {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SelectionState;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        let state = &mut *ptr;
        if state.done {
            return LRESULT(0);
        }
        let p = POINT {
            x: l.0 as i16 as i32,
            y: (l.0 >> 16) as i16 as i32,
        };
        match msg {
            WM_LBUTTONDOWN => {
                state.start = Some(p);
                SetCapture(hwnd);
            }
            WM_MOUSEMOVE => {
                if let Some(start) = state.start {
                    let _ = state.draw(selection_rect(
                        start,
                        p,
                        state.bounds.right - state.bounds.left,
                        state.bounds.bottom - state.bounds.top,
                    ));
                }
            }
            WM_LBUTTONUP => {
                if let Some(start) = state.start.take() {
                    if let Some(r) = selection_rect(
                        start,
                        p,
                        state.bounds.right - state.bounds.left,
                        state.bounds.bottom - state.bounds.top,
                    ) {
                        state.finish(NativeEvent::Selected(RECT {
                            left: r.left + state.bounds.left,
                            top: r.top + state.bounds.top,
                            right: r.right + state.bounds.left,
                            bottom: r.bottom + state.bounds.top,
                        }));
                    } else {
                        let _ = ReleaseCapture();
                        let _ = state.draw(None);
                    }
                }
            }
            WM_RBUTTONUP | WM_CLOSE | WM_DISPLAYCHANGE => {
                state.finish(NativeEvent::CaptureCancelled)
            }
            WM_KEYDOWN if w.0 == 27 => state.finish(NativeEvent::CaptureCancelled),
            WM_ACTIVATE if w.0 & 0xffff == WA_INACTIVE as usize => {
                state.finish(NativeEvent::CaptureCancelled)
            }
            _ => (),
        }
        LRESULT(0)
    }
}

pub struct Flight {
    window: OverlayWindow,
    landing: Landing,
    ends: Instant,
}
impl Flight {
    pub fn start(
        line: &Clothesline,
        item: &Item,
        image: &DynamicImage,
        from: RECT,
    ) -> AppResult<Self> {
        let landing = line
            .landing(&item.id)
            .ok_or("写真の着地点を取得できません")?;
        let bounds = platform::desktop_rect();
        let window = OverlayWindow::new(line.compositor(), bounds, true)?;
        let c = line.compositor();
        let group = c.CreateContainerVisual()?;
        window.root.Children()?.InsertAtTop(&group)?;
        let pw = landing.width - 48.;
        let ph = landing.height - 74.;
        let photo = line.sprite(&image.to_rgba8(), pw, ph, None)?;
        photo.SetOffset(vec3(-pw / 2., 18., 0.))?;
        // The rounded clip grows independently of the photograph's shrinking scale.
        let geometry = c.CreateRoundedRectangleGeometry()?;
        geometry.SetSize(Vector2 { X: pw, Y: ph })?;
        let clip = c.CreateGeometricClipWithGeometry(&geometry)?;
        photo.SetClip(&clip)?;
        group.Children()?.InsertAtTop(&photo)?;
        let (chrome, _, _) = clothesline::card_image(item, landing.factor, false);
        let frame = line.sprite(&chrome, landing.width, landing.height, None)?;
        frame.SetOffset(vec3(-landing.width / 2., -20., 0.))?;
        group.Children()?.InsertAtTop(&frame)?;
        let offset = c.CreateVector3KeyFrameAnimation()?;
        let scale = c.CreateVector3KeyFrameAnimation()?;
        let angle = c.CreateScalarKeyFrameAnimation()?;
        let opacity = c.CreateScalarKeyFrameAnimation()?;
        let radius = c.CreateVector2KeyFrameAnimation()?;
        let linear = c.CreateLinearEasingFunction()?;
        let start_scale = vec3(
            (from.right - from.left) as f32 / pw,
            (from.bottom - from.top) as f32 / ph,
            1.,
        );
        let start = vec3(
            (from.left + from.right) as f32 / 2. - bounds.left as f32,
            from.top as f32 - bounds.top as f32 - 18. * start_scale.Y,
            0.,
        );
        let end = vec3(
            landing.anchor.X - bounds.left as f32,
            landing.anchor.Y - bounds.top as f32,
            0.,
        );
        // Match macOS: 650 ms, cubic ease-in/out and a 30 pt upward arc.
        for n in 0..=60 {
            let t = n as f32 / 60.;
            let k = ease(t);
            let chrome = smooth((k - 0.35) / 0.65);
            offset.InsertKeyFrameWithEasingFunction(
                t,
                vec3(
                    start.X + (end.X - start.X) * k,
                    start.Y + (end.Y - start.Y) * k
                        - (std::f32::consts::PI * k).sin() * 30. * landing.scale,
                    0.,
                ),
                &linear,
            )?;
            scale.InsertKeyFrameWithEasingFunction(
                t,
                vec3(
                    start_scale.X + (landing.scale - start_scale.X) * k,
                    start_scale.Y + (landing.scale - start_scale.Y) * k,
                    1.,
                ),
                &linear,
            )?;
            angle.InsertKeyFrameWithEasingFunction(t, landing.tilt * k, &linear)?;
            opacity.InsertKeyFrameWithEasingFunction(t, chrome, &linear)?;
            radius.InsertKeyFrameWithEasingFunction(
                t,
                Vector2 {
                    X: 12. * k,
                    Y: 12. * k,
                },
                &linear,
            )?;
        }
        // Test-only slow motion makes intermediate frames inspectable by Computer Use.
        let slow = std::env::args().any(|a| a == "--ui-test")
            && std::env::args().any(|a| a == "--slow-flight");
        let timing = if slow { 10 } else { 1 };
        let duration = TimeSpan {
            Duration: 6_500_000 * timing,
        };
        offset.SetDuration(duration)?;
        scale.SetDuration(duration)?;
        angle.SetDuration(duration)?;
        opacity.SetDuration(duration)?;
        radius.SetDuration(duration)?;
        group.SetOffset(start)?;
        group.SetScale(start_scale)?;
        frame.SetOpacity(0.)?;
        group.StartAnimation(h!("Offset"), &offset)?;
        group.StartAnimation(h!("Scale"), &scale)?;
        group.StartAnimation(h!("RotationAngleInDegrees"), &angle)?;
        frame.StartAnimation(h!("Opacity"), &opacity)?;
        geometry.StartAnimation(h!("CornerRadius"), &radius)?;
        let fade_in = c.CreateScalarKeyFrameAnimation()?;
        fade_in.InsertKeyFrame(0., 0.)?;
        fade_in.InsertKeyFrame(1., 1.)?;
        fade_in.SetDuration(TimeSpan {
            Duration: 1_200_000 * timing,
        })?;
        fade_in.SetDelayTime(duration)?;
        let fade_out = c.CreateScalarKeyFrameAnimation()?;
        fade_out.InsertKeyFrame(0., 1.)?;
        fade_out.InsertKeyFrame(1., 0.)?;
        fade_out.SetDuration(TimeSpan {
            Duration: 1_200_000 * timing,
        })?;
        fade_out.SetDelayTime(duration)?;
        window.root.StartAnimation(h!("Opacity"), &fade_out)?;
        landing.visual.SetOpacity(0.)?;
        if let Err(error) = landing.visual.StartAnimation(h!("Opacity"), &fade_in) {
            let _ = landing.visual.SetOpacity(1.);
            return Err(error.into());
        }
        unsafe {
            let _ = ShowWindow(window.hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(Self {
            window,
            landing,
            ends: Instant::now() + Duration::from_millis(770 * timing as u64),
        })
    }
    pub fn complete(&self) -> bool {
        Instant::now() >= self.ends
    }
}
impl Drop for Flight {
    fn drop(&mut self) {
        let _ = self.landing.visual.StopAnimation(h!("Opacity"));
        let _ = self.landing.visual.SetOpacity(1.);
        unsafe {
            let _ = ShowWindow(self.window.hwnd, SW_HIDE);
        }
    }
}
fn ease(t: f32) -> f32 {
    if t < 0.5 {
        4. * t * t * t
    } else {
        1. - (-2. * t + 2.).powi(3) / 2.
    }
}
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}
unsafe extern "system" fn flight_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_ERASEBKGND => LRESULT(1),
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reverse_drag_is_clamped_to_the_frozen_desktop() {
        let r =
            selection_rect(POINT { x: 700, y: 500 }, POINT { x: -20, y: 40 }, 640, 480).unwrap();
        assert_eq!((r.left, r.top, r.right, r.bottom), (0, 40, 640, 480));
        assert!(selection_rect(POINT { x: 10, y: 10 }, POINT { x: 11, y: 40 }, 640, 480).is_none());
    }
    #[test]
    fn crop_handles_negative_monitor_origins_and_cross_monitor_selection() {
        let desktop = RECT {
            left: -1920,
            top: -300,
            right: 2560,
            bottom: 1440,
        };
        assert_eq!(
            local_crop(
                RECT {
                    left: -100,
                    top: -50,
                    right: 200,
                    bottom: 100
                },
                desktop
            ),
            Some((1820, 250, 300, 150))
        );
        assert!(
            local_crop(
                RECT {
                    left: -2000,
                    top: 0,
                    right: 10,
                    bottom: 10
                },
                desktop
            )
            .is_none()
        );
    }
    #[test]
    fn flight_easing_lands_exactly_and_reveals_chrome_late() {
        assert_eq!(ease(0.), 0.);
        assert_eq!(ease(1.), 1.);
        assert_eq!(ease(0.5), 0.5);
        assert_eq!(smooth(-1.), 0.);
        assert_eq!(smooth(2.), 1.);
        let mut previous = 0.;
        for n in 0..=100 {
            let current = ease(n as f32 / 100.);
            assert!(current >= previous);
            previous = current;
        }
    }
}
