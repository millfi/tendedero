//! A real WinUI MenuFlyout, hosted independently of the hidden settings window.
//! Native notifications only enqueue work; WinUI is called from Shelf::update,
//! outside Reactor's native callback / reconciliation stack.
mod abi;

use crate::platform::{self, NativeEvent};
use abi::*;
use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use windows::{
    Foundation::EventHandler,
    Win32::{Foundation::*, UI::WindowsAndMessaging::*},
    core::{HSTRING, IInspectable, Interface, Result},
};

// About the original native menu's size: 132 DIP wide, four 19 DIP rows.
// The compact item template keeps WinUI's menu semantics and theme brushes,
// without the default template's touch-sized / narrow-state padding.
const MENU: &str = r#"
<Grid xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
      xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
  <Grid.Resources>
    <Style x:Key="CompactTrayItem" TargetType="MenuFlyoutItem">
      <Setter Property="FontSize" Value="11" />
      <Setter Property="MinHeight" Value="19" />
      <Setter Property="Padding" Value="10,1" />
      <Setter Property="UseSystemFocusVisuals" Value="True" />
      <Setter Property="Template">
        <Setter.Value>
          <ControlTemplate TargetType="MenuFlyoutItem">
            <Grid x:Name="ItemRoot" Background="{TemplateBinding Background}"
                  Padding="{TemplateBinding Padding}" CornerRadius="3"
                  Control.IsTemplateFocusTarget="True">
              <VisualStateManager.VisualStateGroups>
                <VisualStateGroup x:Name="CommonStates">
                  <VisualState x:Name="Normal" />
                  <VisualState x:Name="PointerOver">
                    <VisualState.Setters>
                      <Setter Target="ItemRoot.Background" Value="{ThemeResource MenuFlyoutItemBackgroundPointerOver}" />
                      <Setter Target="Label.Foreground" Value="{ThemeResource MenuFlyoutItemForegroundPointerOver}" />
                    </VisualState.Setters>
                  </VisualState>
                  <VisualState x:Name="Pressed">
                    <VisualState.Setters>
                      <Setter Target="ItemRoot.Background" Value="{ThemeResource MenuFlyoutItemBackgroundPressed}" />
                      <Setter Target="Label.Foreground" Value="{ThemeResource MenuFlyoutItemForegroundPressed}" />
                    </VisualState.Setters>
                  </VisualState>
                  <VisualState x:Name="Disabled">
                    <VisualState.Setters><Setter Target="Label.Foreground" Value="{ThemeResource MenuFlyoutItemForegroundDisabled}" /></VisualState.Setters>
                  </VisualState>
                </VisualStateGroup>
              </VisualStateManager.VisualStateGroups>
              <TextBlock x:Name="Label" Text="{TemplateBinding Text}"
                         FontSize="{TemplateBinding FontSize}" FontFamily="{TemplateBinding FontFamily}"
                         Foreground="{TemplateBinding Foreground}" VerticalAlignment="Center" />
            </Grid>
          </ControlTemplate>
        </Setter.Value>
      </Setter>
    </Style>
  </Grid.Resources>
  <Grid.ContextFlyout>
<MenuFlyout ShouldConstrainToRootBounds="False" Placement="TopEdgeAlignedRight"
            AreOpenCloseAnimationsEnabled="False">
  <MenuFlyout.SystemBackdrop><DesktopAcrylicBackdrop /></MenuFlyout.SystemBackdrop>
  <MenuFlyout.MenuFlyoutPresenterStyle>
    <Style TargetType="MenuFlyoutPresenter">
      <Setter Property="Width" Value="__WIDTH__" />
      <Setter Property="MinWidth" Value="__WIDTH__" />
      <Setter Property="Padding" Value="3" />
      <Setter Property="Background" Value="Transparent" />
    </Style>
  </MenuFlyout.MenuFlyoutPresenterStyle>
__ITEMS__
</MenuFlyout>
  </Grid.ContextFlyout>
