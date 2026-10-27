use crate::{
    editor::{Editor, EditorInput},
    model::{FolderWatcher, Store},
    platform::{self, Host, NativeEvent},
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use windows_reactor::*;

#[derive(Clone)]
pub enum Message {
    Init,
    Native(NativeEvent),
    Hide,
    Settings,
    Snip,
    Capture,
    Import,
    Imported(PathBuf),
    Context(String),
    Copy(String),
    Edit(String),
    Open(String),
    Save(String),
    Dismiss(String),
    Pin(String),
    Restore,
    Clipboard(bool),
    Edge(bool),
    AutoHide(bool),
    Size(u32),
    Folder,
    PickerFinished(Result<crate::pickers::Outcome, String>),
    RemoveFolder(usize),
    DataFolder,
    Page(i32),
    Drop(DroppedData),
    Exit,
}
pub struct Shelf {
    store: Store,
    watcher: FolderWatcher,
    host: Option<Host>,
    status: String,
    settings: bool,
    page: usize,
    clipboard_seq: u32,
    ticks: u64,
    peek_until: Instant,
    edge_since: Option<Instant>,
    away_since: Option<Instant>,
    selection: Option<crate::capture::Selection>,
    flight: Option<crate::capture::Flight>,
    tray_menu: Option<crate::tray_menu::TrayMenu>,
    item_menu: Option<crate::tray_menu::TrayMenu>,
    picker_pending: bool,
}
impl Shelf {
    fn begin_picker(&mut self, request: crate::pickers::Request, cx: &ComponentContext<Self>) {
        if self.picker_pending {
            return;
        }
        match crate::pickers::start(self.hwnd(), request) {
            Ok(pending) => {
                self.picker_pending = true;
                cx.spawn_background(move |_| Message::PickerFinished(pending.complete()));
            }
            Err(error) => self.status = format!("ファイル選択: {error}"),
        }
    }
    fn hwnd(&self) -> windows::Win32::Foundation::HWND {
        self.host.as_ref().map(|h| h.hwnd).unwrap_or_default()
    }
    fn height(&self) -> i32 {
        if self.settings {
            680
        } else {
            (self.store.settings.size as f64 * 0.64 + 222.) as i32
        }
    }
    fn capacity(&self) -> usize {
        self.host
            .as_ref()
            .map(|h| h.line.borrow().capacity())
            .unwrap_or(1)
    }
    fn reveal(&mut self) {
        self.reveal_at(platform::cursor());
    }
    fn reveal_at(&mut self, point: windows::Win32::Foundation::POINT) {
        let height = self.height();
        if let Some(h) = &self.host {
            h.settings(self.settings);
            h.line.borrow_mut().set_monitor(point);
            let _ =
                h.line
                    .borrow_mut()
                    .sync(&self.store.items, self.store.settings.size, self.page);
            h.place(height);
            h.show();
        }
        self.peek_until = Instant::now() + Duration::from_secs(5);
        self.away_since = None;
    }
    fn persist(&mut self) {
        if let Err(e) = self.store.save() {
            self.status = format!("設定を保存できません: {e}");
        }
    }
    fn clamp_page(&mut self) {
        self.page = self
            .page
            .min(self.store.items.len().saturating_sub(1) / self.capacity());
    }
    fn import(&mut self, path: PathBuf) {
        match self.store.import(&path) {
            Ok(_) => {
                self.status = "棚に追加しました".into();
                self.page = 0;
                self.clamp_page();
                self.reveal();
            }
            Err(e) => self.status = format!("取り込めません: {e}"),
        }
    }
    fn capturing(&self) -> bool {
        self.selection.is_some() || self.flight.is_some()
    }
    fn captured(&mut self, image: image::DynamicImage, rect: windows::Win32::Foundation::RECT) {
        let result = (|| -> crate::model::AppResult<()> {
            let id = self.store.add_image(&image, "画面キャプチャ")?;
            self.page = self
                .store
                .items
                .iter()
                .rev()
                .position(|i| i.id == id)
                .unwrap_or(0)
                / self.capacity();
            let point = windows::Win32::Foundation::POINT {
                x: rect.left + (rect.right - rect.left) / 2,
                y: rect.top + (rect.bottom - rect.top) / 2,
            };
            self.reveal_at(point);
            // A destination monitor can have a different page capacity.
            self.page = self
                .store
                .items
                .iter()
                .rev()
                .position(|i| i.id == id)
                .unwrap_or(0)
                / self.capacity();
            self.reveal_at(point);
            if let Some(host) = &self.host {
                let item = self
                    .store
                    .items
                    .iter()
                    .find(|i| i.id == id)
                    .ok_or("撮影した画像が見つかりません")?;
                self.flight = Some(crate::capture::Flight::start(
                    &host.line.borrow(),
                    item,
                    &image,
                    rect,
                )?);
            }
            Ok(())
        })();
        self.status = match result {
            Ok(()) => "撮影して棚に追加しました".into(),
            Err(e) => format!("キャプチャ: {e}"),
        };
        if self.flight.is_none() {
            self.reveal();
        }
    }
    fn tick(&mut self, _cx: &ComponentContext<Self>) {
        self.ticks += 1;
        if let Some(h) = &self.host {
            h.line.borrow_mut().tick();
        }
        if self.ticks == 5 && !self.capturing() {
            self.reveal();
            let _ = std::fs::write(
                self.store.root.join("startup.log"),
                format!(
                    "Tick: visible={}, fullscreen={}",
                    self.host.as_ref().is_some_and(|h| h.visible()),
                    platform::fullscreen_foreground(self.hwnd())
                ),
            );
        }
        let now = Instant::now();
        if self.flight.as_ref().is_some_and(|f| f.complete()) {
            self.flight.take();
            self.peek_until = now + Duration::from_secs(5);
        }
        if self.capturing() || self.picker_pending {
            return;
        }
        let seq = platform::clipboard_sequence();
        if seq != self.clipboard_seq {
            if self.store.settings.clipboard {
                match platform::read_clipboard(self.hwnd()) {
                    Ok(Some(img)) => {
                        self.clipboard_seq = seq;
                        match self.store.add_image(&img, "クリップボードの画像") {
                            Ok(_) => {
                                self.clamp_page();
                                self.status = "クリップボードから追加しました".into();
                                self.page = 0;
                                self.reveal();
                            }
                            Err(e) => self.status = e.to_string(),
                        }
                    }
                    Ok(None) => self.clipboard_seq = seq,
                    Err(_) => (), // Clipboard is often held briefly by the producer; retry next tick.
                }
            } else {
                self.clipboard_seq = seq;
            }
        }
        if self.ticks.is_multiple_of(15) {
            self.clamp_page();
            let ready = self.watcher.poll(&self.store.settings.folders);
            for path in ready {
                if let Err(e) = self.store.import(&path) {
                    self.watcher.retry(&path);
                    self.status = format!("取り込みを再試行します: {e}");
                } else {
                    self.clamp_page();
                    self.page = 0;
                    self.status = "監視フォルダーから追加しました".into();
                    self.reveal();
                }
            }
        }
        if self.tray_menu.as_ref().is_some_and(|menu| menu.is_open())
            || self.item_menu.as_ref().is_some_and(|menu| menu.is_open())
        {
            return;
        }
        let Some(h) = &self.host else {
            return;
        };
        if self.store.settings.auto_hide
            && !self.settings
            && now > self.peek_until
            && platform::fullscreen_foreground(h.hwnd)
        {
            if h.visible() {
                h.hide();
            }
            self.edge_since = None;
            return;
        }
        let p = platform::cursor();
        let r = platform::monitor_rect(false);
        if self.store.settings.edge && p.y <= r.top + 2 {
            let since = self.edge_since.get_or_insert(now);
            if now.duration_since(*since) > Duration::from_millis(450) && !h.visible() {
                self.reveal();
            }
        } else {
            self.edge_since = None;
        }
        let Some(h) = &self.host else {
            return;
        };
        if h.visible()
            && self.store.settings.auto_hide
            && !self.settings
            && !h.line.borrow().interacting()
            && now > self.peek_until
        {
            if h.contains_cursor() {
                self.away_since = None;
            } else {
                let since = self.away_since.get_or_insert(now);
                if now.duration_since(*since) > Duration::from_millis(800) {
                    h.hide();
                }
            }
        }
    }
}
impl Component for Shelf {
    type Input = PathBuf;
    type Message = Message;
    fn create(root: &PathBuf, _cx: &ComponentContext<Self>) -> Self {
        let _ = std::fs::write(root.join("startup.log"), "Creating shelf\n");
        let store = Store::load(root.clone()).expect("preflight validated storage");
        let watcher = FolderWatcher::new(&store.settings.folders);
        let mut app = Self {
            store,
            watcher,
            host: None,
            status: "撮影した画像を、いつでも手元に。".into(),
            settings: false,
            page: 0,
            clipboard_seq: platform::clipboard_sequence(),
            ticks: 0,
            peek_until: Instant::now() + Duration::from_secs(12),
            edge_since: None,
            away_since: None,
            selection: None,
            flight: None,
            tray_menu: None,
            item_menu: None,
            picker_pending: false,
        };
        app.clamp_page();
        app
    }
    fn update(&mut self, m: Message, cx: &ComponentContext<Self>) {
        if self.picker_pending
            && matches!(
                &m,
                Message::Import
                    | Message::Folder
                    | Message::Save(_)
                    | Message::Capture
                    | Message::Snip
                    | Message::Native(
                        NativeEvent::Capture | NativeEvent::Toggle | NativeEvent::Settings
                    )
            )
        {
            return;
        }
        if self.capturing()
            && !matches!(
                &m,
                Message::Native(
                    NativeEvent::Tick
                        | NativeEvent::Selected(_)
                        | NativeEvent::CaptureCancelled
                        | NativeEvent::Exit
                ) | Message::Exit
            )
        {
            return;
        }
        if !matches!(
            &m,
            Message::Native(NativeEvent::Tick | NativeEvent::TrayMenu(_))
        ) && let Some(menu) = &self.tray_menu
        {
            menu.hide();
        }
        if !matches!(&m, Message::Native(NativeEvent::Tick))
            && let Some(menu) = &self.item_menu
        {
            menu.hide();
        }
        match m {
            Message::Init => {
                let _ = std::fs::write(
                    self.store.root.join("startup.log"),
                    "Attaching native host\n",
                );
                let sender = cx.sender();
                match Host::attach(move |e| {
                    sender.send(Message::Native(e));
                }) {
                    Ok(host) => {
                        if !host.hotkey_ok {
                            self.status =
                                "Ctrl+Alt+Tは別のアプリが使用中です。トレイから操作できます。"
                                    .into();
                        }
                        let _ = std::fs::write(
                            self.store.root.join("startup.log"),
                            format!("Attached {:?}", host.hwnd),
                        );
                        self.host = Some(host);
                        self.reveal();
                    }
                    Err(e) => {
                        self.status = e.to_string();
                        let _ = std::fs::write(
                            self.store.root.join("startup.log"),
                            format!("Attach error: {e}"),
                        );
                    }
                }
            }
            Message::Native(NativeEvent::Tick) => self.tick(cx),
            Message::Native(NativeEvent::TrayMenu(point)) => {
                let result = (|| -> windows::core::Result<()> {
                    if self.tray_menu.is_none() {
                        self.tray_menu = Some(crate::tray_menu::TrayMenu::new()?);
                    }
                    self.tray_menu.as_ref().unwrap().show(point)
                })();
                if let Err(error) = result {
                    self.tray_menu.take();
                    self.status = format!("トレイメニューを表示できません: {error}");
                    self.settings = true;
                    self.reveal();
                }
            }
            Message::Native(NativeEvent::TrayMenuError(error)) => {
                self.tray_menu.take();
                self.item_menu.take();
                self.status = format!("トレイメニューを表示できません: {error}");
                self.settings = true;
                self.reveal();
            }
            Message::Native(NativeEvent::ItemAction(id, action)) => {
                use crate::tray_menu::ItemAction;
                self.item_menu.take();
                cx.sender().send(match action {
                    ItemAction::Copy => Message::Copy(id),
                    ItemAction::Edit => Message::Edit(id),
                    ItemAction::Open => Message::Open(id),
                    ItemAction::Save => Message::Save(id),
                    ItemAction::Pin => Message::Pin(id),
                    ItemAction::Dismiss => Message::Dismiss(id),
                });
            }
            Message::Native(NativeEvent::Selected(rect)) => {
                if let Some(selection) = self.selection.take() {
                    let result = selection.crop(rect);
                    match result {
                        Ok(image) => self.captured(image, rect),
                        Err(e) => {
                            self.status = e.to_string();
                            self.reveal();
                        }
                    }
                    // Keep the frozen desktop on screen until the flight is ready.
                    drop(selection);
                }
            }
            Message::Native(NativeEvent::CaptureCancelled) => {
                if self.selection.take().is_some() {
                    self.status = "撮影をキャンセルしました".into();
                    self.reveal();
                }
            }
            Message::Native(NativeEvent::Overlay(e)) => {
                use crate::clothesline::Event;
                let message = match e {
                    Event::Copy(id) => Message::Copy(id),
                    Event::Open(id) => Message::Open(id),
                    Event::Edit(id) => Message::Edit(id),
                    Event::Context(id) => Message::Context(id),
                    Event::Dismiss(id) => Message::Dismiss(id),
                    Event::Settings => Message::Settings,
                    Event::Hide => Message::Hide,
                    Event::Page(n) => Message::Page(n),
                    Event::Drag(id) => {
                        if let Some(item) = self.store.items.iter().find(|i| i.id == id) {
                            let _ = platform::drag_file(&item.path);
                        }
                        self.peek_until = Instant::now() + Duration::from_secs(5);
                        return;
                    }
                };
                cx.sender().send(message);
            }
            Message::Native(NativeEvent::Show) => self.reveal(),
            Message::Native(NativeEvent::Toggle) => {
                if self.host.as_ref().is_some_and(|h| h.visible()) {
                    if let Some(h) = &self.host {
                        h.hide();
                    }
                } else {
                    self.reveal();
                }
            }
            Message::Hide | Message::Native(NativeEvent::CloseSettings) => {
                self.settings = false;
                if let Some(h) = &self.host {
                    h.hide();
                }
            }
            Message::Native(NativeEvent::Settings) | Message::Settings => {
                self.settings = !self.settings;
                self.reveal();
            }
            Message::Native(NativeEvent::Capture) | Message::Snip => {
                self.settings = false;
                if let Some(h) = &self.host {
                    // Capture the current desktop immediately, including the shelf.
                    let result = crate::capture::Selection::start(&h.line.borrow());
                    match result {
                        Ok(selection) => self.selection = Some(selection),
                        Err(e) => {
                            self.status = format!("キャプチャ: {e}");
                            self.reveal();
                        }
                    }
                }
            }
            Message::Capture => {
                self.settings = false;
                let rect = platform::monitor_rect(false);
                match platform::capture_rect(rect) {
                    Ok(image) => self.captured(image, rect),
                    Err(e) => {
                        self.status = e.to_string();
                        self.reveal();
                    }
                }
            }
            Message::Import => {
                self.begin_picker(crate::pickers::Request::Import, cx);
            }
            Message::Imported(path) => self.import(path),
            Message::Copy(id) => {
                if let Some(item) = self.store.items.iter().find(|i| i.id == id) {
                    let result = if item.video {
                        platform::open(&item.path)
                    } else {
                        platform::copy_image(self.hwnd(), &item.path)
                    };
                    self.status = match result {
                        Ok(()) => {
                            if item.video {
                                "動画を開きました".into()
                            } else {
                                "画像をコピーしました ✓".into()
                            }
                        }
                        Err(e) => e.to_string(),
                    };
                    if let Some(h) = &self.host {
                        h.line.borrow_mut().copied(&id);
                    }
                    self.clipboard_seq = platform::clipboard_sequence();
                }
            }
            Message::Edit(id) => {
                if let Some(host) = &self.host {
                    host.hide();
                }
                if let Some(item) = self.store.items.iter().find(|i| i.id == id).cloned() {
                    if item.video {
                        let _ = cx.open_window(View::component::<crate::video::VideoEditor>(
                            EditorInput {
                                item,
                                saved: cx.sender().callback(Message::Imported),
                            },
                        ));
                    } else {
                        let _ = cx.open_window(View::component::<Editor>(EditorInput {
                            item,
                            saved: cx.sender().callback(Message::Imported),
                        }));
                    }
                    self.peek_until = Instant::now() + Duration::from_secs(5);
                }
            }
            Message::Open(id) => {
                if let Some(host) = &self.host {
                    host.hide();
                }
                if let Some(i) = self.store.items.iter().find(|i| i.id == id)
                    && let Err(e) = platform::open(&i.path)
                {
                    self.status = e.to_string();
                }
            }
            Message::Save(id) => {
                if let Some(i) = self.store.items.iter().find(|i| i.id == id) {
                    self.begin_picker(crate::pickers::Request::Save(i.path.clone()), cx);
                }
            }
            Message::Dismiss(id) => {
                self.store.dismiss(&id);
                self.persist();
                self.clamp_page();
                self.status = "棚から外しました。「戻す」で復元できます。".into();
            }
            Message::Pin(id) => {
                if let Some(i) = self.store.items.iter_mut().find(|i| i.id == id) {
                    i.pinned = !i.pinned;
                }
                self.persist();
            }
            Message::Restore => {
                self.store.restore();
                self.persist();
                self.clamp_page();
                self.status = "最後に外した画像を戻しました".into();
            }
            Message::Clipboard(v) => {
                self.store.settings.clipboard = v;
                self.clipboard_seq = platform::clipboard_sequence();
                self.persist();
            }
            Message::Edge(v) => {
                self.store.settings.edge = v;
                self.persist();
            }
            Message::AutoHide(v) => {
                self.store.settings.auto_hide = v;
                self.persist();
            }
            Message::Size(v) => {
                self.store.settings.size = v;
                self.page = 0;
                self.persist();
                self.reveal();
            }
            Message::Folder => self.begin_picker(crate::pickers::Request::Folder, cx),
            Message::PickerFinished(result) => {
                use crate::pickers::Outcome;
                self.picker_pending = false;
                self.peek_until = Instant::now() + Duration::from_secs(5);
                match result {
                    Ok(Outcome::Import(paths)) => {
                        if !paths.is_empty() {
                            self.settings = false;
                        }
                        for path in paths {
                            self.import(path);
                        }
                    }
                    Ok(Outcome::Folder(path)) => {
                        let paths = path.into_iter();
                        for path in paths {
                            if !self.store.settings.folders.contains(&path) {
                                self.store.settings.folders.push(path);
                            }
                        }
                        self.watcher = FolderWatcher::new(&self.store.settings.folders);
                        self.persist();
                    }
                    Ok(Outcome::Saved(true)) => self.status = "保存しました".into(),
                    Ok(Outcome::Saved(false)) => (),
                    Err(error) => self.status = format!("ファイル選択: {error}"),
                }
            }
            Message::RemoveFolder(i) => {
                if i < self.store.settings.folders.len() {
                    self.store.settings.folders.remove(i);
                    self.watcher = FolderWatcher::new(&self.store.settings.folders);
                    self.persist();
                }
            }
            Message::DataFolder => {
                if let Err(e) = platform::open(&self.store.root) {
                    self.status = e.to_string();
                }
            }
            Message::Context(id) => {
                let pinned = self
                    .store
                    .items
                    .iter()
                    .find(|i| i.id == id)
                    .is_some_and(|i| i.pinned);
                self.item_menu.take();
                match crate::tray_menu::TrayMenu::for_item(&id, pinned).and_then(|menu| {
                    menu.show(platform::cursor())?;
                    Ok(menu)
                }) {
                    Ok(menu) => self.item_menu = Some(menu),
                    Err(error) => {
                        self.status = format!("写真のメニューを表示できません: {error}");
                        self.settings = true;
                        self.reveal();
                    }
                }
                self.peek_until = Instant::now() + Duration::from_secs(5);
            }
            Message::Page(delta) => {
                self.page = (self.page as i32 + delta).max(0) as usize;
                self.page = self
                    .page
                    .min(self.store.items.len().saturating_sub(1) / self.capacity());
            }
            Message::Drop(DroppedData::StorageItems(items)) => {
                for i in items {
                    self.import(PathBuf::from(i.path));
                }
            }
            Message::Drop(_) => (),
            Message::Exit | Message::Native(NativeEvent::Exit) => {
                self.persist();
                self.selection.take();
                self.flight.take();
                self.tray_menu.take();
                self.item_menu.take();
                self.host.take();
                let _ = cx.window().request_close();
            }
        }
        if !self.capturing()
            && let Some(h) = &self.host
            && let Err(e) =
                h.line
                    .borrow_mut()
                    .sync(&self.store.items, self.store.settings.size, self.page)
        {
            self.status = format!("描画できません: {e}");
        }
    }
    fn view(&self, _: &PathBuf, cx: &mut ViewContext<Self>) -> View {
        cx.window_title("Snapline");
        cx.window_visuals(
            WindowVisuals::new()
                .client_size(820., 680.)
                .icon(platform::icon_path())
                .backdrop(WindowBackdrop::Acrylic),
        );
        let sender = cx.sender();
        cx.use_effect("native", (), move || {
            sender.send(Message::Init);
            None
        });
        Border::new().drop_policy(DragDropPolicy::new().storage_items(DragDropAction::new(DragDropOperation::Copy).caption("棚に追加"))).on_drop(cx.callback(Message::Drop)).content(
        StackPanel::new().spacing(24.).margin(28.).children((
            Grid::new().columns([GridLength::Star(1.),GridLength::Auto]).children((
                TextBlock::new().text("Snapline の設定").font_size(26.).font_weight(FontWeight::SEMI_BOLD),
                Button::new().grid_column(1).on_click(cx.message(Message::Settings)).content("閉じる"),
            )),
            self.settings_view(cx),
            StackPanel::new().orientation(Orientation::Horizontal).spacing(10.).children((
                Button::new().on_click(cx.message(Message::Snip)).content("範囲を撮影"),
                Button::new().on_click(cx.message(Message::Capture)).content("画面全体"),
                Button::new().on_click(cx.message(Message::Import)).content("画像を取り込む…"),
                Button::new().on_click(cx.message(Message::Restore)).content("最後に外した画像を戻す"),
            )),
            TextBlock::new().text(&self.status).font_size(12.).opacity(0.65),
            TextBlock::new().text("Ctrl+Alt+T：棚を表示　·　Ctrl+Alt+S：範囲を撮影\n写真をクリック：コピー　·　長押し：編集　·　右クリック：操作メニュー").font_size(12.).opacity(0.7),
        ))
)
    }
}
impl Shelf {
    fn settings_view(&self, cx: &ViewContext<Self>) -> View {
        let folders = self
            .store
            .settings
            .folders
            .iter()
            .enumerate()
            .map(|(n, p)| {
                KeyedView::new(
                    n,
                    Grid::new()
                        .columns([GridLength::Star(1.), GridLength::Auto])
                        .children((
                            TextBlock::new()
                                .text(p.display().to_string())
                                .font_size(12.)
                                .text_trimming(TextTrimming::CharacterEllipsis)
                                .vertical_alignment(VerticalAlignment::Center),
                            Button::new()
                                .grid_column(1)
                                .on_click(cx.message(Message::RemoveFolder(n)))
                                .content("監視を外す"),
                        )),
                )
            });
        ScrollViewer::new().vertical_scroll_bar_visibility(ScrollBarVisibility::Auto).content(StackPanel::new().spacing(12.).children((
            TextBlock::new().text("いつもの撮影を、もっと身軽に。").font_size(18.).font_weight(FontWeight::SEMI_BOLD),
            StackPanel::new().orientation(Orientation::Horizontal).spacing(32.).children((
                StackPanel::new().spacing(4.).children(("コピーした画像を取り込む",ToggleSwitch::new().automation_name("コピーした画像を取り込む").is_on(self.store.settings.clipboard).on_toggled(cx.callback(Message::Clipboard)))),
                StackPanel::new().spacing(4.).children(("上端に触れて表示",ToggleSwitch::new().automation_name("上端に触れて表示").is_on(self.store.settings.edge).on_toggled(cx.callback(Message::Edge)))),
                StackPanel::new().spacing(4.).children(("離れたら自動で隠す",ToggleSwitch::new().automation_name("離れたら自動で隠す").is_on(self.store.settings.auto_hide).on_toggled(cx.callback(Message::AutoHide)))),
            )),
            TextBlock::new().text("画像のみを取り込みます。テキスト・ファイルと混在した内容や非公開の指定がある内容は取り込みません。").font_size(11.).opacity(0.65),
            StackPanel::new().orientation(Orientation::Horizontal).spacing(8.).children((
                TextBlock::new().text("写真のサイズ").vertical_alignment(VerticalAlignment::Center),
                Button::new().on_click(cx.message(Message::Size(140))).content("小"),Button::new().on_click(cx.message(Message::Size(180))).content("中"),Button::new().on_click(cx.message(Message::Size(240))).content("大"),
                Button::new().on_click(cx.message(Message::Folder)).content("監視フォルダーを追加…"),Button::new().on_click(cx.message(Message::DataFolder)).content("保存場所を開く"),
            )),
            StackPanel::new().spacing(4.).keyed_children(folders),
            TextBlock::new().text("既存ファイルは読み込まず、新しい画像・動画だけを取り込みます。棚は30枚まで（固定した項目を除く）。外した項目は保存場所に残り、「戻す」で復元できます。").font_size(11.).text_wrapping(TextWrapping::Wrap).opacity(0.65),
            Button::new().on_click(cx.message(Message::Exit)).content("Snaplineを終了"),
        )))
    }
}
