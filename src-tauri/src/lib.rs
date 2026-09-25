pub mod appearance;
mod application;
pub mod aurora;
pub mod auth;
pub mod cache;
pub mod config;
pub mod distribution;
pub mod downloads;
pub mod fabric;
pub mod install;
pub mod instance_content;
pub mod instance_mods;
pub mod instances;
pub mod integrity;
pub mod launch;
pub mod minecraft;
pub mod modrinth;
pub mod paths;
pub mod runtime;
pub mod shortcuts;

#[cfg(test)]
mod test_support;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    eprintln!("[aurora-launcher] starting native backend");

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            application::get_application_status,
            application::get_launcher_state,
            application::get_appearance,
            application::set_appearance,
            application::get_desktop_integration,
            application::create_desktop_shortcut,
            application::remove_desktop_shortcut,
            application::acquire_artifact,
            application::plan_minecraft_install,
            application::plan_fabric_install,
            application::install_game,
            application::validate_installed_game,
            application::list_aurora_releases,
            application::create_instance,
            application::retry_instance_install,
            application::rename_instance,
            application::update_instance_configuration,
            application::install_instance_configuration,
            application::list_minecraft_versions,
            application::list_fabric_loader_versions,
            application::select_instance,
            application::open_instance_folder,
            application::get_instance_mods,
            application::set_instance_mod_enabled,
            application::remove_instance_mod,
            application::open_instance_mods_folder,
            application::get_instance_content_context,
            application::get_instance_content,
            application::remove_instance_content,
            application::open_instance_content_folder,
            application::search_modrinth,
            application::get_modrinth_project,
            application::preview_modrinth_install,
            application::install_modrinth,
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
