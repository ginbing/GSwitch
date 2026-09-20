mod accounts;
mod app_server;
mod codex;
mod commands;
mod identity;
mod intake;
mod runtime;
mod storage;
mod switching;
mod types;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let store_path = app.path().app_config_dir()?.join("accounts.json");
            let state = accounts::AppState::new(store_path).map_err(std::io::Error::other)?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_runtime_info,
            commands::list_accounts,
            commands::start_oauth_login,
            commands::get_oauth_login_status,
            commands::import_auth_json,
            commands::import_api_key,
            commands::get_live_account_state,
            commands::save_current_account,
            commands::enable_account_switching,
            commands::switch_account,
            commands::recover_pending_switch,
            commands::remove_saved_account
        ])
        .run(tauri::generate_context!())
        .expect("error while running GSwitch");
}
