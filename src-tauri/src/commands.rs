use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::{
    accounts::AppState,
    codex, intake, migration, quota, switching,
    types::{
        AccountView, AppSnapshot, ExportResult, ImportResult, LiveAccountView, MigrationPreview,
        OAuthLoginStart, OAuthLoginStatus, QuotaView, ResetCreditOutcome, RuntimeInfo,
        StorageStatus, SwitchFailure, SwitchFailureCode, SwitchOutcome, UpdateDelivery,
        WakeOperationView, WakeStart,
    },
    wake,
};

/// Keeps Tauri's command runtime available for the WebView while the existing
/// synchronous domain layer performs filesystem, provider, process, or App
/// Server work. Credential-operation serialization remains owned by AppState.
async fn run_blocking<T>(
    task: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String>
where
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|_| "A GSwitch background operation stopped unexpectedly".to_string())?
}

#[tauri::command]
pub async fn get_runtime_info() -> Result<RuntimeInfo, String> {
    run_blocking(codex::runtime_info).await
}

/// Supplies one credential-free initial workspace projection. When GSwitch's
/// own account store is damaged, this deliberately
/// avoids touching Codex configuration or credentials so the recovery screen
/// can still open safely.
#[tauri::command]
pub async fn get_app_snapshot(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    let state = state.inner().clone();
    run_blocking(move || app_snapshot(&state)).await
}

fn app_snapshot(state: &AppState) -> Result<AppSnapshot, String> {
    let storage = state.storage_view();
    if storage.status == StorageStatus::RecoveryRequired {
        return Ok(AppSnapshot {
            storage,
            accounts: Vec::new(),
            pending_reset_credit: false,
            runtime: None,
            live: None,
        });
    }

    Ok(AppSnapshot {
        storage,
        accounts: state.list()?,
        pending_reset_credit: state.has_pending_reset_credit()?,
        runtime: Some(codex::runtime_info()?),
        live: Some(switching::live_account(state)?),
    })
}

#[tauri::command]
pub async fn list_accounts(state: State<'_, AppState>) -> Result<Vec<AccountView>, String> {
    let state = state.inner().clone();
    run_blocking(move || state.list()).await
}

#[tauri::command]
pub async fn reset_damaged_account_store(state: State<'_, AppState>) -> Result<(), String> {
    let state = state.inner().clone();
    run_blocking(move || state.reset_damaged_store()).await
}

#[tauri::command]
pub async fn start_oauth_login(state: State<'_, AppState>) -> Result<OAuthLoginStart, String> {
    let state = state.inner().clone();
    run_blocking(move || intake::start_oauth(state)).await
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
pub fn get_update_delivery() -> UpdateDelivery {
    update_delivery_for(std::env::consts::OS, std::env::var_os("APPIMAGE").is_some())
}

#[tauri::command]
pub fn open_latest_release(app: AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(
            "https://github.com/ginbing/GSwitch/releases/latest",
            None::<&str>,
        )
        .map_err(|_| "Unable to open the GSwitch release page in your browser".to_string())
}

fn update_delivery_for(os: &str, appimage: bool) -> UpdateDelivery {
    match os {
        "windows" => UpdateDelivery::InstallerExits,
        "macos" => UpdateDelivery::RelaunchRequired,
        "linux" if appimage => UpdateDelivery::RelaunchRequired,
        "linux" => UpdateDelivery::ReleaseDownload,
        _ => UpdateDelivery::ReleaseDownload,
    }
}

#[tauri::command]
pub async fn import_auth_json(
    state: State<'_, AppState>,
    raw_json: String,
    label: Option<String>,
) -> Result<AccountView, String> {
    let state = state.inner().clone();
    run_blocking(move || intake::import_json(&state, &raw_json, label)).await
}

#[tauri::command]
pub async fn import_auth_file(
    state: State<'_, AppState>,
    path: String,
) -> Result<ImportResult, String> {
    let state = state.inner().clone();
    run_blocking(move || intake::import_file(&state, &path)).await
}

#[tauri::command]
pub async fn import_auth_files(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<ImportResult, String> {
    let state = state.inner().clone();
    run_blocking(move || intake::import_files(&state, paths)).await
}

#[tauri::command]
pub async fn export_accounts(
    app: AppHandle,
    state: State<'_, AppState>,
    selected_ids: Vec<String>,
) -> Result<ExportResult, String> {
    let state = state.inner().clone();
    let export =
        run_blocking(move || intake::prepare_accounts_export(&state, selected_ids)).await?;
    let destination = app
        .dialog()
        .file()
        .set_title("Export GSwitch accounts")
        .set_file_name("gswitch-accounts.json")
        .add_filter("JSON", &["json"])
        .blocking_save_file()
        .map(|destination| {
            destination
                .into_path()
                .map_err(|_| "Unable to use the selected export location".to_string())
        })
        .transpose()?;
    run_blocking(move || intake::complete_accounts_export(&export, destination.as_deref())).await
}

#[tauri::command]
pub async fn discover_local_accounts(
    state: State<'_, AppState>,
    custom_root: Option<String>,
) -> Result<MigrationPreview, String> {
    let state = state.inner().clone();
    run_blocking(move || migration::discover(&state, custom_root)).await
}

#[tauri::command]
pub async fn import_local_accounts(
    state: State<'_, AppState>,
    custom_root: Option<String>,
    selected_ids: Vec<String>,
) -> Result<ImportResult, String> {
    let state = state.inner().clone();
    run_blocking(move || migration::confirm(&state, custom_root, selected_ids)).await
}

#[tauri::command]
pub async fn import_api_key(
    state: State<'_, AppState>,
    api_key: String,
    label: Option<String>,
) -> Result<AccountView, String> {
    let state = state.inner().clone();
    run_blocking(move || intake::import_api_key(&state, &api_key, label)).await
}

#[tauri::command]
pub async fn get_live_account_state(state: State<'_, AppState>) -> Result<LiveAccountView, String> {
    let state = state.inner().clone();
    run_blocking(move || switching::live_account(&state)).await
}

#[tauri::command]
pub async fn save_current_account(state: State<'_, AppState>) -> Result<AccountView, String> {
    let state = state.inner().clone();
    run_blocking(move || switching::save_current_account(&state)).await
}

#[tauri::command]
pub async fn enable_account_switching(state: State<'_, AppState>) -> Result<bool, String> {
    let state = state.inner().clone();
    run_blocking(move || switching::enable_file_store(&state)).await
}

#[tauri::command]
pub async fn switch_account(
    state: State<'_, AppState>,
    target_id: String,
) -> Result<SwitchOutcome, SwitchFailure> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || switching::switch_account(&state, &target_id))
        .await
        .map_err(|_| SwitchFailure {
            code: SwitchFailureCode::VerificationFailed,
        })?
}

