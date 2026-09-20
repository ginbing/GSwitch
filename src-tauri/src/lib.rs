mod accounts;
mod app_server;
mod codex;
mod commands;
mod intake;
mod storage;
mod types;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let store_path = app.path().app_config_dir()?.join("accounts.json");
            let state = accounts::AppState::new(store_path)
                .map_err(|error| std::io::Error::other(error))?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_runtime_info,
            commands::list_accounts,
            commands::start_oauth_login,
            commands::get_oauth_login_status,
            commands::import_auth_json,
            commands::import_api_key
        ])
        .run(tauri::generate_context!())
        .expect("error while running GSwitch");
}
