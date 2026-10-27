//! Narrow WinUI 3 ABI projection for the tray flyout. Interface IDs and slot
//! order follow Microsoft.UI.Xaml.winmd (Windows App SDK 2 / WinUI 2.3).
//! Unused slots are retained so later methods have their original ABI offsets.
//! Reactor keeps its WinUI projection private; these interfaces use the app's
//! windows-core 0.62 and never exchange Rust wrappers with Reactor's 0.100.
#![allow(non_snake_case)]

use std::ffi::c_void;
use windows_core::*;

pub type Object = *mut c_void;
type GetObject = unsafe extern "system" fn(Object, *mut Object) -> HRESULT;
type SetObject = unsafe extern "system" fn(Object, Object) -> HRESULT;
type Action = unsafe extern "system" fn(Object) -> HRESULT;
type AddEvent = unsafe extern "system" fn(Object, Object, *mut i64) -> HRESULT;
type RemoveEvent = unsafe extern "system" fn(Object, i64) -> HRESULT;

imp::define_interface!(
    IWindow,
    IWindow_Vtbl,
    0x61f0ec79_5d52_56b5_86fb_40fa4af288b0
);
#[repr(C)]
pub struct IWindow_Vtbl {
    pub base__: IInspectable_Vtbl,
    unused0: [usize; 3],
    SetContent: SetObject,
    unused1: [usize; 5],
    pub SetTitle: SetObject,
    unused2: [usize; 10],
    pub Activate: Action,
    pub Close: Action,
}
impl IWindow {
    pub fn set_content(&self, content: &IUIElement) -> Result<()> {
        // Window.Content is UIElement; use its default interface, not another
        // interface obtained from the same XAML object.
        unsafe { (self.vtable().SetContent)(self.as_raw(), content.as_raw()).ok() }
    }
}

imp::define_interface!(
    IWindowFactory,
    IWindowFactory_Vtbl,
    0xf0441536_afef_5222_918f_324a9b2dec75
);
#[repr(C)]
pub struct IWindowFactory_Vtbl {
    pub base__: IInspectable_Vtbl,
    pub CreateInstance:
        unsafe extern "system" fn(Object, Object, *mut Object, *mut Object) -> HRESULT,
}
pub struct WindowClass;
impl RuntimeName for WindowClass {
    const NAME: &'static str = "Microsoft.UI.Xaml.Window";
}

imp::define_interface!(
    IWindowNative,
    IWindowNative_Vtbl,
    0xeecdbf0e_bae9_4cb6_a68e_9598e1cb57bb
);
#[repr(C)]
pub struct IWindowNative_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub WindowHandle: GetObject,
}

imp::define_interface!(
    IXamlReaderStatics,
    IXamlReaderStatics_Vtbl,
    0x82a4cd9e_435e_5aeb_8c4f_300cece45cae
);
#[repr(C)]
pub struct IXamlReaderStatics_Vtbl {
    pub base__: IInspectable_Vtbl,
    pub Load: unsafe extern "system" fn(Object, Object, *mut Object) -> HRESULT,
}
pub struct XamlReaderClass;
impl RuntimeName for XamlReaderClass {
    const NAME: &'static str = "Microsoft.UI.Xaml.Markup.XamlReader";
}

imp::define_interface!(
    IUIElement,
    IUIElement_Vtbl,
    0xc3c01020_320c_5cf6_9d24_d396bbfa4d8b
);
#[repr(C)]
pub struct IUIElement_Vtbl {
    pub base__: IInspectable_Vtbl,
    unused: [usize; 39],
    ContextFlyout: GetObject,
}
impl IUIElement {
    pub fn context_flyout(&self) -> Result<IFlyoutBase> {
        let mut flyout = std::ptr::null_mut();
        unsafe {
            (self.vtable().ContextFlyout)(self.as_raw(), &mut flyout).ok()?;
            IFlyoutBase::from_abi(flyout)
        }
    }
}
imp::define_interface!(
    IFrameworkElement,
    IFrameworkElement_Vtbl,
    0xfe08f13d_dc6a_5495_ad44_c2d8d21863b0
);
#[repr(C)]
pub struct IFrameworkElement_Vtbl {
    pub base__: IInspectable_Vtbl,
    unused: [usize; 55],
    pub Loaded: AddEvent,
    pub RemoveLoaded: RemoveEvent,
}

