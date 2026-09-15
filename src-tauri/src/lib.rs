mod application;
pub mod aurora;
pub mod auth;
pub mod cache;
pub mod config;
pub mod distribution;
pub mod downloads;
pub mod fabric;
pub mod install;
pub mod instances;
pub mod integrity;
pub mod launch;
pub mod minecraft;
pub mod paths;
pub mod runtime;

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
            application::plan_fabric_install,
            application::install_game,
            application::validate_installed_game,
            application::list_aurora_releases,
            application::create_instance,
            application::retry_instance_install,
            application::rename_instance,
            application::select_instance,
            application::validate_instance,
            application::get_instance_runtime_status,
            application::ensure_instance_runtime,
            application::get_accounts,
            application::begin_microsoft_login,
            application::cancel_microsoft_login,
            application::select_account,
            application::remove_account,
            application::refresh_account_session,
            application::get_play_readiness,
            application::get_launch_state,
            application::play_instance
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Aurora Launcher");
}
