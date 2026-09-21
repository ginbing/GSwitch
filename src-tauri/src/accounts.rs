use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use fs2::FileExt;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    storage::{self, AccountStore},
    types::{
        AccountIdentity, AccountKind, AccountView, OAuthLoginStatus, PendingResetCredit,
        PendingSwitch, StorageStatus, StorageView, StoredAccount, StoredResetCredits,
        WakeOperationStatus, WakeOperationView,
    },
};

#[derive(Clone)]
pub struct AppState {
    store_path: Arc<PathBuf>,
    recovery_dir: Arc<PathBuf>,
    store_recovery_dir: Arc<PathBuf>,
    store: Arc<Mutex<AccountStore>>,
    store_recovery_required: Arc<AtomicBool>,
    operation_lock: Arc<Mutex<()>>,
    oauth_logins: Arc<Mutex<HashMap<String, OAuthLoginControl>>>,
    wake_operations: Arc<Mutex<HashMap<String, WakeControl>>>,
}

#[derive(Clone)]
struct OAuthLoginControl {
    status: OAuthLoginStatus,
    auth_url: String,
    cancelled: Arc<AtomicBool>,
}

#[derive(Clone)]
struct WakeControl {
    view: WakeOperationView,
    cancelled: Arc<AtomicBool>,
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
        let storage_root = store_path
            .parent()
            .ok_or_else(|| "Invalid GSwitch storage path".to_string())?
            .to_path_buf();
        let (store, store_recovery_required) = match storage::load(&store_path) {
            Ok(store) => (store, false),
            // Do not prevent the desktop application from opening because
            // GSwitch's own account file is unreadable. The replacement
            // default is intentionally unusable until the user confirms a
            // recovery reset; it can never authorize a credential mutation.
            Err(_) => (AccountStore::default(), true),
        };

        Ok(Self {
            store_path: Arc::new(store_path),
            recovery_dir: Arc::new(storage_root.join("pending-credentials")),
            store_recovery_dir: Arc::new(storage_root.join("damaged-account-stores")),
            store: Arc::new(Mutex::new(store)),
            store_recovery_required: Arc::new(AtomicBool::new(store_recovery_required)),
            operation_lock: Arc::new(Mutex::new(())),
            oauth_logins: Arc::new(Mutex::new(HashMap::new())),
            wake_operations: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn storage_view(&self) -> StorageView {
        if self.store_recovery_required.load(Ordering::SeqCst) {
            StorageView {
                status: StorageStatus::RecoveryRequired,
                message: Some(
                    "GSwitch could not safely read its saved account library. Codex credentials were not changed."
                        .to_string(),
                ),
            }
        } else {
            StorageView {
                status: StorageStatus::Ready,
                message: None,
            }
        }
    }

    pub fn recovery_required(&self) -> bool {
        self.store_recovery_required.load(Ordering::SeqCst)
    }

    pub fn ensure_store_ready(&self) -> Result<(), String> {
        if self.recovery_required() {
            return Err(
                "GSwitch account storage needs recovery before it can make changes".to_string(),
            );
        }
        Ok(())
    }

    /// Resets only GSwitch-owned account storage after an explicit user
    /// confirmation. The unreadable source is moved to a private recovery
    /// directory before a fresh empty store is written. No Codex file is read,
    /// deleted, or replaced by this operation.
    pub fn reset_damaged_store(&self) -> Result<(), String> {
        if !self.recovery_required() {
            return Err("GSwitch account storage does not need recovery".to_string());
        }
        let _operation = self.acquire_recovery_operation()?;
        if !self.recovery_required() {
            return Err("GSwitch account storage does not need recovery".to_string());
        }

        storage::preserve_damaged_store(&self.store_path, &self.store_recovery_dir)?;
        let fresh_store = AccountStore::default();
        storage::save_atomic(&self.store_path, &fresh_store)?;

        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        *store = fresh_store;
        self.store_recovery_required.store(false, Ordering::SeqCst);
        Ok(())
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

    pub fn account_by_id(&self, id: &str) -> Result<StoredAccount, String> {
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

    /// Exposes only whether reset recovery needs the user's attention. The
    /// selected provider credit and idempotency key never leave Rust-owned
    /// storage.
    pub fn has_pending_reset_credit(&self) -> Result<bool, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        Ok(store.pending_reset_credit.is_some())
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
        self.ensure_store_ready()?;
        self.acquire_operation_lock()
    }

    fn acquire_recovery_operation(&self) -> Result<OperationGuard<'_>, String> {
        self.acquire_operation_lock()
    }

    fn acquire_operation_lock(&self) -> Result<OperationGuard<'_>, String> {
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
                // Reauthentication invalidates a prior capacity snapshot.
                quota: None,
                reset_credits: None,
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
                quota: None,
                reset_credits: None,
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

    /// Store the complete refreshed credential and its accompanying quota in
    /// one atomic account-store replacement. The caller has already verified
    /// that the refreshed document still belongs to this profile.
    pub fn update_credential_and_quota_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        id: &str,
        credential: Value,
        quota: crate::types::QuotaSnapshot,
        reset_credits: Option<StoredResetCredits>,
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
        candidate.accounts[index].quota = Some(quota);
        candidate.accounts[index].reset_credits = reset_credits;
        storage::save_atomic(&self.store_path, &candidate)?;
        *store = candidate;
        Ok(())
    }

    /// Stores a provider projection without replacing the credential
    /// document. Read-only quota refreshes use this path so a live token
    /// snapshot can never become a credential mutation.
    pub fn update_quota_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        id: &str,
        quota: crate::types::QuotaSnapshot,
        reset_credits: Option<StoredResetCredits>,
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
        candidate.accounts[index].quota = Some(quota);
        candidate.accounts[index].reset_credits = reset_credits;
        storage::save_atomic(&self.store_path, &candidate)?;
        *store = candidate;
        Ok(())
    }

