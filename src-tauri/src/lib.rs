mod application;
mod paths;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    eprintln!("[aurora-launcher] starting native backend");

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            application::get_application_status
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Aurora Launcher");
}
