use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use fs2::FileExt;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    storage::{self, AccountStore},
    types::{
        AccountIdentity, AccountKind, AccountView, OAuthLoginStatus, PendingSwitch, StoredAccount,
    },
};

#[derive(Clone)]
pub struct AppState {
    store_path: Arc<PathBuf>,
    recovery_dir: Arc<PathBuf>,
    store: Arc<Mutex<AccountStore>>,
    operation_lock: Arc<Mutex<()>>,
    oauth_logins: Arc<Mutex<HashMap<String, OAuthLoginStatus>>>,
}

pub struct OperationGuard<'a> {
    _in_process: MutexGuard<'a, ()>,
    lock_file: File,
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.lock_file);
    }
}

pub struct AccountDraft {
    pub label: Option<String>,
    pub default_label: String,
    pub kind: AccountKind,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub identity: AccountIdentity,
    pub credential: Value,
}

impl AppState {
    pub fn new(store_path: PathBuf) -> Result<Self, String> {
        let store = storage::load(&store_path)?;
        let recovery_dir = store_path
            .parent()
            .ok_or_else(|| "Invalid GSwitch storage path".to_string())?
            .join("pending-credentials");

        Ok(Self {
            store_path: Arc::new(store_path),
            recovery_dir: Arc::new(recovery_dir),
            store: Arc::new(Mutex::new(store)),
            operation_lock: Arc::new(Mutex::new(())),
            oauth_logins: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn list(&self) -> Result<Vec<AccountView>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        Ok(store
            .accounts
            .iter()
            .map(|account| Self::view(account, store.active_account_id.as_deref()))
            .collect())
    }

    pub fn account_by_identity(
        &self,
        identity: &AccountIdentity,
    ) -> Result<Option<AccountView>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        Ok(store
            .accounts
            .iter()
            .find(|account| account.identity.as_ref() == Some(identity))
            .map(|account| Self::view(account, store.active_account_id.as_deref())))
    }

    pub fn has_pending_switch(&self) -> Result<bool, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        Ok(store.pending_switch.is_some())
    }

    /// Codex 0.144.5 refuses a CODEX_HOME under the system temporary folder.
    /// Keep short-lived isolated profiles in GSwitch-owned application storage
    /// instead; each profile is still independently deleted after its task.
    pub fn isolated_profile_root(&self) -> Result<PathBuf, String> {
        self.store_path
            .parent()
            .map(|path| path.join("isolated-codex"))
            .ok_or_else(|| "Invalid GSwitch storage path".to_string())
    }

    /// Serializes any operation that can touch credentials across both GSwitch
    /// windows/processes. The file lock has no authority over Codex itself;
    /// switching performs its own external-runtime checks.
    pub fn acquire_operation(&self) -> Result<OperationGuard<'_>, String> {
        let in_process = self
            .operation_lock
            .try_lock()
            .map_err(|_| "Another GSwitch operation is already in progress".to_string())?;
        let parent = self
            .store_path
            .parent()
            .ok_or_else(|| "Invalid GSwitch storage path".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|_| "Unable to prepare GSwitch operation storage".to_string())?;

        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(parent.join("operations.lock"))
            .map_err(|_| "Unable to lock GSwitch operations".to_string())?;
        lock_file
            .try_lock_exclusive()
            .map_err(|_| "Another GSwitch operation is already in progress".to_string())?;

