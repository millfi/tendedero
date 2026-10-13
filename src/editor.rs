use crate::{
    model::{AppResult, Item},
    platform,
};
use std::path::PathBuf;
use windows_reactor::*;

#[derive(Clone, PartialEq)]
pub struct EditorInput {
    pub item: Item,
    pub saved: Callback<PathBuf>,
}
#[derive(Clone)]
pub enum EditMessage {
    Down(PointerEventInfo),
    Move(PointerEventInfo),
    Up,
    Undo,
    Clear,
    Save,
    Crop,
    Pen,
    Rectangle,
    External,
    Close,
}
#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Pen,
    Rectangle,
    Crop,
}
#[derive(Clone)]
struct Stroke {
    points: Vec<(f64, f64)>,
    rectangle: bool,
}
pub struct Editor {
    input: EditorInput,
    original: image::RgbaImage,
    strokes: Vec<Stroke>,
    current: Option<Stroke>,
    tool: Tool,
    crop: Option<(f64, f64, f64, f64)>,
    status: String,
    width: f64,
    height: f64,
}
impl Component for Editor {
    type Input = EditorInput;
    type Message = EditMessage;
    fn create(input: &EditorInput, _: &ComponentContext<Self>) -> Self {
        let original = crate::model::read_image(&input.item.path)
            .map(|i| i.to_rgba8())
            .unwrap_or_else(|_| image::RgbaImage::new(1, 1));
        let scale = (900.0 / original.width() as f64)
            .min(510.0 / original.height() as f64)
            .min(1.0);
        Self {
            input: input.clone(),
            width: original.width() as f64 * scale,
            height: original.height() as f64 * scale,
            original,
            strokes: vec![],
            current: None,
            tool: Tool::Pen,
            crop: None,
            status: "ドラッグして描画。切り抜きは範囲をドラッグして指定します。".into(),
        }
    }
    fn update(&mut self, m: EditMessage, cx: &ComponentContext<Self>) {
        match m {
            EditMessage::Down(p) if p.is_left_button_pressed => {
                self.current = Some(Stroke {
                    points: vec![(p.x.clamp(0., self.width), p.y.clamp(0., self.height))],
                    rectangle: self.tool != Tool::Pen,
                });
            }
            EditMessage::Move(p) if p.is_left_button_pressed => {
                if let Some(s) = self.current.as_mut() {
                    s.points
                        .push((p.x.clamp(0., self.width), p.y.clamp(0., self.height)));
                }
            }
            EditMessage::Up => {
                if let Some(s) = self.current.take() {
                    if self.tool == Tool::Crop {
                        if let (Some(a), Some(b)) = (s.points.first(), s.points.last()) {
                            self.crop = Some((
                                a.0.min(b.0),
                                a.1.min(b.1),
                                (a.0 - b.0).abs(),
                                (a.1 - b.1).abs(),
                            ));
                        }
                    } else {
                        self.strokes.push(s);
                    }
                }
            }
            EditMessage::Undo => {
                self.current = None;
                if self.crop.take().is_none() {
                    self.strokes.pop();
                }
            }
            EditMessage::Clear => {
                self.strokes.clear();
                self.crop = None;
                self.current = None;
            }
            EditMessage::Pen => self.tool = Tool::Pen,
            EditMessage::Rectangle => self.tool = Tool::Rectangle,
            EditMessage::Crop => self.tool = Tool::Crop,
            EditMessage::Save => match self.save() {
                Ok(path) => {
                    self.status = if self.input.saved.call(path) {
                        let _ = cx.window().request_close();
                        "編集した画像を新しい1枚として棚に保存しました".into()
                    } else {
                        "画像を保存しましたが、棚は閉じられています。保存場所から取り込めます。"
                            .into()
                    };
                }
                Err(e) => self.status = e.to_string(),
            },
            EditMessage::External => {
                self.status = match platform::edit(&self.input.item.path) {
                    Ok(()) => "ペイントで編集できます。保存後に棚の画像も更新されます。".into(),
                    Err(e) => e.to_string(),
                };
            }
            EditMessage::Close => {
                let _ = cx.window().request_close();
            }
            _ => (),
        }
    }
    fn view(&self, _: &EditorInput, cx: &mut ViewContext<Self>) -> View {
        cx.window_title(format!("Snapline · {}", self.input.item.name));
        cx.window_visuals(
            WindowVisuals::new()
                .client_size(980., 700.)
                .icon(platform::icon_path())
                .backdrop(WindowBackdrop::Mica),
        );
        let original = Image::new()
            .source_file(&self.input.item.path)
            .unwrap_or_default()
            .stretch(Stretch::Uniform)
            .width(self.width)
            .height(self.height);
        let mut lines = Vec::new();
        for (i, s) in self.strokes.iter().chain(self.current.iter()).enumerate() {
            let color = if self.tool == Tool::Crop && i == self.strokes.len() {
                Color::rgb(77, 180, 245)
            } else {
                Color::rgb(244, 91, 105)
            };
            append_stroke(&mut lines, i, s, color);
        }
        if let Some((x, y, w, h)) = self.crop {
            lines.push(KeyedView::new(
                "crop",
                windows_reactor::Rectangle::new()
                    .width(w)
                    .height(h)
                    .stroke(Color::rgb(77, 180, 245))
                    .stroke_thickness(2.)
                    .canvas_left(x)
                    .canvas_top(y),
            ));
        }
        StackPanel::new().spacing(18.).margin(24.).children((
            TextBlock::new()
                .text("マークアップ")
                .font_size(24.)
                .font_weight(FontWeight::SEMI_BOLD),
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(8.)
                .children((
                    Button::new()
                        .on_click(cx.message(EditMessage::Pen))
                        .content(if self.tool == Tool::Pen {
                            "● ペン"
                        } else {
                            "ペン"
                        }),
                    Button::new()
                        .on_click(cx.message(EditMessage::Rectangle))
                        .content(if self.tool == Tool::Rectangle {
                            "● 四角"
                        } else {
                            "四角"
                        }),
                    Button::new()
                        .on_click(cx.message(EditMessage::Crop))
                        .content(if self.tool == Tool::Crop {
                            "● 切り抜き"
                        } else {
                            "切り抜き"
                        }),
                    Button::new()
                        .on_click(cx.message(EditMessage::Undo))
                        .content("元に戻す"),
                    Button::new()
                        .on_click(cx.message(EditMessage::Clear))
                        .content("リセット"),
                    Button::new()
                        .on_click(cx.message(EditMessage::External))
                        .content("ペイントで開く"),
                    Button::new()
                        .on_click(cx.message(EditMessage::Save))
                        .content("コピーを保存"),
                    Button::new()
                        .on_click(cx.message(EditMessage::Close))
                        .content("閉じる"),
                )),
            Border::new()
                .background(Color::argb(35, 0, 0, 0))
                .corner_radius(8.)
                .padding(8.)
                .horizontal_alignment(HorizontalAlignment::Left)
                .content(
                    Border::new()
                        .width(self.width)
                        .height(self.height)
                        .background(Color::transparent())
                        .capture_pointer_on_press(true)
                        .on_pointer_pressed(cx.callback(EditMessage::Down))
                        .on_pointer_moved(cx.callback(EditMessage::Move))
                        .on_pointer_released(cx.callback(|_| EditMessage::Up))
                        .on_pointer_capture_lost(cx.message(EditMessage::Up))
                        .content(
                            Grid::new().children((original, Canvas::new().keyed_children(lines))),
                        ),
                ),
            TextBlock::new().text(&self.status).font_size(12.),
        ))
    }
}
fn append_stroke(lines: &mut Vec<KeyedView>, index: usize, s: &Stroke, color: Color) {
    if s.rectangle
        && let (Some(a), Some(b)) = (s.points.first(), s.points.last())
    {
        lines.push(KeyedView::new(
            format!("rect-{index}"),
            windows_reactor::Rectangle::new()
                .width((b.0 - a.0).abs())
                .height((b.1 - a.1).abs())
                .stroke(color)
                .stroke_thickness(3.)
                .canvas_left(a.0.min(b.0))
                .canvas_top(a.1.min(b.1)),
        ));
        return;
    }
    lines.extend(s.points.windows(2).enumerate().map(|(i, p)| {
        KeyedView::new(
            format!("line-{index}-{i}"),
            Line::new()
                .x1(p[0].0)
                .y1(p[0].1)
                .x2(p[1].0)
                .y2(p[1].1)
                .stroke(color)
                .stroke_thickness(3.),
        )
    }));
}
impl Editor {
    fn save(&self) -> AppResult<PathBuf> {
        let mut img = self.original.clone();
        let scale = img.width() as f64 / self.width;
        for s in &self.strokes {
            if s.rectangle {
                if let (Some(a), Some(b)) = (s.points.first(), s.points.last()) {
                    for (a, b) in [
                        (*a, (b.0, a.1)),
                        ((b.0, a.1), *b),
                        (*b, (a.0, b.1)),
                        ((a.0, b.1), *a),
                    ] {
                        draw_line(&mut img, a, b, scale);
                    }
                }
            } else {
                for p in s.points.windows(2) {
                    draw_line(&mut img, p[0], p[1], scale);
                }
            }
        }
        if let Some((x, y, w, h)) = self.crop {
            if w < 2. || h < 2. {
                return Err("切り抜く範囲を広げてください".into());
            }
            let x = (x * scale).round() as u32;
            let y = (y * scale).round() as u32;
            let w = ((w * scale).round() as u32).min(img.width().saturating_sub(x));
            let h = ((h * scale).round() as u32).min(img.height().saturating_sub(y));
            if w == 0 || h == 0 {
                return Err("切り抜き範囲が画像の外です".into());
            }
            img = image::imageops::crop_imm(&img, x, y, w, h).to_image();
        }
        let path = self
            .input
            .item
            .path
            .parent()
            .ok_or("画像の保存場所がありません")?
            .join(format!("{}-edited.png", crate::model::unique_id()));
        img.save(&path)?;
        Ok(path)
    }
}
fn draw_line(img: &mut image::RgbaImage, a: (f64, f64), b: (f64, f64), scale: f64) {
    let (x0, y0, x1, y1) = (a.0 * scale, a.1 * scale, b.0 * scale, b.1 * scale);
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.) as usize;
    let radius = (1.5 * scale).round().max(1.) as i32;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let x = (x0 + (x1 - x0) * t).round() as i32;
        let y = (y0 + (y1 - y0) * t).round() as i32;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy <= radius * radius
                    && x + dx >= 0
                    && y + dy >= 0
                    && (x + dx) < img.width() as i32
                    && (y + dy) < img.height() as i32
                {
                    img.put_pixel(
                        (x + dx) as u32,
                        (y + dy) as u32,
                        image::Rgba([244, 91, 105, 255]),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drawing_clips_at_image_edges() {
        let mut i = image::RgbaImage::new(20, 20);
        draw_line(&mut i, (-4., -4.), (24., 24.), 1.);
        assert_eq!(i.get_pixel(10, 10).0, [244, 91, 105, 255]);
    }
}