imp::define_interface!(
    IFlyoutBase,
    IFlyoutBase_Vtbl,
    0xbb6603bf_744d_5c31_a87d_744394634d77
);
#[repr(C)]
pub struct IFlyoutBase_Vtbl {
    pub base__: IInspectable_Vtbl,
    unused0: [usize; 26],
    pub Closed: AddEvent,
    pub RemoveClosed: RemoveEvent,
    unused1: [usize; 4],
    ShowAt: SetObject,
    unused2: usize,
    pub Hide: Action,
}
impl IFlyoutBase {
    pub fn show_at(&self, target: &IFrameworkElement) -> Result<()> {
        // ShowAt(FrameworkElement) requires IFrameworkElement's default
        // interface pointer. An IUIElement pointer is a different COM vtable,
        // even for the same Grid, and passing it here can crash inside WinUI.
        unsafe { (self.vtable().ShowAt)(self.as_raw(), target.as_raw()).ok() }
    }
}

imp::define_interface!(
    IMenuFlyout,
    IMenuFlyout_Vtbl,
    0xf4c77c6c_1fa5_5d85_8559_5d02b7d4e5e7
);
#[repr(C)]
pub struct IMenuFlyout_Vtbl {
    pub base__: IInspectable_Vtbl,
    pub Items: GetObject,
}
imp::define_interface!(
    IMenuFlyoutItemBase,
    IInspectable_Vtbl,
    0x4bee2715_44a1_5f94_86e8_02ddbe3dc6b9
);
impl RuntimeType for IMenuFlyoutItemBase {
    const SIGNATURE: imp::ConstBuffer = imp::ConstBuffer::for_interface::<Self>();
}
#[repr(transparent)]
#[derive(Clone)]
pub struct MenuFlyoutItemBase(IUnknown);
unsafe impl Interface for MenuFlyoutItemBase {
    type Vtable = IInspectable_Vtbl;
    const IID: GUID = IMenuFlyoutItemBase::IID;
}
impl RuntimeType for MenuFlyoutItemBase {
    const SIGNATURE: imp::ConstBuffer = imp::ConstBuffer::for_class::<Self, IMenuFlyoutItemBase>();
}
impl RuntimeName for MenuFlyoutItemBase {
    const NAME: &'static str = "Microsoft.UI.Xaml.Controls.MenuFlyoutItemBase";
}
imp::define_interface!(
    IMenuFlyoutItem,
    IMenuFlyoutItem_Vtbl,
    0x4252df5a_44f9_5ee8_b1cc_53de9aaa4095
);
#[repr(C)]
pub struct IMenuFlyoutItem_Vtbl {
    pub base__: IInspectable_Vtbl,
    unused: [usize; 11],
    pub Click: AddEvent,
    pub RemoveClick: RemoveEvent,
}

#[interface("dae23d85-69ca-5bdf-805b-6161a3a215cc")]
pub unsafe trait RoutedEventHandler: IUnknown {
    fn Invoke(&self, sender: Object, args: Object) -> HRESULT;
}

#[implement(RoutedEventHandler)]
pub struct Handler(pub Box<dyn Fn()>);
impl RoutedEventHandler_Impl for Handler_Impl {
    unsafe fn Invoke(&self, _: Object, _: Object) -> HRESULT {
        (self.0)();
        HRESULT(0)
    }
}

pub fn load(xaml: &str) -> Result<IInspectable> {
    let factory: IXamlReaderStatics = imp::load_factory::<XamlReaderClass, _>()?;
    let xaml = HSTRING::from(xaml);
    let mut result = std::ptr::null_mut();
    unsafe {
        (factory.vtable().Load)(
            factory.as_raw(),
            std::mem::transmute_copy(&xaml),
            &mut result,
        )
        .ok()?;
        IInspectable::from_abi(result)
    }
}

pub fn window() -> Result<IWindow> {
    let factory: IWindowFactory = imp::load_factory::<WindowClass, _>()?;
    let mut result = std::ptr::null_mut();
    unsafe {
        (factory.vtable().CreateInstance)(
            factory.as_raw(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut result,
        )
        .ok()?;
        IWindow::from_abi(result)
    }
}
