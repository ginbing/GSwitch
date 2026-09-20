use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::{
    accounts::AppState,
    codex, intake, quota, switching,
    types::{
        AccountView, ImportResult, LiveAccountView, OAuthLoginStart, OAuthLoginStatus, QuotaView,
        ResetCreditOutcome, RuntimeInfo, SwitchOutcome, WakeOperationView, WakeStart,
    },
    wake,
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
pub fn cancel_oauth_login(state: State<'_, AppState>, login_id: String) -> Result<(), String> {
    intake::cancel_oauth(state.inner(), &login_id)
}

#[tauri::command]
pub fn open_oauth_login(
    app: AppHandle,
    state: State<'_, AppState>,
    login_id: String,
) -> Result<(), String> {
    let url = state.oauth_url(&login_id)?;
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|_| "Unable to open the OAuth URL in your browser".to_string())
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
pub fn import_auth_file(state: State<'_, AppState>, path: String) -> Result<ImportResult, String> {
    intake::import_file(state.inner(), &path)
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

#[tauri::command]
pub fn get_account_quota(state: State<'_, AppState>, id: String) -> Result<QuotaView, String> {
    quota::cached_quota(state.inner(), &id)
}

#[tauri::command]
pub fn refresh_account_quota(state: State<'_, AppState>, id: String) -> Result<QuotaView, String> {
    quota::refresh_quota(state.inner(), &id)
}

#[tauri::command]
pub fn redeem_earliest_reset_credit(
    state: State<'_, AppState>,
    id: String,
) -> Result<ResetCreditOutcome, String> {
    quota::redeem_earliest_reset_credit(state.inner(), &id)
}

#[tauri::command]
pub fn recover_pending_reset_credit(
    state: State<'_, AppState>,
) -> Result<ResetCreditOutcome, String> {
    quota::recover_pending_reset_credit(state.inner())
}

#[tauri::command]
pub fn start_wake(
    state: State<'_, AppState>,
    id: String,
    model: Option<String>,
) -> Result<WakeStart, String> {
    wake::start_one(state.inner().clone(), id, model)
}

#[tauri::command]
pub fn start_wake_all(state: State<'_, AppState>) -> Result<WakeStart, String> {
    wake::start_all(state.inner().clone())
}

#[tauri::command]
pub fn get_wake_operation(
    state: State<'_, AppState>,
    operation_id: String,
) -> Result<WakeOperationView, String> {
    wake::operation(state.inner(), &operation_id)
}

#[tauri::command]
pub fn cancel_wake(state: State<'_, AppState>, operation_id: String) -> Result<(), String> {
    wake::cancel(state.inner(), &operation_id)
}
