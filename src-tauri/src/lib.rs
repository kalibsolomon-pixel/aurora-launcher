mod application;
pub mod cache;
pub mod config;
pub mod distribution;
pub mod downloads;
pub mod fabric;
pub mod instances;
pub mod integrity;
pub mod minecraft;
pub mod paths;

#[cfg(test)]
mod test_support;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    eprintln!("[aurora-launcher] starting native backend");

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            application::get_application_status,
            application::get_launcher_state,
            application::acquire_artifact,
            application::plan_minecraft_install,
            application::plan_fabric_install
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Aurora Launcher");
}