#[tauri::command]
pub async fn recover_pending_switch(state: State<'_, AppState>) -> Result<(), String> {
    let state = state.inner().clone();
    run_blocking(move || switching::recover_pending_switch(&state)).await
}

#[tauri::command]
pub async fn remove_saved_account(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let state = state.inner().clone();
    run_blocking(move || switching::remove_saved_account(&state, &id)).await
}

#[tauri::command]
pub async fn get_account_quota(
    state: State<'_, AppState>,
    id: String,
) -> Result<QuotaView, String> {
    let state = state.inner().clone();
    run_blocking(move || quota::cached_quota(&state, &id)).await
}

#[tauri::command]
pub async fn refresh_account_quota(
    state: State<'_, AppState>,
    id: String,
) -> Result<QuotaView, String> {
    let state = state.inner().clone();
    run_blocking(move || quota::refresh_quota(&state, &id)).await
}

#[tauri::command]
pub async fn redeem_earliest_reset_credit(
    state: State<'_, AppState>,
    id: String,
) -> Result<ResetCreditOutcome, String> {
    let state = state.inner().clone();
    run_blocking(move || quota::redeem_earliest_reset_credit(&state, &id)).await
}

#[tauri::command]
pub async fn recover_pending_reset_credit(
    state: State<'_, AppState>,
) -> Result<ResetCreditOutcome, String> {
    let state = state.inner().clone();
    run_blocking(move || quota::recover_pending_reset_credit(&state)).await
}

#[tauri::command]
pub fn start_wake(state: State<'_, AppState>, id: String) -> Result<WakeStart, String> {
    wake::start_one(state.inner().clone(), id)
}

#[tauri::command]
pub fn start_wake_all(state: State<'_, AppState>) -> Result<WakeStart, String> {
    wake::start_all(state.inner().clone())
}

#[tauri::command]
pub fn start_wake_selected(
    state: State<'_, AppState>,
    selected_ids: Vec<String>,
) -> Result<WakeStart, String> {
    wake::start_selected(state.inner().clone(), selected_ids)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Duration};

    #[test]
    fn delayed_domain_work_runs_away_from_the_command_thread() {
        let command_thread = thread::current().id();
        let worker_thread = tauri::async_runtime::block_on(run_blocking(|| {
            thread::sleep(Duration::from_millis(10));
            Ok(thread::current().id())
        }))
        .expect("background task should complete");

        assert_ne!(worker_thread, command_thread);
    }

    #[test]
    fn update_delivery_requires_a_manual_download_for_debian_packages() {
        assert_eq!(
            update_delivery_for("linux", false),
            UpdateDelivery::ReleaseDownload
        );
        assert_eq!(
            update_delivery_for("linux", true),
            UpdateDelivery::RelaunchRequired
        );
        assert_eq!(
            update_delivery_for("windows", false),
            UpdateDelivery::InstallerExits
        );
        assert_eq!(
            update_delivery_for("macos", false),
            UpdateDelivery::RelaunchRequired
        );
    }
}
