use tauri::State;

use crate::{
    accounts::AppState,
    codex, intake,
    types::{AccountView, OAuthLoginStart, OAuthLoginStatus, RuntimeInfo},
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
