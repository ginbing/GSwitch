use tauri::State;

use crate::{
    accounts::AppState,
    codex, intake, switching,
    types::{
        AccountView, LiveAccountView, OAuthLoginStart, OAuthLoginStatus, RuntimeInfo, SwitchOutcome,
    },
};

#[tauri::command]
pub fn get_runtime_info() -> Result<RuntimeInfo, String> {
    codex::runtime_info()
}

#[tauri::command]
pub fn list_accounts(state: State<'_, AppState>) -> Result<Vec<AccountView>, String> {
    state.list()
}

#[tauri::command]
pub fn start_oauth_login(state: State<'_, AppState>) -> Result<OAuthLoginStart, String> {
    intake::start_oauth(state.inner().clone())
}

#[tauri::command]
pub fn get_oauth_login_status(
    state: State<'_, AppState>,
    login_id: String,
) -> Result<OAuthLoginStatus, String> {
    state.oauth_status(&login_id)
}

#[tauri::command]
pub fn import_auth_json(
    state: State<'_, AppState>,
    raw_json: String,
    label: Option<String>,
) -> Result<AccountView, String> {
    intake::import_json(state.inner(), &raw_json, label)
}

#[tauri::command]
pub fn import_api_key(
    state: State<'_, AppState>,
    api_key: String,
    label: Option<String>,
) -> Result<AccountView, String> {
    intake::import_api_key(state.inner(), &api_key, label)
}

#[tauri::command]
pub fn get_live_account_state(state: State<'_, AppState>) -> Result<LiveAccountView, String> {
    switching::live_account(state.inner())
}

#[tauri::command]
pub fn save_current_account(state: State<'_, AppState>) -> Result<AccountView, String> {
    switching::save_current_account(state.inner())
}

#[tauri::command]
pub fn enable_account_switching(state: State<'_, AppState>) -> Result<bool, String> {
    switching::enable_file_store(state.inner())
}

#[tauri::command]
pub fn switch_account(
    state: State<'_, AppState>,
    target_id: String,
) -> Result<SwitchOutcome, String> {
    switching::switch_account(state.inner(), &target_id)
}

#[tauri::command]
pub fn recover_pending_switch(state: State<'_, AppState>) -> Result<(), String> {
    switching::recover_pending_switch(state.inner())
}

#[tauri::command]
pub fn remove_saved_account(state: State<'_, AppState>, id: String) -> Result<(), String> {
    switching::remove_saved_account(state.inner(), &id)
}
