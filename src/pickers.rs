//! Windows App SDK path-based pickers. Start on the UI thread, wait on a
//! Reactor background task, and deliver only paths / errors back to Shelf.
mod abi;

use abi::*;
use std::path::{Path, PathBuf};
use windows::{
    Win32::{Foundation::HWND, System::WinRT::*},
    core::*,
};
use windows_collections::IVectorView;
use windows_future::IAsyncOperation;

pub enum Request {
    Import,
    Folder,
    Save(PathBuf),
}

#[derive(Clone)]
pub enum Outcome {
    Import(Vec<PathBuf>),
    Folder(Option<PathBuf>),
    Saved(bool),
}

pub enum Pending {
    Import(IAsyncOperation<IVectorView<PickFileResult>>),
    Folder(IAsyncOperation<PickFolderResult>),
    Save(IAsyncOperation<PickFileResult>, PathBuf),
}

pub fn start(owner: HWND, request: Request) -> Result<Pending> {
    let window_id = abi::window_id(owner)?;
    match request {
        Request::Import => Ok(Pending::Import(abi::open_files(window_id)?)),
        Request::Folder => Ok(Pending::Folder(abi::open_folder(window_id)?)),
        Request::Save(source) => Ok(Pending::Save(abi::save_file(window_id, &source)?, source)),
    }
}

// WinRT represents a cancelled picker as S_OK + a null object. windows-core
// reports that null as Error::empty() (code S_OK), not a failing HRESULT.
fn optional<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code().is_ok() => Ok(None),
        Err(error) => Err(error),
    }
}

impl Pending {
    pub fn complete(self) -> std::result::Result<Outcome, String> {
        // The thread-pool apartment belongs to this call; balance S_OK and
        // S_FALSE with RoUninitialize. No blocking wait runs on the UI thread.
        let run = || -> crate::model::AppResult<Outcome> {
            unsafe {
                RoInitialize(RO_INIT_MULTITHREADED)?;
            }
            struct Apartment;
            impl Drop for Apartment {
                fn drop(&mut self) {
                    unsafe {
                        RoUninitialize();
                    }
                }
            }
            let _apartment = Apartment;
            Ok(match self {
                Self::Import(op) => {
                    let mut paths = Vec::new();
                    if let Some(files) = optional(op.join())? {
                        for file in files {
                            paths.push(PathBuf::from(file.path()?.to_string()));
                        }
                    }
                    Outcome::Import(paths)
                }
                Self::Folder(op) => Outcome::Folder(
                    optional(op.join())?
                        .map(|folder| folder.path().map(|p| PathBuf::from(p.to_string())))
                        .transpose()?,
                ),
                Self::Save(op, source) => {
                    if let Some(file) = optional(op.join())? {
                        copy_to(&source, Path::new(&file.path()?.to_string()))?;
                        Outcome::Saved(true)
                    } else {
                        Outcome::Saved(false)
                    }
                }
            })
        };
        run().map_err(|error| error.to_string())
    }
}

fn copy_to(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source == destination
        || source
            .canonicalize()
            .ok()
            .zip(destination.canonicalize().ok())
            .is_some_and(|(from, to)| from == to)
    {
        return Ok(());
    }
    std::fs::copy(source, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picker_cancel_is_distinct_from_failure() {
        assert_eq!(optional::<u32>(Err(Error::empty())).unwrap(), None);
        assert_eq!(optional(Ok(42)).unwrap(), Some(42));
        let error = Error::from_hresult(HRESULT(0x80070005u32 as i32));
        assert_eq!(
            optional::<u32>(Err(error)).unwrap_err().code(),
            HRESULT(0x80070005u32 as i32)
        );
    }
    #[test]
    fn saving_to_the_source_does_not_truncate_it() {
        let root = std::env::temp_dir().join(format!("snapline-picker-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.png");
        let output = root.join("output.png");
        std::fs::write(&source, b"original image").unwrap();
        copy_to(&source, &root.join(".").join("source.png")).unwrap();
        copy_to(&source, &output).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), b"original image");
        assert_eq!(std::fs::read(&output).unwrap(), b"original image");
        std::fs::remove_dir_all(root).unwrap();
    }
}