</Grid>"#;
const TRAY_ITEMS: &str = r#"
  <MenuFlyoutItem Text="棚を表示 / 隠す" Style="{StaticResource CompactTrayItem}" />
  <MenuFlyoutItem Text="範囲を撮影" Style="{StaticResource CompactTrayItem}" />
  <MenuFlyoutItem Text="設定" Style="{StaticResource CompactTrayItem}" />
  <MenuFlyoutSeparator Height="1" Padding="0" Margin="4,1" />
  <MenuFlyoutItem Text="Snaplineを終了" Style="{StaticResource CompactTrayItem}" />
"#;

#[derive(Clone, Copy)]
pub enum ItemAction {
    Copy,
    Edit,
    Open,
    Save,
    Pin,
    Dismiss,
}

pub struct TrayMenu {
    window: OwnedWindow,
    hwnd: HWND,
    root: IFrameworkElement,
    flyout: IFlyoutBase,
    clicks: Vec<(IMenuFlyoutItem, i64)>,
    loaded: Option<(IFrameworkElement, i64)>,
    closed: Option<i64>,
    ready: Rc<Cell<bool>>,
    requested: Arc<AtomicBool>,
}

struct OwnedWindow(IWindow);
impl std::ops::Deref for OwnedWindow {
    type Target = IWindow;
    fn deref(&self) -> &IWindow {
        &self.0
    }
}
impl Drop for OwnedWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = (self.0.vtable().Close)(self.0.as_raw());
        }
    }
}

impl TrayMenu {
    pub fn new() -> Result<Self> {
        Self::create(
            132,
            TRAY_ITEMS,
            vec![
                (0, NativeEvent::Toggle),
                (1, NativeEvent::Capture),
                (2, NativeEvent::Settings),
                (4, NativeEvent::Exit),
            ],
        )
    }

    pub fn for_item(id: &str, pinned: bool) -> Result<Self> {
        let mut items = String::new();
        let mut commands = Vec::new();
        for (index, (label, action)) in [
            ("コピー", ItemAction::Copy),
            ("マークアップ", ItemAction::Edit),
            ("既定のアプリで開く", ItemAction::Open),
            ("名前を付けて保存…", ItemAction::Save),
            (
                if pinned {
                    "固定を解除"
                } else {
                    "棚に固定"
                },
                ItemAction::Pin,
            ),
            ("棚から外す", ItemAction::Dismiss),
        ]
        .into_iter()
        .enumerate()
        {
            items.push_str(&format!(
                r#"<MenuFlyoutItem Text="{label}" Style="{{StaticResource CompactTrayItem}}" />"#
            ));
            commands.push((index as u32, NativeEvent::ItemAction(id.to_owned(), action)));
        }
        Self::create(176, &items, commands)
    }

