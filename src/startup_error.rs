//! WinUI error window for failures before Shelf exists or after its loop exits.
use windows_reactor::*;

pub struct StartupError;
#[derive(Clone)]
pub enum Message {
    Close,
}
impl Component for StartupError {
    type Input = String;
    type Message = Message;
    fn create(_: &String, _: &ComponentContext<Self>) -> Self {
        Self
    }
    fn update(&mut self, _: Message, cx: &ComponentContext<Self>) {
        let _ = cx.window().request_close();
    }
    fn view(&self, message: &String, cx: &mut ViewContext<Self>) -> View {
        cx.window_title("Snapline — 起動エラー");
        cx.window_visuals(
            WindowVisuals::new()
                .client_size(520., 260.)
                .backdrop(WindowBackdrop::Acrylic),
        );
        StackPanel::new().margin(24.).spacing(20.).children((
            TextBlock::new()
                .text("Snapline を起動できませんでした")
                .font_size(20.)
                .text_wrapping(TextWrapping::Wrap),
            TextBlock::new()
                .text(message)
                .text_wrapping(TextWrapping::Wrap),
            Button::new()
                .on_click(cx.message(Message::Close))
                .content("閉じる"),
        ))
    }
}
