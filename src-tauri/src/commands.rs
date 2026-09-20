use tauri::State;

use crate::{
    accounts::AppState,
    codex,
    types::{AccountView, RuntimeInfo},
};

#[tauri::command]
pub fn get_runtime_info() -> Result<RuntimeInfo, String> {
    codex::runtime_info()
}

#[tauri::command]
pub fn list_accounts(state: State<'_, AppState>) -> Result<Vec<AccountView>, String> {
    state.list()
}