        Ok(OperationGuard {
            _in_process: in_process,
            lock_file,
        })
    }

    pub fn find_exact_document_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        kind: &AccountKind,
        identity: &AccountIdentity,
        credential: &Value,
    ) -> Result<Option<AccountView>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        Ok(store
            .accounts
            .iter()
            .find(|account| {
                account.kind == *kind
                    && account.identity.as_ref() == Some(identity)
                    && account.credential == *credential
            })
            .map(|account| Self::view(account, store.active_account_id.as_deref())))
    }

    pub fn upsert_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        draft: AccountDraft,
    ) -> Result<AccountView, String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;

        let existing_index = store.accounts.iter().position(|existing| {
            existing.kind == draft.kind
                && (existing.identity.as_ref() == Some(&draft.identity)
                    || (existing.identity.is_none() && existing.credential == draft.credential))
        });

        let account = if let Some(index) = existing_index {
            let existing = &store.accounts[index];
            StoredAccount {
                id: existing.id.clone(),
                label: draft.label.unwrap_or_else(|| existing.label.clone()),
                kind: draft.kind,
                email: draft.email,
                plan_type: draft.plan_type,
                identity: Some(draft.identity),
                credential: draft.credential,
            }
        } else {
            StoredAccount {
                id: Uuid::new_v4().to_string(),
                label: draft.label.unwrap_or(draft.default_label),
                kind: draft.kind,
                email: draft.email,
                plan_type: draft.plan_type,
                identity: Some(draft.identity),
                credential: draft.credential,
            }
        };

        let mut candidate = store.clone();
        if let Some(index) = existing_index {
            candidate.accounts[index] = account.clone();
        } else {
            candidate.accounts.push(account.clone());
        }
        storage::save_atomic(&self.store_path, &candidate)?;
        let active_account_id = candidate.active_account_id.clone();
        *store = candidate;
        Ok(Self::view(&account, active_account_id.as_deref()))
    }

    pub fn account_by_id_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        id: &str,
    ) -> Result<StoredAccount, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        store
            .accounts
            .iter()
            .find(|account| account.id == id)
            .cloned()
            .ok_or_else(|| "The selected account is no longer saved".to_string())
    }

    pub fn account_by_identity_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        identity: &AccountIdentity,
    ) -> Result<Option<StoredAccount>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        Ok(store
            .accounts
            .iter()
            .find(|account| account.identity.as_ref() == Some(identity))
            .cloned())
    }

    pub fn update_credential_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        id: &str,
        credential: Value,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        let index = store
            .accounts
            .iter()
            .position(|account| account.id == id)
            .ok_or_else(|| "The selected account is no longer saved".to_string())?;
        let mut candidate = store.clone();
        candidate.accounts[index].credential = credential;
        storage::save_atomic(&self.store_path, &candidate)?;
        *store = candidate;
        Ok(())
    }

    pub fn prepare_switch_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        pending_switch: PendingSwitch,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        if store.pending_switch.is_some() {
            return Err(
                "GSwitch must recover a previous switch before starting another".to_string(),
            );
        }
        let mut candidate = store.clone();
        candidate.pending_switch = Some(pending_switch);
        storage::save_atomic(&self.store_path, &candidate)?;
        *store = candidate;
        Ok(())
    }

    pub fn pending_switch_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
    ) -> Result<Option<PendingSwitch>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        Ok(store.pending_switch.clone())
    }

    pub fn active_account_id_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
    ) -> Result<Option<String>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        Ok(store.active_account_id.clone())
    }

    pub fn mark_switch_verified_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        let mut candidate = store.clone();
        let pending = candidate
            .pending_switch
            .as_mut()
            .ok_or_else(|| "No GSwitch switch transaction is pending".to_string())?;
        pending.stage = crate::types::PendingSwitchStage::Verified;
        storage::save_atomic(&self.store_path, &candidate)?;
        *store = candidate;
        Ok(())
    }

    pub fn complete_switch_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        id: &str,
    ) -> Result<AccountView, String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        let account = store
            .accounts
            .iter()
            .find(|account| account.id == id)
            .cloned()
            .ok_or_else(|| "The selected account is no longer saved".to_string())?;
        let mut candidate = store.clone();
        candidate.active_account_id = Some(id.to_string());
        candidate.pending_switch = None;
        storage::save_atomic(&self.store_path, &candidate)?;
        *store = candidate;
        Ok(Self::view(&account, Some(id)))
    }

    pub fn clear_pending_switch_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        active_account_id: Option<String>,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        let mut candidate = store.clone();
        candidate.active_account_id = active_account_id;
        candidate.pending_switch = None;
        storage::save_atomic(&self.store_path, &candidate)?;
        *store = candidate;
        Ok(())
    }

    pub fn remove_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        id: &str,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        if store.active_account_id.as_deref() == Some(id) {
            return Err("The active Codex account cannot be removed".to_string());
        }
        let mut candidate = store.clone();
        let before = candidate.accounts.len();
        candidate.accounts.retain(|account| account.id != id);
        if candidate.accounts.len() == before {
            return Err("The selected account is no longer saved".to_string());
        }
        storage::save_atomic(&self.store_path, &candidate)?;
        *store = candidate;
        Ok(())
    }

    /// Persists a refreshed credential independently of the main account store
    /// when the latter cannot be updated. This is deliberately a small recovery
    /// queue, not a general backup service.
    pub fn record_pending_credential(
        &self,
        _operation: &OperationGuard<'_>,
        credential: &Value,
    ) -> Result<(), String> {
        let path = self.recovery_dir.join(format!("{}.json", Uuid::new_v4()));
        storage::write_json_atomic(
            &path,
            &json!({"version": 1, "credential": credential}),
            "pending credential recovery",
        )
    }

    pub fn set_oauth_status(
        &self,
        login_id: String,
        status: OAuthLoginStatus,
    ) -> Result<(), String> {
        let mut sessions = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is unavailable".to_string())?;
        sessions.insert(login_id, status);
        Ok(())
    }

    pub fn oauth_status(&self, login_id: &str) -> Result<OAuthLoginStatus, String> {
        let sessions = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is unavailable".to_string())?;

        sessions
            .get(login_id)
            .cloned()
            .ok_or_else(|| "Unknown OAuth login session".to_string())
    }

    fn view(account: &StoredAccount, active_account_id: Option<&str>) -> AccountView {
        AccountView {
            id: account.id.clone(),
            label: account.label.clone(),
            kind: account.kind.clone(),
            email: account.email.clone(),
            plan_type: account.plan_type.clone(),
            active: active_account_id == Some(account.id.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "gswitch-accounts-{name}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ))
            .join("accounts.json")
    }

    fn draft(workspace: &str, credential: Value) -> AccountDraft {
        AccountDraft {
            label: None,
            default_label: "User@example.com".into(),
            kind: AccountKind::ChatGpt,
            email: Some("User@example.com".into()),
            plan_type: Some("plus".into()),
            identity: AccountIdentity::ChatGpt {
                user_id: "user".into(),
                workspace_id: Some(workspace.into()),
            },
            credential,
        }
    }

    fn save(state: &AppState, draft: AccountDraft) -> AccountView {
        let operation = state.acquire_operation().expect("operation");
        state
            .upsert_under_operation(&operation, draft)
            .expect("save account")
    }

    #[test]
    fn allows_the_same_email_in_different_workspaces() {
        let path = temp_path("different-workspaces");
        let state = AppState::new(path.clone()).expect("state");

        save(&state, draft("workspace-a", json!({"token": "first"})));
        save(&state, draft("workspace-b", json!({"token": "second"})));

        assert_eq!(state.list().expect("list").len(), 2);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn reauthentication_updates_the_existing_profile() {
        let path = temp_path("reauthentication");
        let state = AppState::new(path.clone()).expect("state");

        let first = save(&state, draft("workspace", json!({"token": "old"})));
        let second = save(&state, draft("workspace", json!({"token": "rotated"})));

        assert_eq!(first.id, second.id);
        assert_eq!(state.list().expect("list").len(), 1);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_second_operation_is_rejected() {
        let path = temp_path("operation-lock");
        let state = AppState::new(path.clone()).expect("state");
        let first = state.acquire_operation().expect("first operation");

        match state.acquire_operation() {
            Ok(_) => panic!("must reject overlap"),
            Err(error) => assert_eq!(error, "Another GSwitch operation is already in progress"),
        }
        drop(first);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn failed_persistence_does_not_change_in_memory_store() {
        let path = temp_path("failed-save");
        let parent = path.parent().expect("parent");
        let state = AppState::new(path.clone()).expect("state");
        fs::write(parent, b"blocks directory creation").expect("write blocking file");

        let operation = state.acquire_operation();
        assert!(operation.is_err());
        assert!(state.list().expect("list").is_empty());
        let _ = fs::remove_file(parent);
    }

    #[test]
    fn completing_a_switch_marks_only_the_verified_target_active() {
        let path = temp_path("complete-switch");
        let state = AppState::new(path.clone()).expect("state");
        let first = save(&state, draft("workspace-a", json!({"token": "first"})));
        let second = save(&state, draft("workspace-b", json!({"token": "second"})));

        let operation = state.acquire_operation().expect("operation");
        state
            .prepare_switch_under_operation(
                &operation,
                PendingSwitch {
                    target_id: second.id.clone(),
                    target_identity: AccountIdentity::ChatGpt {
                        user_id: "user".into(),
                        workspace_id: Some("workspace-b".into()),
                    },
                    previous_active_id: Some(first.id.clone()),
                    previous_auth: Some(json!({"token": "first"})),
                    stage: crate::types::PendingSwitchStage::Prepared,
                },
            )
            .expect("prepare switch");
        let completed = state
            .complete_switch_under_operation(&operation, &second.id)
            .expect("complete switch");

        assert!(completed.active);
        assert!(!state.has_pending_switch().expect("pending"));
        let accounts = state.list().expect("accounts");
        assert!(accounts
            .iter()
            .any(|account| account.id == second.id && account.active));
        assert!(accounts
            .iter()
            .any(|account| account.id == first.id && !account.active));
        assert_eq!(
            state
                .remove_under_operation(&operation, &second.id)
                .expect_err("active account"),
            "The active Codex account cannot be removed"
        );
        drop(operation);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }
}
