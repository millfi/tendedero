use crate::{editor::EditorInput, platform};
use std::path::{Path, PathBuf};
use windows::{
    Foundation::TimeSpan,
    Media::{
        Editing::{MediaClip, MediaComposition, MediaTrimmingPreference},
        Transcoding::TranscodeFailureReason,
    },
    Storage::{CreationCollisionOption, StorageFile, StorageFolder},
    Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize},
    core::HSTRING,
};
use windows_reactor::*;

struct Apartment;
impl Apartment {
    fn new() -> windows::core::Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
fn clip(path: &Path) -> windows::core::Result<MediaClip> {
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(
        path.to_string_lossy().replace('/', "\\"),
    ))?
    .join()?;
    MediaClip::CreateFromFileAsync(&file)?.join()
}
pub fn duration(path: &Path) -> Result<f64, String> {
    let _apartment = Apartment::new().map_err(|e| e.to_string())?;
    clip(path)
        .and_then(|c| c.OriginalDuration())
        .map(|d| d.Duration as f64 / 10_000_000.)
        .map_err(|e| e.to_string())
}
pub fn trim(path: &Path, start: f64, end: f64) -> Result<PathBuf, String> {
    fn run(path: &Path, start: f64, end: f64) -> crate::model::AppResult<PathBuf> {
        let _apartment = Apartment::new()?;
        let clip = clip(path)?;
        let full = clip.OriginalDuration()?.Duration;
        let a = (start * 10_000_000.).round() as i64;
        let b = (end * 10_000_000.).round() as i64;
        if !start.is_finite() || !end.is_finite() || a < 0 || b <= a || b > full {
            return Err("開始・終了時刻が正しくありません".into());
        }
        clip.SetTrimTimeFromStart(TimeSpan { Duration: a })?;
        clip.SetTrimTimeFromEnd(TimeSpan { Duration: full - b })?;
        let composition = MediaComposition::new()?;
        composition.Clips()?.Append(&clip)?;
        let folder = StorageFolder::GetFolderFromPathAsync(&HSTRING::from(
            path.parent()
                .ok_or("保存場所がありません")?
                .to_string_lossy()
                .replace('/', "\\"),
        ))?
        .join()?;
        let output = folder
            .CreateFileAsync(
                &HSTRING::from(format!("{}-trimmed.mp4", crate::model::unique_id())),
                CreationCollisionOption::FailIfExists,
            )?
            .join()?;
        let result = composition
            .RenderToFileWithTrimmingPreferenceAsync(&output, MediaTrimmingPreference::Precise)?
            .join()?;
        if result != TranscodeFailureReason::None {
            return Err(format!("動画の書き出しに失敗しました: {result:?}").into());
        }
        Ok(PathBuf::from(output.Path()?.to_string()))
    }
    run(path, start, end).map_err(|e| e.to_string())
}
pub enum VideoMessage {
    Loaded(Result<f64, String>),
    Start(f64),
    End(f64),
    Save,
    Finished(Result<PathBuf, String>),
    Play,
    Close,
}
pub struct VideoEditor {
    input: EditorInput,
    duration: f64,
    start: f64,
    end: f64,
    busy: bool,
    status: String,
}
impl Component for VideoEditor {
    type Input = EditorInput;
    type Message = VideoMessage;
    fn create(input: &EditorInput, cx: &ComponentContext<Self>) -> Self {
        let path = input.item.path.clone();
        cx.spawn_background(move |_| VideoMessage::Loaded(duration(&path)));
        Self {
            input: input.clone(),
            duration: 0.,
            start: 0.,
            end: 0.,
            busy: true,
            status: "動画を読み込んでいます…".into(),
        }
    }
    fn update(&mut self, m: VideoMessage, cx: &ComponentContext<Self>) {
        match m {
            VideoMessage::Loaded(result) => {
                self.busy = false;
                match result {
                    Ok(d) => {
                        self.duration = d;
                        self.end = d;
                        self.status =
                            "残したい範囲を秒数で指定します。元の動画は変更されません。".into();
                    }
                    Err(e) => self.status = format!("この動画を編集できません: {e}"),
                }
            }
            VideoMessage::Start(v) => self.start = v,
            VideoMessage::End(v) => self.end = v,
            VideoMessage::Play => {
                if let Err(e) = platform::open(&self.input.item.path) {
                    self.status = e.to_string();
                }
            }
            VideoMessage::Save => {
                if self.start >= self.end {
                    self.status = "終了時刻を開始時刻より後にしてください".into();
                    return;
                }
                self.busy = true;
                self.status = "動画を書き出しています…".into();
                let (path, start, end) = (self.input.item.path.clone(), self.start, self.end);
                cx.spawn_background(move |_| VideoMessage::Finished(trim(&path, start, end)));
            }
            VideoMessage::Finished(result) => {
                self.busy = false;
                match result {
                    Ok(p) => {
                        if self.input.saved.call(p) {
                            let _ = cx.window().request_close();
                        } else {
                            self.status = "保存しました。棚を開き直して取り込んでください。".into();
                        }
                    }
                    Err(e) => self.status = e,
                }
            }
            VideoMessage::Close => {
                if !self.busy {
                    let _ = cx.window().request_close();
                }
            }
        }
    }
    fn view(&self, _: &EditorInput, cx: &mut ViewContext<Self>) -> View {
        cx.window_title("Snapline · 動画をトリミング");
        cx.window_visuals(
            WindowVisuals::new()
                .client_size(620., 430.)
                .backdrop(WindowBackdrop::Mica),
        );
        StackPanel::new().margin(28.).spacing(18.).children((
            TextBlock::new()
                .text("動画をトリミング")
                .font_size(26.)
                .font_weight(FontWeight::SEMI_BOLD),
            TextBlock::new()
                .text(&self.input.item.name)
                .font_size(13.)
                .text_trimming(TextTrimming::CharacterEllipsis),
            TextBlock::new().text(format!("開始  {:.2} 秒", self.start)),
            Slider::new()
                .minimum(0.)
                .maximum(self.duration.max(1.))
                .value(self.start)
                .is_enabled(!self.busy && self.duration > 0.)
                .on_value_changed(cx.callback(VideoMessage::Start)),
            TextBlock::new().text(format!(
                "終了  {:.2} 秒   /   全体 {:.2} 秒",
                self.end, self.duration
            )),
            Slider::new()
                .minimum(0.)
                .maximum(self.duration.max(1.))
                .value(self.end)
                .is_enabled(!self.busy && self.duration > 0.)
                .on_value_changed(cx.callback(VideoMessage::End)),
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(8.)
                .children((
                    Button::new()
                        .on_click(cx.callback(|_| VideoMessage::Play))
                        .content("動画を確認"),
                    Button::new()
                        .is_enabled(!self.busy && self.duration > 0.)
                        .on_click(cx.callback(|_| VideoMessage::Save))
                        .content("コピーを保存"),
                    Button::new()
                        .is_enabled(!self.busy)
                        .on_click(cx.callback(|_| VideoMessage::Close))
                        .content("閉じる"),
                )),
            TextBlock::new()
                .text(&self.status)
                .text_wrapping(TextWrapping::Wrap)
                .font_size(12.),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires test-data/fixtures/test-recording.mp4 and the Windows media stack"]
    fn native_trim_preserves_source_and_sets_duration() {
        let source = std::env::current_dir()
            .unwrap()
            .join("test-data/fixtures/test-recording.mp4");
        let before = std::fs::read(&source).unwrap();
        assert!((duration(&source).unwrap() - 3.).abs() < 0.1);
        let output = trim(&source, 0.5, 2.0).unwrap();
        assert!((duration(&output).unwrap() - 1.5).abs() < 0.15);
        assert_eq!(std::fs::read(&source).unwrap(), before);
        std::fs::remove_file(output).unwrap();
    }
}
