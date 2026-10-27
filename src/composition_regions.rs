//! Present one Composition scene through disjoint rectangular desktop targets.
//! The gaps have no visible/input HWND. No HRGN, GDI bitmap, or paint DC is used.
//! Input is forwarded to the transparent controller in its physical coordinates.
use std::collections::HashMap;
use windows::{
    UI::Composition::{Desktop::DesktopWindowTarget, *},
    Win32::{
        Foundation::*,
        Graphics::Gdi::{ClientToScreen, ScreenToClient},
        System::{LibraryLoader::GetModuleHandleW, WinRT::Composition::ICompositorDesktopInterop},
        UI::WindowsAndMessaging::*,
    },
    core::*,
};
use windows_numerics::{Vector2, Vector3};

#[derive(Default)]
pub struct RegionWindows {
    parts: Vec<Part>,
}
impl RegionWindows {
    pub fn update(
        &mut self,
        owner: HWND,
        compositor: &Compositor,
        source: &CompositionVisualSurface,
        size: (i32, i32),
        regions: &[RECT],
        visible: bool,
    ) -> Result<()> {
        let rects = disjoint_rects(regions, size);
        while self.parts.len() < rects.len() {
            self.parts.push(Part::new(owner, compositor, source)?);
        }
        self.parts.truncate(rects.len());
        let mut origin = RECT::default();
        unsafe {
            GetWindowRect(owner, &mut origin)?;
        }
        for (part, rect) in self.parts.iter().zip(rects) {
            part.visual.SetSize(Vector2 {
                X: size.0 as f32,
                Y: size.1 as f32,
            })?;
            part.visual.SetOffset(Vector3 {
                X: -rect.left as f32,
                Y: -rect.top as f32,
                Z: 0.,
            })?;
            unsafe {
                SetWindowPos(
                    part.hwnd,
                    Some(HWND_TOPMOST),
                    origin.left + rect.left,
                    origin.top + rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOACTIVATE,
                )?;
                let _ = ShowWindow(part.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
        Ok(())
    }
    pub fn hide(&self) {
        for part in &self.parts {
            unsafe {
                let _ = ShowWindow(part.hwnd, SW_HIDE);
            }
        }
    }
    pub fn clear(&mut self) {
        self.parts.clear();
    }
}

struct Part {
    hwnd: HWND,
    visual: SpriteVisual,
    _target: DesktopWindowTarget,
}
impl Part {
    fn new(
        owner: HWND,
        compositor: &Compositor,
        source: &CompositionVisualSurface,
    ) -> Result<Self> {
        unsafe {
            let module = GetModuleHandleW(None)?;
            let class = w!("SnaplineCompositionRegion");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(input_proc),
                hInstance: module.into(),
                lpszClassName: class,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                ..Default::default()
            });
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                class,
                w!("Snapline — Shelf region"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                Some(owner),
                None,
                Some(module.into()),
                None,
            )?;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, owner.0 as isize);
            let created = (|| {
                let interop: ICompositorDesktopInterop = compositor.cast()?;
                let target = interop.CreateDesktopWindowTarget(hwnd, true)?;
                let visual = compositor.CreateSpriteVisual()?;
                let brush = compositor.CreateSurfaceBrushWithSurface(source)?;
                brush.SetStretch(CompositionStretch::None)?;
                brush.SetHorizontalAlignmentRatio(0.)?;
                brush.SetVerticalAlignmentRatio(0.)?;
                visual.SetBrush(&brush)?;
                target.SetRoot(&visual)?;
                Ok(Self {
                    hwnd,
                    visual,
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
impl Drop for Part {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn input_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_MOUSEACTIVATE {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        if msg == WM_ERASEBKGND {
            return LRESULT(1);
        }
        let owner = HWND(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut _);
        if !owner.is_invalid() {
            match msg {
                WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP | WM_MOUSEMOVE => {
                    let mut point = POINT {
                        x: l.0 as i16 as i32,
                        y: (l.0 >> 16) as i16 as i32,
                    };
                    let _ = ClientToScreen(hwnd, &mut point);
                    let _ = ScreenToClient(owner, &mut point);
                    let mapped = LPARAM(
                        ((point.x as u16 as u32) | ((point.y as u16 as u32) << 16)) as isize,
                    );
                    return SendMessageW(owner, msg, Some(w), Some(mapped));
                }
                WM_MOUSEWHEEL | WM_KEYDOWN | WM_CLOSE => {
                    return SendMessageW(owner, msg, Some(w), Some(l));
                }
                _ => (),
            }
        }
        // DefWindowProc validates WM_PAINT. All pixels come from Composition.
        DefWindowProcW(hwnd, msg, w, l)
    }
}

/// Union the coverage and split it without overlaps. Overlapping translucent
/// targets would otherwise paint the scene twice and darken those pixels.
fn disjoint_rects(rects: &[RECT], size: (i32, i32)) -> Vec<RECT> {
    let rects: Vec<_> = rects
        .iter()
        .filter_map(|r| {
            let rect = RECT {
                left: r.left.max(0),
                top: r.top.max(0),
                right: r.right.min(size.0),
                bottom: r.bottom.min(size.1),
            };
            (rect.left < rect.right && rect.top < rect.bottom).then_some(rect)
        })
        .collect();
    let mut xs: Vec<_> = rects.iter().flat_map(|r| [r.left, r.right]).collect();
    xs.sort_unstable();
    xs.dedup();
    let mut output: Vec<RECT> = Vec::new();
    let mut previous = HashMap::new();
    for pair in xs.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        let mut ys: Vec<_> = rects
            .iter()
            .filter(|r| r.left <= left && r.right >= right)
            .map(|r| (r.top, r.bottom))
            .collect();
        ys.sort_unstable();
        let mut merged: Vec<(i32, i32)> = Vec::new();
        for (top, bottom) in ys {
            if let Some(last) = merged.last_mut()
                && top <= last.1
            {
                last.1 = last.1.max(bottom);
            } else {
                merged.push((top, bottom));
            }
        }
        let mut current = HashMap::new();
        for (top, bottom) in merged {
            let key = (top, bottom);
            let index = if let Some(&index) = previous.get(&key) {
                let rect: &mut RECT = &mut output[index];
                rect.right = right;
                index
            } else {
                output.push(RECT {
                    left,
                    top,
                    right,
                    bottom,
                });
                output.len() - 1
            };
            current.insert(key, index);
        }
        previous = current;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contains(r: &RECT, x: i32, y: i32) -> bool {
        x >= r.left && x < r.right && y >= r.top && y < r.bottom
    }
    #[test]
    fn coverage_preserves_gaps_clips_edges_and_never_double_paints() {
        let rects = [
            RECT {
                left: -4,
                top: -2,
                right: 9,
                bottom: 3,
            },
            RECT {
                left: 4,
                top: 1,
                right: 12,
                bottom: 8,
            },
            RECT {
                left: 16,
                top: 4,
                right: 25,
                bottom: 15,
            },
            RECT {
                left: 3,
                top: 5,
                right: 2,
                bottom: 6,
            },
        ];
        let output = disjoint_rects(&rects, (20, 12));
        for x in -5..26 {
            for y in -3..16 {
                let expected = (0..20).contains(&x)
                    && (0..12).contains(&y)
                    && rects.iter().any(|r| contains(r, x, y));
                assert_eq!(
                    output.iter().filter(|r| contains(r, x, y)).count(),
                    usize::from(expected),
                    "{x}, {y}"
                );
            }
        }
    }
    #[test]
    fn adjacent_rope_strips_merge_and_empty_layout_has_no_windows() {
        let strips: Vec<_> = (0..100)
            .map(|x| RECT {
                left: x,
                top: 4,
                right: x + 2,
                bottom: 9,
            })
            .collect();
        assert_eq!(
            disjoint_rects(&strips, (100, 20)),
            vec![RECT {
                left: 0,
                top: 4,
                right: 100,
                bottom: 9
            }]
        );
        assert!(disjoint_rects(&[], (100, 20)).is_empty());
    }
}
