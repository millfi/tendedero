//! Narrow projection of Microsoft.Windows.Storage.Pickers (Windows App SDK).
//! IDs and slot order follow Microsoft.Windows.Storage.Pickers.winmd 2.3.12.
//! Raw calls stay private; the UI module exchanges typed operations and paths.
#![allow(non_snake_case)]
use std::{ffi::c_void, path::Path};
use windows::{
    Win32::{Foundation::*, System::LibraryLoader::*},
    core::*,
};
use windows_collections::{IVector, IVectorView};
use windows_future::IAsyncOperation;

type Object = *mut c_void;
type GetObject = unsafe extern "system" fn(Object, *mut Object) -> HRESULT;
type SetString = unsafe extern "system" fn(Object, Object) -> HRESULT;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WindowId {
    value: u64,
}

pub fn window_id(hwnd: HWND) -> Result<WindowId> {
    // This is the implementation prescribed by Microsoft.UI.Interop.h; a
    // WindowId is not obtained by reinterpreting an HWND as an integer.
    unsafe {
        let module = LoadLibraryW(w!("Microsoft.Internal.FrameworkUdk.dll"))?;
        let result = (|| {
            let function = GetProcAddress(module, s!("Windowing_GetWindowIdFromWindow"))
                .ok_or_else(Error::from_thread)?;
            let function: unsafe extern "system" fn(HWND, *mut WindowId) -> HRESULT =
                std::mem::transmute(function);
            let mut id = WindowId { value: 0 };
            function(hwnd, &mut id).ok()?;
            Ok(id)
        })();
        let _ = FreeLibrary(module);
        result
    }
}

#[repr(C)]
pub struct FactoryVtbl {
    base: IInspectable_Vtbl,
    CreateInstance: unsafe extern "system" fn(Object, WindowId, *mut Object) -> HRESULT,
}
imp::define_interface!(
    IOpenFactory,
    FactoryVtbl,
    0x315e86d7_d7a2_5d81_b379_7af78207b1af
);
imp::define_interface!(
    IFolderFactory,
    FactoryVtbl,
    0xe1550d89_b389_5886_8395_022b1588d6a8
);
imp::define_interface!(
    ISaveFactory,
    FactoryVtbl,
    0x2e256696_30b6_5a05_a8f5_c752db6dd268
);
struct OpenClass;
struct FolderClass;
struct SaveClass;
impl RuntimeName for OpenClass {
    const NAME: &'static str = "Microsoft.Windows.Storage.Pickers.FileOpenPicker";
}
impl RuntimeName for FolderClass {
    const NAME: &'static str = "Microsoft.Windows.Storage.Pickers.FolderPicker";
}
impl RuntimeName for SaveClass {
    const NAME: &'static str = "Microsoft.Windows.Storage.Pickers.FileSavePicker";
}

fn create<C: RuntimeName, F: Interface<Vtable = FactoryVtbl>>(
    id: WindowId,
) -> Result<IInspectable> {
    let factory: F = imp::load_factory::<C, F>()?;
    let mut value = std::ptr::null_mut();
    unsafe {
        (factory.vtable().CreateInstance)(factory.as_raw(), id, &mut value).ok()?;
        IInspectable::from_abi(value)
    }
}

#[repr(C)]
pub struct OpenVtbl {
    base: IInspectable_Vtbl,
    unused: [usize; 6],
    FileTypeFilter: GetObject,
    PickSingleFileAsync: usize,
    PickMultipleFilesAsync: GetObject,
}
imp::define_interface!(IOpen, OpenVtbl, 0x9d00f175_c783_51bd_8c93_fb63695d3abc);
#[repr(C)]
pub struct FolderVtbl {
    base: IInspectable_Vtbl,
    unused: [usize; 6],
    PickSingleFolderAsync: GetObject,
}
imp::define_interface!(IFolder, FolderVtbl, 0x3ef0d1ca_97c6_5873_8ea2_02c450174290);
#[repr(C)]
pub struct SaveVtbl {
    base: IInspectable_Vtbl,
    unused0: [usize; 6],
    SetDefaultFileExtension: SetString,
    SuggestedFileName: usize,
    SetSuggestedFileName: SetString,
    unused1: [usize; 2],
    PickSaveFileAsync: GetObject,
}
imp::define_interface!(ISave, SaveVtbl, 0x79f1f4df_741b_59b2_aa06_fe9ac817b7dd);

