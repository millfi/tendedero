#![windows_subsystem = "windows"]
mod app;
mod capture;
mod clothesline;
mod editor;
mod model;
mod platform;
mod video;

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let root = args
        .iter()
        .position(|a| a == "--data-dir")
        .and_then(|n| args.get(n + 1))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default())
                .join("Snapline")
        });
    if let Err(error) = model::Store::load(root.clone()) {
        show_error(&format!("保存データを開けません: {error}"));
        return;
    }
    let _instance = match platform::single_instance(&root) {
        Ok(Some(instance)) => instance,
        Ok(None) => return,
        Err(e) => {
            show_error(&e.to_string());
            return;
        }
    };
    let log = root.join("startup.log");
    let _ = std::fs::write(&log, "Starting WinUI\n");
    let panic_log = log.clone();
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::fs::write(&panic_log, info.to_string());
    }));
    unsafe {
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }
    if let Err(error) = windows_reactor::App::run_component::<app::Shelf>(root) {
        let _ = std::fs::write(log, error.to_string());
        show_error(&error.to_string());
    }
    unsafe {
        windows::Win32::System::Ole::OleUninitialize();
    }
}
fn show_error(message: &str) {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
            None,
            windows::core::PCWSTR(platform::wide(message).as_ptr()),
            windows::core::w!("Snapline"),
            windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
        );
    }
}