    pub fn prepare_reset_credit_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        pending: PendingResetCredit,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        if store.pending_reset_credit.is_some() {
            return Err(
                "GSwitch must recover a previous reset-credit operation before starting another"
                    .to_string(),
            );
        }
        let mut candidate = store.clone();
        candidate.pending_reset_credit = Some(pending);
        storage::save_atomic(&self.store_path, &candidate)?;
        *store = candidate;
        Ok(())
    }

    pub fn pending_reset_credit_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
    ) -> Result<Option<PendingResetCredit>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        Ok(store.pending_reset_credit.clone())
    }

    pub fn clear_pending_reset_credit_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        let mut candidate = store.clone();
        candidate.pending_reset_credit = None;
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

    pub fn start_oauth_login(
        &self,
        login_id: String,
        auth_url: String,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(), String> {
        self.ensure_store_ready()?;
        let mut sessions = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is unavailable".to_string())?;
        if sessions
            .values()
            .any(|session| matches!(session.status, OAuthLoginStatus::Pending))
        {
            return Err("Another OAuth sign-in is already waiting for completion".to_string());
        }
        sessions.insert(
            login_id,
            OAuthLoginControl {
                status: OAuthLoginStatus::Pending,
                auth_url,
                cancelled,
            },
        );
        Ok(())
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
        let session = sessions
            .get_mut(&login_id)
            .ok_or_else(|| "Unknown OAuth login session".to_string())?;
        session.status = status;
        Ok(())
    }

    pub fn oauth_status(&self, login_id: &str) -> Result<OAuthLoginStatus, String> {
        let sessions = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is unavailable".to_string())?;

        sessions
            .get(login_id)
            .map(|session| session.status.clone())
            .ok_or_else(|| "Unknown OAuth login session".to_string())
    }

    pub fn oauth_url(&self, login_id: &str) -> Result<String, String> {
        self.oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is unavailable".to_string())?
            .get(login_id)
            .map(|session| session.auth_url.clone())
            .ok_or_else(|| "Unknown OAuth login session".to_string())
    }

    pub fn cancel_oauth(&self, login_id: &str) -> Result<(), String> {
        let sessions = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is unavailable".to_string())?;
        let session = sessions
            .get(login_id)
            .ok_or_else(|| "Unknown OAuth login session".to_string())?;
        if matches!(session.status, OAuthLoginStatus::Pending) {
            session.cancelled.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Wake state is deliberately process-local. It exists only to let the
    /// current window render progress and request cancellation; it is not a
    /// durable queue or history surface.
    pub fn insert_wake_operation(
        &self,
        view: WakeOperationView,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let mut operations = self
            .wake_operations
            .lock()
            .map_err(|_| "Wake operation lock is unavailable".to_string())?;
        if operations
            .values()
            .any(|operation| matches!(&operation.view.status, WakeOperationStatus::Running))
        {
            return Err("A Wake operation is already running".to_string());
        }
        operations
            .retain(|_, operation| matches!(&operation.view.status, WakeOperationStatus::Running));
        operations.insert(view.id.clone(), WakeControl { view, cancelled });
        Ok(())
    }

    pub fn update_wake_operation(&self, view: WakeOperationView) -> Result<(), String> {
        let mut operations = self
            .wake_operations
            .lock()
            .map_err(|_| "Wake operation lock is unavailable".to_string())?;
        let operation = operations
            .get_mut(&view.id)
            .ok_or_else(|| "Unknown Wake operation".to_string())?;
        operation.view = view;
        Ok(())
    }

    pub fn wake_operation(&self, id: &str) -> Result<WakeOperationView, String> {
        self.wake_operations
            .lock()
            .map_err(|_| "Wake operation lock is unavailable".to_string())?
            .get(id)
            .map(|operation| operation.view.clone())
            .ok_or_else(|| "Unknown Wake operation".to_string())
    }

    pub fn cancel_wake(&self, id: &str) -> Result<(), String> {
        let operations = self
            .wake_operations
            .lock()
            .map_err(|_| "Wake operation lock is unavailable".to_string())?;
        let operation = operations
            .get(id)
            .ok_or_else(|| "Unknown Wake operation".to_string())?;
        if matches!(&operation.view.status, WakeOperationStatus::Running) {
            operation.cancelled.store(true, Ordering::SeqCst);
        }
        Ok(())
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
    fn damaged_store_requires_explicit_reset_and_never_touches_other_files() {
        let path = temp_path("damaged-store");
        let parent = path.parent().expect("parent");
        fs::create_dir_all(parent).expect("create parent");
        fs::write(&path, "{ private-token").expect("write damaged store");
        let live_credential = parent.join("unrelated-live-auth.json");
        fs::write(&live_credential, "live Codex credential").expect("write live sentinel");

        let state = AppState::new(path.clone()).expect("recovery state");
        assert_eq!(state.storage_view().status, StorageStatus::RecoveryRequired);
        assert!(state.list().expect("empty recovery projection").is_empty());
        match state.acquire_operation() {
            Ok(_) => panic!("recovery must block credential operations"),
            Err(error) => assert_eq!(
                error,
                "GSwitch account storage needs recovery before it can make changes"
            ),
        }

        state.reset_damaged_store().expect("explicit reset");
        assert_eq!(state.storage_view().status, StorageStatus::Ready);
        assert!(crate::storage::load(&path)
            .expect("fresh store")
            .accounts
            .is_empty());
        assert_eq!(
            fs::read_to_string(&live_credential).expect("live credential remains"),
            "live Codex credential"
        );
        let preserved = fs::read_dir(parent.join("damaged-account-stores"))
            .expect("recovery directory")
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("accounts-damaged-")
            });
        assert!(preserved);

        let _ = fs::remove_dir_all(parent);
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

    #[test]
    fn wake_operations_are_singleton_and_cancellable() {
        let path = temp_path("wake-operation");
        let state = AppState::new(path.clone()).expect("state");
        let cancelled = Arc::new(AtomicBool::new(false));
        let first = WakeOperationView {
            id: "first".to_string(),
            status: WakeOperationStatus::Running,
            current_account_id: None,
            results: Vec::new(),
        };
        state
            .insert_wake_operation(first.clone(), cancelled.clone())
            .expect("first wake");

        let second = WakeOperationView {
            id: "second".to_string(),
            status: WakeOperationStatus::Running,
            current_account_id: None,
            results: Vec::new(),
        };
        assert_eq!(
            state
                .insert_wake_operation(second.clone(), Arc::new(AtomicBool::new(false)))
                .expect_err("overlapping wake"),
            "A Wake operation is already running"
        );
        state.cancel_wake("first").expect("cancel wake");
        assert!(cancelled.load(Ordering::SeqCst));

        let mut completed = first;
        completed.status = WakeOperationStatus::Completed;
        state
            .update_wake_operation(completed)
            .expect("complete first wake");
        state
            .insert_wake_operation(second, Arc::new(AtomicBool::new(false)))
            .expect("later wake");
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn oauth_cancellation_is_scoped_to_the_pending_login() {
        let path = temp_path("oauth-cancel");
        let state = AppState::new(path.clone()).expect("state");
        let cancelled = Arc::new(AtomicBool::new(false));
        state
            .start_oauth_login(
                "login-one".to_string(),
                "https://auth.openai.com/example".to_string(),
                cancelled.clone(),
            )
            .expect("start oauth");

        state.cancel_oauth("login-one").expect("cancel oauth");
        assert!(cancelled.load(Ordering::SeqCst));
        assert_eq!(
            state.oauth_url("login-one").expect("url"),
            "https://auth.openai.com/example"
        );
        state
            .set_oauth_status("login-one".to_string(), OAuthLoginStatus::Cancelled)
            .expect("complete cancel");
        assert!(matches!(
            state.oauth_status("login-one").expect("status"),
            OAuthLoginStatus::Cancelled
        ));
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }
}