#[repr(C)]
pub struct ResultVtbl {
    base: IInspectable_Vtbl,
    Path: unsafe extern "system" fn(Object, *mut HSTRING) -> HRESULT,
}
imp::define_interface!(
    PickFileResult,
    ResultVtbl,
    0xe6f2e3d6_7bb0_5d81_9e7d_6fd35a1f25ab
);
imp::define_interface!(
    PickFolderResult,
    ResultVtbl,
    0x6f7fd316_fe29_59d1_9343_c49cf5cde680
);
macro_rules! result_class {
    ($name:ident, $runtime:literal) => {
        impl RuntimeType for $name {
            const SIGNATURE: imp::ConstBuffer = imp::ConstBuffer::new()
                .push_slice(concat!("rc(", $runtime, ";").as_bytes())
                .push_other(imp::ConstBuffer::for_interface::<Self>())
                .push_slice(b")");
        }
        impl $name {
            pub fn path(&self) -> Result<HSTRING> {
                let mut path = HSTRING::new();
                unsafe {
                    (self.vtable().Path)(self.as_raw(), &mut path).ok()?;
                }
                Ok(path)
            }
        }
    };
}
result_class!(
    PickFileResult,
    "Microsoft.Windows.Storage.Pickers.PickFileResult"
);
result_class!(
    PickFolderResult,
    "Microsoft.Windows.Storage.Pickers.PickFolderResult"
);

fn operation<T: RuntimeType>(object: Object, method: GetObject) -> Result<IAsyncOperation<T>> {
    let mut value = std::ptr::null_mut();
    unsafe {
        method(object, &mut value).ok()?;
        IAsyncOperation::from_abi(value)
    }
}
fn set_string(object: Object, method: SetString, value: &str) -> Result<()> {
    let value = HSTRING::from(value);
    unsafe { method(object, std::mem::transmute_copy(&value)).ok() }
}
pub fn open_files(id: WindowId) -> Result<IAsyncOperation<IVectorView<PickFileResult>>> {
    let picker: IOpen = create::<OpenClass, IOpenFactory>(id)?.cast()?;
    let mut filter = std::ptr::null_mut();
    unsafe {
        (picker.vtable().FileTypeFilter)(picker.as_raw(), &mut filter).ok()?;
        let filter = IVector::<HSTRING>::from_abi(filter)?;
        for extension in [
            ".png", ".jpg", ".jpeg", ".bmp", ".gif", ".webp", ".mp4", ".mov", ".mkv", ".webm",
        ] {
            filter.Append(&HSTRING::from(extension))?;
        }
    }
    operation(picker.as_raw(), picker.vtable().PickMultipleFilesAsync)
}
pub fn open_folder(id: WindowId) -> Result<IAsyncOperation<PickFolderResult>> {
    let picker: IFolder = create::<FolderClass, IFolderFactory>(id)?.cast()?;
    operation(picker.as_raw(), picker.vtable().PickSingleFolderAsync)
}
pub fn save_file(id: WindowId, source: &Path) -> Result<IAsyncOperation<PickFileResult>> {
    let picker: ISave = create::<SaveClass, ISaveFactory>(id)?.cast()?;
    set_string(
        picker.as_raw(),
        picker.vtable().SetSuggestedFileName,
        &source.file_name().unwrap_or_default().to_string_lossy(),
    )?;
    if let Some(extension) = source.extension() {
        set_string(
            picker.as_raw(),
            picker.vtable().SetDefaultFileExtension,
            &format!(".{}", extension.to_string_lossy()),
        )?;
    }
    operation(picker.as_raw(), picker.vtable().PickSaveFileAsync)
}