    fn create(width: u32, items: &str, commands: Vec<(u32, NativeEvent)>) -> Result<Self> {
        let xaml = MENU
            .replace("__WIDTH__", &width.to_string())
            .replace("__ITEMS__", items);
        let root: IFrameworkElement = abi::load(&xaml)?.cast()?;
        let ui_root: IUIElement = root.cast()?;
        let flyout = ui_root.context_flyout()?;
        let window = OwnedWindow(abi::window()?);
        let native = window.cast::<IWindowNative>()?;
        let mut handle = std::ptr::null_mut();
        unsafe { (native.vtable().WindowHandle)(native.as_raw(), &mut handle).ok()? };
        // Own the window immediately so every subsequent failure closes it.
        let mut menu = Self {
            window,
            hwnd: HWND(handle),
            root,
            flyout,
            clicks: Vec::new(),
            loaded: None,
            closed: None,
            ready: Rc::new(Cell::new(false)),
            requested: Arc::new(AtomicBool::new(false)),
        };
        unsafe {
            let title = HSTRING::from("Snapline — Tray menu");
            (menu.window.vtable().SetTitle)(menu.window.as_raw(), std::mem::transmute_copy(&title))
                .ok()?;
            menu.window.set_content(&ui_root)?;
            // A one-pixel, borderless tool window supplies a live XamlRoot even
            // while settings are hidden. Only its unconstrained flyout is UI.
            SetWindowLongPtrW(menu.hwnd, GWL_STYLE, WS_POPUP.0 as isize);
            SetWindowLongPtrW(menu.hwnd, GWL_EXSTYLE, WS_EX_TOOLWINDOW.0 as isize);
            SetWindowPos(
                menu.hwnd,
                None,
                0,
                0,
                1,
                1,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED,
            )?;

            let items = menu.flyout.cast::<IMenuFlyout>()?;
            let mut vector = std::ptr::null_mut();
            (items.vtable().Items)(items.as_raw(), &mut vector).ok()?;
            let items = windows_collections::IVector::<MenuFlyoutItemBase>::from_raw(vector);
            for (index, event) in commands {
                let item = items.GetAt(index)?.cast::<IMenuFlyoutItem>()?;
                let handler: RoutedEventHandler =
                    Handler(Box::new(move || platform::emit(event.clone()))).into();
                let mut token = 0;
                (item.vtable().Click)(item.as_raw(), handler.as_raw(), &mut token).ok()?;
                menu.clicks.push((item, token));
            }

            let hwnd = menu.hwnd.0 as usize;
            let requested = menu.requested.clone();
            let closed = EventHandler::<IInspectable>::new(move |_, _| {
                // Do not destroy XAML objects within a flyout event callback.
                requested.store(false, Ordering::Relaxed);
                let _ = ShowWindow(HWND(hwnd as *mut _), SW_HIDE);
                Ok(())
            });
            let mut token = 0;
            (menu.flyout.vtable().Closed)(menu.flyout.as_raw(), closed.as_raw(), &mut token)
                .ok()?;
            menu.closed = Some(token);

            let element = menu.root.clone();
            let flyout = menu.flyout.clone();
            let root = menu.root.clone();
            let ready = menu.ready.clone();
            let requested = menu.requested.clone();
            let loaded: RoutedEventHandler = Handler(Box::new(move || {
                ready.set(true);
                if !requested.load(Ordering::Relaxed) {
                    return;
                }
                if let Err(error) = flyout.show_at(&root) {
                    let _ = ShowWindow(HWND(hwnd as *mut _), SW_HIDE);
                    platform::emit(NativeEvent::TrayMenuError(error.to_string()));
                }
            }))
            .into();
            (element.vtable().Loaded)(element.as_raw(), loaded.as_raw(), &mut token).ok()?;
            menu.loaded = Some((element, token));
        }
        Ok(menu)
    }

    pub fn show(&self, point: POINT) -> Result<()> {
        unsafe {
            (self.flyout.vtable().Hide)(self.flyout.as_raw()).ok()?;
            self.requested.store(true, Ordering::Relaxed);
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                point.x,
                point.y,
                1,
                1,
                SWP_NOACTIVATE,
            )?;
            // Loaded opens the first flyout after the XamlRoot becomes ready.
            // Later openings reuse the same root and the same event handlers.
            let _ = SetForegroundWindow(self.hwnd);
            let ready = self.ready.get();
            (self.window.vtable().Activate)(self.window.as_raw()).ok()?;
            if ready {
                self.flyout.show_at(&self.root)?;
            }
        }
        Ok(())
    }

    pub fn hide(&self) {
        self.requested.store(false, Ordering::Relaxed);
        unsafe {
            let _ = (self.flyout.vtable().Hide)(self.flyout.as_raw());
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    pub fn is_open(&self) -> bool {
        self.requested.load(Ordering::Relaxed)
    }
}

impl Drop for TrayMenu {
    fn drop(&mut self) {
        self.hide();
        unsafe {
            for (item, token) in &self.clicks {
                let _ = (item.vtable().RemoveClick)(item.as_raw(), *token);
            }
            if let Some((element, token)) = &self.loaded {
                let _ = (element.vtable().RemoveLoaded)(element.as_raw(), *token);
            }
            if let Some(token) = self.closed {
                let _ = (self.flyout.vtable().RemoveClosed)(self.flyout.as_raw(), token);
            }
        }
    }
}
