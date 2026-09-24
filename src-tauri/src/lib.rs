mod accounts;
mod app_server;
mod chatgpt;
mod cli_update;
mod codex;
mod commands;
mod identity;
mod intake;
mod migration;
mod quota;
mod runtime;
mod storage;
mod switching;
mod types;
mod vault;
mod wake;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let store_path = app.path().app_config_dir()?.join("accounts.json");
            let state = accounts::AppState::new(store_path).map_err(std::io::Error::other)?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_runtime_info,
            commands::get_codex_cli_info,
            commands::update_codex_cli,
            commands::open_codex_cli_guide,
            commands::get_app_snapshot,
            commands::list_accounts,
            commands::reset_damaged_account_store,
            commands::recover_pending_credentials,
            commands::start_oauth_login,
            commands::get_oauth_login_status,
            commands::cancel_oauth_login,
            commands::open_oauth_login,
            commands::get_update_delivery,
            commands::open_latest_release,
            commands::import_auth_json,
            commands::import_auth_file,
            commands::import_auth_files,
            commands::export_accounts,
            commands::discover_local_accounts,
            commands::import_local_accounts,
            commands::import_api_key,
            commands::get_live_account_state,
            commands::save_current_account,
            commands::enable_account_switching,
            commands::switch_account,
            commands::recover_pending_switch,
            commands::remove_saved_account,
            commands::get_account_quota,
            commands::refresh_account_quota,
            commands::redeem_earliest_reset_credit,
            commands::recover_pending_reset_credit,
            commands::start_wake,
            commands::start_wake_all,
            commands::start_wake_selected,
            commands::get_wake_operation,
            commands::cancel_wake
        ])
        .run(tauri::generate_context!())
        .expect("error while running GSwitch");
}
