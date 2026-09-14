mod application;
pub mod config;
pub mod distribution;
pub mod instances;
pub mod paths;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    eprintln!("[aurora-launcher] starting native backend");

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            application::get_application_status,
            application::get_launcher_state
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Aurora Launcher");
}
