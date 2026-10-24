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
use serde_json::Value;
use uuid::Uuid;

use crate::{
    app_server::AccountMetadata,
    identity::{derive_identity, document_kind},
    storage::{self, AccountStore},
    types::{
        AccountIdentity, AccountKind, AccountView, OAuthLoginStatus, PendingResetCredit,
        PendingResetView, PendingSwitch, StorageStatus, StorageView, StoredAccount,
        StoredResetCredits, WakeOperationStatus, WakeOperationView,
    },
    vault::LegacyVault,
};

const VAULT_MIGRATION_FAILED: &str = "Legacy vault migration could not read saved accounts";
const PENDING_CREDENTIAL_VERSION: u32 = 2;

#[derive(Clone)]
pub struct AppState {
    store_path: Arc<PathBuf>,
    store_recovery_dir: Arc<PathBuf>,
    pending_dir: Arc<PathBuf>,
    store: Arc<Mutex<AccountStore>>,
    store_recovery_required: Arc<AtomicBool>,
    store_recovery_message: Arc<Mutex<Option<String>>>,
    operation_lock: Arc<Mutex<()>>,
    oauth_logins: Arc<Mutex<HashMap<String, OAuthLoginControl>>>,
    wake_operations: Arc<Mutex<HashMap<String, WakeControl>>>,
}

#[derive(Clone)]
struct OAuthLoginControl {
    status: OAuthLoginStatus,
    auth_url: String,
    cancelled: Arc<AtomicBool>,
    recovery_ref: Option<String>,
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

#[derive(Debug)]
pub(crate) enum OperationAcquireFailure {
    Busy,
    Failed(String),
}

impl std::fmt::Display for OperationAcquireFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => formatter.write_str("Another GSwitch operation is already in progress"),
            Self::Failed(message) => formatter.write_str(message),
        }
    }
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
    pub workspace_name: Option<String>,
    pub account_structure: Option<String>,
    pub identity: AccountIdentity,
    pub credential: Value,
}

/// One interrupted credential save, kept in its own owner-only file. Version 1
/// files predate the Stronghold vault and hold only the credential.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct PendingCredential {
    version: u32,
    credential: Value,
    #[serde(default)]
    expected_identity: Option<AccountIdentity>,
    #[serde(default)]
    expected_account_id: Option<String>,
    #[serde(default)]
    expected_generation: Option<u64>,
    #[serde(default)]
    oauth: Option<crate::intake::OAuthRecovery>,
}

impl AppState {
    pub fn new(store_path: PathBuf) -> Result<Self, String> {
        let storage_root = store_path
            .parent()
            .ok_or_else(|| "Invalid GSwitch storage path".to_string())?
            .to_path_buf();
        #[cfg(test)]
        let vault = LegacyVault::with_test_root(storage_root.join("credentials.hold"));
        #[cfg(not(test))]
        let vault = LegacyVault::new(storage_root.join("credentials.hold"));
        let pending_dir = storage_root.join("pending-credentials");
        let (store, store_recovery_required, store_recovery_message) =
            match Self::load_store_serialized(&store_path, &pending_dir, &vault) {
                Ok(store) => (store, false, None),
                // Do not prevent the desktop application from opening because
                // GSwitch's own account file is unreadable. The replacement
                // default is intentionally unusable until the user confirms a
                // recovery reset; it can never authorize a credential mutation.
                Err(error) => (
                    AccountStore::default(),
                    true,
                    Some(Self::recovery_message(&error)),
                ),
            };

        let state = Self {
            store_path: Arc::new(store_path),
            store_recovery_dir: Arc::new(storage_root.join("damaged-account-stores")),
            pending_dir: Arc::new(pending_dir),
            store: Arc::new(Mutex::new(store)),
            store_recovery_required: Arc::new(AtomicBool::new(store_recovery_required)),
            store_recovery_message: Arc::new(Mutex::new(store_recovery_message)),
            operation_lock: Arc::new(Mutex::new(())),
            oauth_logins: Arc::new(Mutex::new(HashMap::new())),
            wake_operations: Arc::new(Mutex::new(HashMap::new())),
        };
        if let Ok(root) = state.isolated_profile_root() {
            crate::isolated_profile::remove_leftovers(&root);
        }
        Ok(state)
    }

    fn recovery_message(error: &str) -> String {
        if error == VAULT_MIGRATION_FAILED {
            "GSwitch could not move saved accounts out of its previous encrypted storage. Nothing was changed, and GSwitch will try again the next time it starts."
                .to_string()
        } else if error.starts_with("Legacy") {
            "GSwitch could not complete account storage migration. The previous account library was left unchanged."
                .to_string()
        } else {
            "GSwitch could not safely read its saved account library. Codex credentials were not changed."
                .to_string()
        }
    }

    fn load_store_serialized(
        store_path: &std::path::Path,
        pending_dir: &std::path::Path,
        vault: &LegacyVault,
    ) -> Result<AccountStore, String> {
        let parent = store_path
            .parent()
            .ok_or_else(|| "Invalid GSwitch storage path".to_string())?;
        storage::ensure_private_dir(parent)
            .map_err(|_| "Unable to prepare GSwitch operation storage".to_string())?;
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(parent.join("operations.lock"))
            .map_err(|_| "Unable to lock GSwitch operations".to_string())?;
        FileExt::lock_exclusive(&lock_file)
            .map_err(|_| "Unable to lock GSwitch operations".to_string())?;
        // The file lock spans the load, a one-time migration and its readback.
        // Closing it also unlocks.
        Self::load_store(store_path, pending_dir, vault)
    }

    fn load_store(
        store_path: &std::path::Path,
        pending_dir: &std::path::Path,
        vault: &LegacyVault,
    ) -> Result<AccountStore, String> {
        let store = match storage::read_store_json(store_path)? {
            Some(mut raw) => {
                let version = storage::store_version(&raw)?;
                let vault_pending = if version == storage::VAULT_STORE_VERSION {
                    vault
                        .resolve(&mut raw)
                        .map_err(|_| VAULT_MIGRATION_FAILED.to_string())?
                } else {
                    Vec::new()
                };
                let mut store = storage::store_from_json(raw)?;
                Self::verify_credentials(&mut store, version)?;
                if version < storage::STORE_VERSION {
                    Self::commit_migration(store_path, pending_dir, &mut store, &vault_pending)
                        .map_err(|error| {
                            if version == storage::VAULT_STORE_VERSION {
                                VAULT_MIGRATION_FAILED.to_string()
                            } else {
                                error
                            }
                        })?;
                }
                // The file now holds everything. A vault left by an interrupted
                // cleanup is removed without being read.
                if vault.exists() {
                    let _ = vault.remove();
                }
                store
            }
            None => AccountStore::default(),
        };
        for (_, pending) in storage::pending_credentials::<PendingCredential>(pending_dir)? {
            Self::validate_pending(&pending)?;
        }
        Ok(store)
    }

    /// Every saved account must hold a complete credential that still derives
    /// its recorded kind and identity. Stores older than the vault may not
    /// have recorded an identity yet.
    fn verify_credentials(store: &mut AccountStore, version: u32) -> Result<(), String> {
        let failure = || {
            if version < storage::VAULT_STORE_VERSION {
                "Legacy account storage could not be verified safely".to_string()
            } else {
                "Saved account credentials do not match their account metadata".to_string()
            }
        };
        for account in &mut store.accounts {
            if account.credential.is_null() {
                return Err(failure());
            }
            let kind = document_kind(&account.credential).map_err(|_| failure())?;
            let identity = derive_identity(&kind, &account.credential).map_err(|_| failure())?;
            let recorded = account.identity.as_ref();
            if account.kind != kind
                || recorded.is_some_and(|saved| saved != &identity)
                || (recorded.is_none() && version >= storage::VAULT_STORE_VERSION)
            {
                return Err(failure());
            }
            account.identity = Some(identity);
            account.credential_generation = account.credential_generation.max(1);
        }
        if store.pending_reset_credit.as_ref().is_some_and(|pending| {
            pending.credit_id.is_empty() || pending.idempotency_key.is_empty()
        }) {
            return Err(failure());
        }
        Ok(())
    }

    /// Writes a migrated store and its pending entries, then reads both back.
    /// Only after this succeeds may the previous vault be removed.
    fn commit_migration(
        store_path: &std::path::Path,
        pending_dir: &std::path::Path,
        store: &mut AccountStore,
        vault_pending: &[(String, Value)],
    ) -> Result<(), String> {
        let failure = || "Legacy account storage could not be verified safely".to_string();
        let mut pending = Vec::with_capacity(vault_pending.len());
        for (reference, value) in vault_pending {
            let mut record = value.clone();
            record
                .as_object_mut()
                .ok_or_else(failure)?
                .insert("version".into(), PENDING_CREDENTIAL_VERSION.into());
            let entry: PendingCredential = serde_json::from_value(record).map_err(|_| failure())?;
            Self::validate_pending(&entry).map_err(|_| failure())?;
            pending.push((reference, entry));
        }
        for (reference, entry) in &pending {
            storage::write_pending_credential(pending_dir, reference, entry)?;
        }
        store.version = storage::STORE_VERSION;
        storage::save_atomic(store_path, store)?;
        if !same_json(&storage::load(store_path)?, store) {
            return Err(failure());
        }
        for (reference, entry) in &pending {
            let saved: PendingCredential =
                storage::read_pending_credential(pending_dir, reference)?;
            if !same_json(&saved, entry) {
                return Err(failure());
            }
        }
        Ok(())
    }

    fn validate_pending(pending: &PendingCredential) -> Result<(), String> {
        if !matches!(pending.version, 1 | PENDING_CREDENTIAL_VERSION)
            || !pending.credential.is_object()
            || pending.expected_account_id.is_some() != pending.expected_generation.is_some()
        {
            return Err("Credential recovery is incomplete".to_string());
        }
        let kind = document_kind(&pending.credential)?;
        derive_identity(&kind, &pending.credential).map(|_| ())
    }

    fn next_credential_generation(previous: Option<u64>, initial: u64) -> Result<u64, String> {
        match previous {
            Some(generation) => generation
                .checked_add(1)
                .ok_or_else(|| "Credential generation is exhausted".to_string()),
            None => Ok(initial.max(1)),
        }
    }

    /// The only store commit: one atomic replacement of the owner-only file.
    /// A changed credential must still derive its account's identity.
    fn persist_candidate(
        &self,
        current: &AccountStore,
        candidate: &mut AccountStore,
    ) -> Result<(), String> {
        for account in &mut candidate.accounts {
            let previous = current.accounts.iter().find(|saved| saved.id == account.id);
            let changed = previous.is_none_or(|saved| {
                saved.credential != account.credential
                    || saved.reset_credits != account.reset_credits
            });
            if changed {
                let kind = document_kind(&account.credential)?;
                let identity = derive_identity(&kind, &account.credential)?;
                if account.kind != kind || account.identity.as_ref() != Some(&identity) {
                    return Err("A saved credential does not match its account".to_string());
                }
                account.credential_generation = Self::next_credential_generation(
                    previous.map(|saved| saved.credential_generation),
                    account.credential_generation,
                )?;
            }
        }
        storage::save_atomic(&self.store_path, candidate)
    }

    pub fn storage_view(&self) -> StorageView {
        if self.store_recovery_required.load(Ordering::SeqCst) {
            StorageView {
                status: StorageStatus::RecoveryRequired,
                message: self
                    .store_recovery_message
                    .lock()
                    .ok()
                    .and_then(|message| message.clone())
                    .or_else(|| {
                        Some(
                            "GSwitch storage needs recovery before it can make changes."
                                .to_string(),
                        )
                    }),
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
        if let Ok(mut message) = self.store_recovery_message.lock() {
            *message = None;
        }
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

    /// Exposes which saved account has a reset needing recovery, and whether
    /// retrying can no longer help. The selected provider credit and
    /// idempotency key never leave Rust-owned storage.
    pub fn pending_reset_view(&self) -> Result<Option<PendingResetView>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        Ok(store.pending_reset_credit.as_ref().map(|pending| {
            let account_saved = store
                .accounts
                .iter()
                .any(|account| account.id == pending.account_id);
            PendingResetView {
                account_id: pending.account_id.clone(),
                discardable: pending.discard_allowed || !account_saved,
            }
        }))
    }

    pub fn allow_pending_reset_discard_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        let mut candidate = store.clone();
        let Some(pending) = candidate.pending_reset_credit.as_mut() else {
            return Ok(());
        };
        pending.discard_allowed = true;
        self.persist_candidate(&store, &mut candidate)?;
        *store = candidate;
        Ok(())
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
            .map_err(|error| error.to_string())
    }

    /// A read-only quota request finishes its network work before reaching this
    /// lock. Waiting here lets a user-started credential operation finish first;
    /// the short quota commit still uses the cross-process storage boundary.
    pub(crate) fn acquire_quota_commit_operation(&self) -> Result<OperationGuard<'_>, String> {
        self.ensure_store_ready()?;
        self.wait_for_operation_lock()
    }

    /// Wake sends its requests without this lock. Only an isolated refresh
    /// after an authentication failure, and marking a rejected credential,
    /// wait here, so GSwitch's own short work never makes Wake fail as busy.
    pub(crate) fn acquire_wake_operation(&self) -> Result<OperationGuard<'_>, String> {
        self.ensure_store_ready()?;
        self.wait_for_operation_lock()
    }

    fn wait_for_operation_lock(&self) -> Result<OperationGuard<'_>, String> {
        let in_process = self
            .operation_lock
            .lock()
            .map_err(|_| "Unable to lock GSwitch operations".to_string())?;
        crate::owned_process::ensure_quiet()?;
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
        FileExt::lock_exclusive(&lock_file)
            .map_err(|_| "Unable to lock GSwitch operations".to_string())?;
        Ok(OperationGuard {
            _in_process: in_process,
            lock_file,
        })
    }

    pub(crate) fn acquire_operation_for_switch(
        &self,
    ) -> Result<OperationGuard<'_>, OperationAcquireFailure> {
        self.ensure_store_ready()
            .map_err(OperationAcquireFailure::Failed)?;
        self.acquire_operation_lock()
    }

    fn acquire_recovery_operation(&self) -> Result<OperationGuard<'_>, String> {
        self.acquire_operation_lock()
            .map_err(|error| error.to_string())
    }

    fn acquire_operation_lock(&self) -> Result<OperationGuard<'_>, OperationAcquireFailure> {
        let in_process = self
            .operation_lock
            .try_lock()
            .map_err(|_| OperationAcquireFailure::Busy)?;
        crate::owned_process::ensure_quiet().map_err(OperationAcquireFailure::Failed)?;
        let parent = self.store_path.parent().ok_or_else(|| {
            OperationAcquireFailure::Failed("Invalid GSwitch storage path".to_string())
        })?;
        fs::create_dir_all(parent).map_err(|_| {
            OperationAcquireFailure::Failed(
                "Unable to prepare GSwitch operation storage".to_string(),
            )
        })?;

        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(parent.join("operations.lock"))
            .map_err(|_| {
                OperationAcquireFailure::Failed("Unable to lock GSwitch operations".to_string())
            })?;
        lock_file
            .try_lock_exclusive()
            .map_err(|_| OperationAcquireFailure::Busy)?;

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

    pub fn find_import_match_under_operation(
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
                    && (account.identity.as_ref() == Some(identity)
                        || (account.identity.is_none() && account.credential == *credential))
            })
            .map(|account| Self::view(account, store.active_account_id.as_deref())))
    }

    pub fn upsert_under_operation(
        &self,
        operation: &OperationGuard<'_>,
        draft: AccountDraft,
    ) -> Result<AccountView, String> {
        self.upsert_with_live_state_under_operation(operation, draft, false, false)
    }

    pub(crate) fn upsert_verified_live_under_operation(
        &self,
        operation: &OperationGuard<'_>,
        draft: AccountDraft,
    ) -> Result<AccountView, String> {
        self.upsert_with_live_state_under_operation(operation, draft, true, false)
    }

    pub(crate) fn upsert_pending_live_under_operation(
        &self,
        operation: &OperationGuard<'_>,
        draft: AccountDraft,
    ) -> Result<AccountView, String> {
        self.upsert_with_live_state_under_operation(operation, draft, false, true)
    }

    fn upsert_with_live_state_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        draft: AccountDraft,
        verified_live: bool,
        pending_new: bool,
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
            if verified_live && existing.needs_apply {
                return Err("A newer saved sign-in is waiting to be applied".into());
            }
            let credential_changed = existing.credential != draft.credential;
            StoredAccount {
                id: existing.id.clone(),
                label: draft.label.unwrap_or_else(|| existing.label.clone()),
                kind: draft.kind,
                email: draft.email.or_else(|| existing.email.clone()),
                plan_type: draft.plan_type,
                workspace_name: draft
                    .workspace_name
                    .or_else(|| existing.workspace_name.clone()),
                account_structure: draft
                    .account_structure
                    .or_else(|| existing.account_structure.clone()),
                identity: Some(draft.identity),
                // A new login invalidates the live projection without erasing
                // the last visible values. An unchanged credential does not.
                quota: existing.quota.clone().map(|mut quota| {
                    if credential_changed {
                        quota.fetched_at_unix_ms = 0;
                    }
                    quota
                }),
                reset_credits: if credential_changed {
                    None
                } else {
                    existing.reset_credits.clone()
                },
                credential_generation: existing.credential_generation,
                needs_apply: !verified_live && (existing.needs_apply || credential_changed),
                sign_in_required: false,
                credential: draft.credential,
            }
        } else {
            StoredAccount {
                id: Uuid::new_v4().to_string(),
                label: draft.label.unwrap_or(draft.default_label),
                kind: draft.kind,
                email: draft.email,
                plan_type: draft.plan_type,
                workspace_name: draft.workspace_name,
                account_structure: draft.account_structure,
                identity: Some(draft.identity),
                quota: None,
                reset_credits: None,
                credential_generation: 0,
                needs_apply: pending_new,
                sign_in_required: false,
                credential: draft.credential,
            }
        };

        let mut candidate = store.clone();
        if let Some(index) = existing_index {
            candidate.accounts[index] = account.clone();
        } else {
            candidate.accounts.push(account.clone());
        }
        self.persist_candidate(&store, &mut candidate)?;
        let active_account_id = candidate.active_account_id.clone();
        let account = candidate
            .accounts
            .iter()
            .find(|saved| saved.id == account.id)
            .unwrap();
        let view = Self::view(account, active_account_id.as_deref());
        *store = candidate;
        Ok(view)
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

    pub(crate) fn mark_sign_in_required(
        &self,
        _operation: &OperationGuard<'_>,
        id: &str,
        generation: u64,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable")?;
        let Some(index) = store.accounts.iter().position(|account| account.id == id) else {
            return Ok(());
        };
        let account = &store.accounts[index];
        if account.credential_generation != generation
            || account.needs_apply
            || account.sign_in_required
        {
            return Ok(());
        }
        let mut candidate = store.clone();
        candidate.accounts[index].sign_in_required = true;
        self.persist_candidate(&store, &mut candidate)?;
        *store = candidate;
        Ok(())
    }

    pub fn update_credential_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        id: &str,
        credential: Value,
        expected_generation: u64,
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
        if store.accounts[index].credential_generation != expected_generation {
            return Err("The saved sign-in changed before live reconciliation".into());
        }
        if store.accounts[index].needs_apply || store.accounts[index].credential == credential {
            return Ok(());
        }
        let mut candidate = store.clone();
        candidate.accounts[index].credential = credential;
        self.persist_candidate(&store, &mut candidate)?;
        *store = candidate;
        Ok(())
    }

    /// Replaces a saved credential with the same-identity document Codex
    /// refreshed in an isolated profile. The caller has verified the identity;
    /// account metadata and quota wait for the next provider read.
    pub fn update_refreshed_credential_under_operation(
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
        candidate.accounts[index].needs_apply |= candidate.accounts[index].credential != credential;
        candidate.accounts[index].sign_in_required = false;
        candidate.accounts[index].credential = credential;
        self.persist_candidate(&store, &mut candidate)?;
        *store = candidate;
        Ok(())
    }

    /// Commits the provider projection verified for a switch target.
    pub fn update_switch_validation_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        id: &str,
        metadata: &AccountMetadata,
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
        let saved = &store.accounts[index];
        if !saved.sign_in_required
            && saved.email == metadata.email
            && saved.plan_type == metadata.plan_type
            && metadata
                .workspace_name
                .as_ref()
                .is_none_or(|name| saved.workspace_name.as_ref() == Some(name))
            && metadata
                .account_structure
                .as_ref()
                .is_none_or(|structure| saved.account_structure.as_ref() == Some(structure))
        {
            return Ok(());
        }
        let mut candidate = store.clone();
        let account = &mut candidate.accounts[index];
        account.sign_in_required = false;
        account.email = metadata.email.clone();
        account.plan_type = metadata.plan_type.clone();
        if metadata.workspace_name.is_some() {
            account.workspace_name = metadata.workspace_name.clone();
        }
        if metadata.account_structure.is_some() {
            account.account_structure = metadata.account_structure.clone();
        }
        self.persist_candidate(&store, &mut candidate)?;
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
        candidate.accounts[index].needs_apply |= candidate.accounts[index].credential != credential;
        candidate.accounts[index].sign_in_required = false;
        candidate.accounts[index].credential = credential;
        candidate.accounts[index].quota = Some(quota);
        candidate.accounts[index].reset_credits = reset_credits;
        self.persist_candidate(&store, &mut candidate)?;
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
        candidate.accounts[index].sign_in_required = false;
        candidate.accounts[index].quota = Some(quota);
        candidate.accounts[index].reset_credits = reset_credits;
        self.persist_candidate(&store, &mut candidate)?;
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
        self.persist_candidate(&store, &mut candidate)?;
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
        self.persist_candidate(&store, &mut candidate)?;
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
        self.persist_candidate(&store, &mut candidate)?;
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
        self.persist_candidate(&store, &mut candidate)?;
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
        if !store.accounts.iter().any(|account| account.id == id) {
            return Err("The selected account is no longer saved".to_string());
        }
        let mut candidate = store.clone();
        candidate.active_account_id = Some(id.to_string());
        candidate.pending_switch = None;
        let applied = candidate
            .accounts
            .iter_mut()
            .find(|account| account.id == id)
            .unwrap();
        applied.needs_apply = false;
        applied.sign_in_required = false;
        let account = applied.clone();
        self.persist_candidate(&store, &mut candidate)?;
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
        self.persist_candidate(&store, &mut candidate)?;
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
        self.persist_candidate(&store, &mut candidate)?;
        *store = candidate;
        Ok(())
    }

    /// Retains an interrupted refreshed credential in its own owner-only file.
    /// This is deliberately a small recovery queue, not a general backup service.
    pub fn record_pending_credential(
        &self,
        operation: &OperationGuard<'_>,
        credential: &Value,
    ) -> Result<(), String> {
        self.record_pending_credential_for_identity(operation, credential, None)
    }

    pub fn record_pending_credential_for_identity(
        &self,
        _operation: &OperationGuard<'_>,
        credential: &Value,
        expected_identity: Option<&AccountIdentity>,
    ) -> Result<(), String> {
        let kind = document_kind(credential)?;
        let identity = derive_identity(&kind, credential)?;
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is unavailable".to_string())?;
        let existing = store
            .accounts
            .iter()
            .find(|account| account.kind == kind && account.identity.as_ref() == Some(&identity));
        let entry = PendingCredential {
            version: PENDING_CREDENTIAL_VERSION,
            credential: credential.clone(),
            expected_identity: expected_identity.cloned(),
            expected_account_id: existing.map(|account| account.id.clone()),
            expected_generation: existing.map(|account| account.credential_generation),
            oauth: None,
        };
        drop(store);
        storage::write_pending_credential(
            &self.pending_dir,
            &storage::new_pending_credential_reference(),
            &entry,
        )
    }

    /// Explicit, Rust-only recovery of a credential whose previous metadata
    /// commit failed. No recovery reference or credential crosses Tauri IPC.
    /// A changed account generation is never overwritten by an older queue
    /// entry; retirement follows a successful metadata commit.
    pub fn recover_pending_credentials(&self) -> Result<u32, String> {
        let pending = storage::pending_credentials::<PendingCredential>(&self.pending_dir)?;
        let mut recovered = 0u32;
        let mut first_error = None;
        for (reference, secret) in pending {
            let result: Result<(), String> = (|| {
                if let Some(context) = &secret.oauth {
                    crate::intake::finish_oauth_recovery(self, &secret.credential, context)?;
                    return storage::retire_pending_credential(&self.pending_dir, &reference);
                }
                let operation = self.acquire_operation()?;
                let kind = document_kind(&secret.credential)?;
                let identity = derive_identity(&kind, &secret.credential)?;
                if secret
                    .expected_identity
                    .as_ref()
                    .is_some_and(|expected| expected != &identity)
                {
                    return Err("Credential recovery identity changed".to_string());
                }
                if secret.expected_account_id.is_some() != secret.expected_generation.is_some() {
                    return Err("Credential recovery is incomplete".to_string());
                }
                {
                    let mut store = self
                        .store
                        .lock()
                        .map_err(|_| "Account store lock is unavailable".to_string())?;
                    let existing = store.accounts.iter().position(|account| {
                        account.kind == kind && account.identity.as_ref() == Some(&identity)
                    });
                    match (existing, secret.expected_account_id.as_deref()) {
                        (Some(index), Some(expected_id)) => {
                            let account = &store.accounts[index];
                            if account.id != expected_id {
                                return Err("Credential recovery account changed".to_string());
                            }
                            if account.credential != secret.credential {
                                if Some(account.credential_generation) != secret.expected_generation
                                {
                                    return Err(
                                        "Credential recovery generation changed".to_string()
                                    );
                                }
                                let mut candidate = store.clone();
                                candidate.accounts[index].needs_apply = true;
                                candidate.accounts[index].sign_in_required = false;
                                candidate.accounts[index].credential = secret.credential.clone();
                                candidate.accounts[index].quota = None;
                                candidate.accounts[index].reset_credits = None;
                                self.persist_candidate(&store, &mut candidate)?;
                                *store = candidate;
                            }
                        }
                        (Some(index), None)
                            if store.accounts[index].credential == secret.credential => {}
                        (Some(_), None) => {
                            return Err(
                                "Credential recovery has no safe account baseline".to_string()
                            );
                        }
                        (None, Some(_)) => {
                            return Err(
                                "Credential recovery account is no longer saved".to_string()
                            );
                        }
                        (None, None) => {
                            drop(store);
                            self.upsert_under_operation(
                                &operation,
                                AccountDraft {
                                    label: None,
                                    default_label: match kind {
                                        AccountKind::ChatGpt => {
                                            "Recovered ChatGPT account".to_string()
                                        }
                                        AccountKind::ApiKey => "Recovered API key".to_string(),
                                    },
                                    kind,
                                    email: None,
                                    plan_type: None,
                                    workspace_name: None,
                                    account_structure: None,
                                    identity,
                                    credential: secret.credential,
                                },
                            )?;
                        }
                    }
                }
                storage::retire_pending_credential(&self.pending_dir, &reference)?;
                Ok(())
            })();
            match result {
                Ok(()) => {
                    recovered = recovered
                        .checked_add(1)
                        .ok_or_else(|| "Credential recovery count overflow".to_string())?;
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(recovered),
        }
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
        if sessions.values().any(|session| {
            matches!(
                session.status,
                OAuthLoginStatus::Pending | OAuthLoginStatus::Finishing
            )
        }) {
            return Err("Another OAuth sign-in is already waiting for completion".to_string());
        }
        sessions.insert(
            login_id,
            OAuthLoginControl {
                status: OAuthLoginStatus::Pending,
                auth_url,
                cancelled,
                recovery_ref: None,
            },
        );
        Ok(())
    }

    /// Acceptance and cancellation are serialized. A browser tab closing after
    /// completion has no effect on the local commit.
    pub(crate) fn accept_oauth_completion(&self, login_id: &str) -> Result<bool, String> {
        let mut sessions = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is unavailable")?;
        let session = sessions
            .get_mut(login_id)
            .ok_or("Unknown OAuth login session")?;
        if session.cancelled.load(Ordering::SeqCst)
            || !matches!(session.status, OAuthLoginStatus::Pending)
        {
            return Ok(false);
        }
        session.status = OAuthLoginStatus::Finishing;
        Ok(true)
    }

    pub(crate) fn stage_oauth_recovery(
        &self,
        login_id: &str,
        credential: &Value,
        context: crate::intake::OAuthRecovery,
    ) -> Result<(), String> {
        let reference = storage::new_pending_credential_reference();
        let entry = PendingCredential {
            version: PENDING_CREDENTIAL_VERSION,
            credential: credential.clone(),
            expected_identity: None,
            expected_account_id: None,
            expected_generation: None,
            oauth: Some(context),
        };
        storage::write_pending_credential(&self.pending_dir, &reference, &entry)?;
        self.oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is unavailable")?
            .get_mut(login_id)
            .ok_or("Unknown OAuth login session")?
            .recovery_ref = Some(reference);
        Ok(())
    }

    pub(crate) fn retry_oauth_completion(&self, login_id: &str) -> Result<(), String> {
        let mut sessions = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is unavailable")?;
        let session = sessions
            .get_mut(login_id)
            .ok_or("Unknown OAuth login session")?;
        if !matches!(
            session.status,
            OAuthLoginStatus::Failed {
                retryable: true,
                ..
            }
        ) {
            return Err("This sign-in cannot be retried".into());
        }
        session.status = OAuthLoginStatus::Finishing;
        Ok(())
    }

    pub(crate) fn finish_staged_oauth(
        &self,
        login_id: &str,
    ) -> Result<(AccountView, bool), String> {
        let reference = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is unavailable")?
            .get(login_id)
            .and_then(|session| session.recovery_ref.clone())
            .ok_or("No completed sign-in is available to retry")?;
        let secret: PendingCredential =
            storage::read_pending_credential(&self.pending_dir, &reference)?;
        let account = crate::intake::finish_oauth_recovery(
            self,
            &secret.credential,
            secret
                .oauth
                .as_ref()
                .ok_or("No completed sign-in is available to retry")?,
        )?;
        let retirement_warning =
            storage::retire_pending_credential(&self.pending_dir, &reference).is_err();
        self.oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is unavailable")?
            .get_mut(login_id)
            .ok_or("Unknown OAuth login session")?
            .recovery_ref = None;
        Ok((account, retirement_warning))
    }

    pub(crate) fn oauth_is_retryable(&self, login_id: &str) -> bool {
        self.oauth_logins
            .lock()
            .ok()
            .and_then(|sessions| {
                sessions
                    .get(login_id)
                    .map(|session| session.recovery_ref.is_some())
            })
            .unwrap_or(false)
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
            workspace_name: account.workspace_name.clone(),
            active: active_account_id == Some(account.id.as_str()),
            needs_apply: account.needs_apply,
            sign_in_required: account.sign_in_required,
            revision: account.credential_generation,
        }
    }
}

/// Compares two values by their complete serialized form.
fn same_json<T: serde::Serialize>(left: &T, right: &T) -> bool {
    matches!(
        (serde_json::to_value(left), serde_json::to_value(right)),
        (Ok(left), Ok(right)) if left == right
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;
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
            workspace_name: Some(workspace.into()),
            account_structure: Some("workspace".into()),
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

    fn stable_credential(workspace: &str, access_token: &str) -> Value {
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "https://api.openai.com/auth": {
                    "chatgpt_user_id": "user",
                    "chatgpt_account_id": workspace,
                }
            }))
            .expect("claims"),
        );
        json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": access_token,
                "refresh_token": "refresh-token",
                "id_token": format!("header.{claims}.signature"),
            },
            "unknown_future_field": {"keep": true},
        })
    }

    #[test]
    fn allows_the_same_email_in_different_workspaces() {
        let path = temp_path("different-workspaces");
        let state = AppState::new(path.clone()).expect("state");

        save(
            &state,
            draft("workspace-a", stable_credential("workspace-a", "first")),
        );
        save(
            &state,
            draft("workspace-b", stable_credential("workspace-b", "second")),
        );

        assert_eq!(state.list().expect("list").len(), 2);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn reauthentication_updates_the_existing_profile() {
        let path = temp_path("reauthentication");
        let state = AppState::new(path.clone()).expect("state");

        let first = save(
            &state,
            draft("workspace", stable_credential("workspace", "old")),
        );
        let second = save(
            &state,
            draft("workspace", stable_credential("workspace", "rotated")),
        );

        assert_eq!(first.id, second.id);
        assert_eq!(state.list().expect("list").len(), 1);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn new_sign_in_survives_old_live_reconciliation_and_requires_application() {
        let path = temp_path("new-sign-in-wins");
        let state = AppState::new(path.clone()).expect("state");
        let old = stable_credential("workspace", "old");
        let fresh = stable_credential("workspace", "fresh");
        let first = save(&state, draft("workspace", old.clone()));
        let operation = state.acquire_operation().expect("operation");
        state
            .complete_switch_under_operation(&operation, &first.id)
            .expect("active");
        drop(operation);

        let replacement = save(&state, draft("workspace", fresh.clone()));
        assert_eq!(replacement.id, first.id);
        assert!(replacement.needs_apply);
        assert!(replacement.active);
        let operation = state.acquire_operation().expect("operation");
        state
            .update_credential_under_operation(&operation, &first.id, old, replacement.revision)
            .expect("older live credential is ignored");
        assert_eq!(
            state.account_by_id(&first.id).expect("saved").credential,
            fresh
        );
        state
            .complete_switch_under_operation(&operation, &first.id)
            .expect("applied");
        assert!(!state.list().expect("list")[0].needs_apply);
        drop(operation);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn verified_live_rotation_updates_saved_login_without_pending_apply() {
        let path = temp_path("verified-live-rotation");
        let state = AppState::new(path.clone()).expect("state");
        let first = save(
            &state,
            draft("workspace", stable_credential("workspace", "old")),
        );
        let operation = state.acquire_operation().expect("operation");
        let rotated = state
            .upsert_verified_live_under_operation(
                &operation,
                draft("workspace", stable_credential("workspace", "rotated")),
            )
            .expect("verified live rotation");
        assert_eq!(rotated.id, first.id);
        assert!(!rotated.needs_apply);
        drop(operation);

        let newer = save(
            &state,
            draft("workspace", stable_credential("workspace", "browser-login")),
        );
        assert!(newer.needs_apply);
        let operation = state.acquire_operation().expect("operation");
        assert!(state
            .upsert_verified_live_under_operation(
                &operation,
                draft("workspace", stable_credential("workspace", "old-live")),
            )
            .is_err());
        assert_eq!(
            state.account_by_id(&first.id).expect("saved").credential,
            stable_credential("workspace", "browser-login")
        );
        drop(operation);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn first_saved_browser_login_can_wait_for_live_application() {
        let path = temp_path("new-pending-live-login");
        let state = AppState::new(path.clone()).expect("state");
        let operation = state.acquire_operation().expect("operation");
        let account = state
            .upsert_pending_live_under_operation(
                &operation,
                draft("workspace", stable_credential("workspace", "browser-login")),
            )
            .expect("save pending login");
        assert!(account.needs_apply);
        assert!(state.list().expect("list")[0].needs_apply);
        drop(operation);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn authentication_status_is_generation_scoped() {
        let path = temp_path("auth-generation");
        let state = AppState::new(path.clone()).expect("state");
        let first = save(
            &state,
            draft("workspace", stable_credential("workspace", "old")),
        );
        let operation = state.acquire_operation().expect("operation");
        state
            .mark_sign_in_required(&operation, &first.id, first.revision)
            .expect("mark");
        assert!(state.list().expect("list")[0].sign_in_required);
        drop(operation);
        let fresh = save(
            &state,
            draft("workspace", stable_credential("workspace", "fresh")),
        );
        assert!(!fresh.sign_in_required);
        let operation = state.acquire_operation().expect("operation");
        state
            .mark_sign_in_required(&operation, &first.id, first.revision)
            .expect("old failure");
        assert!(!state.list().expect("list")[0].sign_in_required);
        drop(operation);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn rotation_advances_the_credential_generation_and_keeps_unknown_fields() {
        let path = temp_path("credential-rotation");
        let state = AppState::new(path.clone()).expect("state");
        let first = save(
            &state,
            draft("workspace", stable_credential("workspace", "first-token")),
        );
        let first_generation = state
            .account_by_id(&first.id)
            .expect("first account")
            .credential_generation;

        let second = save(
            &state,
            draft("workspace", stable_credential("workspace", "second-token")),
        );
        let saved = state.account_by_id(&second.id).expect("rotated account");
        assert_eq!(first.id, second.id);
        assert!(saved.credential_generation > first_generation);
        assert_eq!(
            saved.credential.pointer("/unknown_future_field/keep"),
            Some(&Value::Bool(true))
        );
        let on_disk = storage::load(&path).expect("store");
        assert_eq!(on_disk.accounts[0].credential, saved.credential);
        assert_eq!(
            on_disk.accounts[0].credential_generation,
            saved.credential_generation
        );
        assert!(!fs::read_to_string(&path)
            .expect("store")
            .contains("first-token"));

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    fn pending_entries(state: &AppState) -> Vec<(String, PendingCredential)> {
        storage::pending_credentials(&state.pending_dir).expect("pending credentials")
    }

    fn identity_json(credential: &Value) -> Value {
        serde_json::to_value(derive_identity(&AccountKind::ChatGpt, credential).expect("identity"))
            .expect("identity JSON")
    }

    fn write_store(path: &std::path::Path, store: Value) {
        fs::create_dir_all(path.parent().expect("parent")).expect("storage root");
        fs::write(path, serde_json::to_vec_pretty(&store).expect("store")).expect("store file");
    }

    fn legacy_vault(path: &std::path::Path) -> LegacyVault {
        LegacyVault::with_test_root(path.parent().expect("parent").join("credentials.hold"))
    }

    #[test]
    fn a_vault_store_moves_to_the_owner_only_file_and_the_vault_is_removed() {
        let path = temp_path("vault-migration");
        let first = stable_credential("workspace-a", "first-token");
        let second = stable_credential("workspace-b", "second-token");
        let rollback = json!({"tokens": {"access_token": "rollback-token"}, "future": [1]});
        write_store(
            &path,
            json!({
                "version": 4,
                "active_account_id": "a",
                "accounts": [
                    {"id": "a", "label": "A", "kind": "chat_gpt", "identity": identity_json(&first),
                     "credential_ref": "account:a", "credential_generation": 3},
                    {"id": "b", "label": "B", "kind": "chat_gpt", "identity": identity_json(&second),
                     "credential_ref": "account:b", "credential_generation": 1}
                ],
                "pending_switch": {"target_id": "b", "target_identity": identity_json(&second),
                    "previous_active_id": "a", "secret_ref": "pending-switch-x",
                    "secret_generation": 2, "stage": "prepared"},
                "pending_reset_credit": {"account_id": "a", "secret_ref": "pending-reset-credit-x",
                    "secret_generation": 1, "created_at_unix_ms": 123, "discard_allowed": false}
            }),
        );
        let vault = legacy_vault(&path);
        vault
            .put(
                "account:a:3",
                &json!({"credential": first, "reset_credits": {"available_count": 1,
                    "credits": [{"id": "credit-private", "status": "available"}]}}),
            )
            .expect("first account");
        vault
            .put("account:b:1", &json!({"credential": second}))
            .expect("second account");
        vault
            .put("pending-switch-x:2", &json!({"previous_auth": rollback}))
            .expect("switch rollback");
        vault
            .put(
                "pending-reset-credit-x:1",
                &json!({"credit_id": "credit-private", "idempotency_key": "key-private"}),
            )
            .expect("reset credit");
        let reference = format!("pending-credential-{}", Uuid::new_v4());
        vault
            .put_pending_credential(
                &reference,
                &json!({"credential": stable_credential("workspace-a", "queued-token"),
                    "expected_account_id": "a", "expected_generation": 3}),
            )
            .expect("pending credential");

        let state = AppState::new(path.clone()).expect("migrate");
        assert_eq!(state.storage_view().status, StorageStatus::Ready);
        let account = state.account_by_id("a").expect("first account");
        assert_eq!(account.credential, first);
        assert_eq!(account.credential_generation, 3);
        let credits = account.reset_credits.expect("reset credits");
        assert_eq!(
            credits.credits.expect("credit list")[0].id,
            "credit-private"
        );
        assert_eq!(state.account_by_id("b").expect("second").credential, second);
        let operation = state.acquire_operation().expect("operation");
        assert_eq!(
            state
                .pending_switch_under_operation(&operation)
                .expect("switch")
                .expect("pending switch")
                .previous_auth,
            Some(rollback)
        );
        let reset = state
            .pending_reset_credit_under_operation(&operation)
            .expect("reset")
            .expect("pending reset");
        assert_eq!(reset.credit_id, "credit-private");
        assert_eq!(reset.idempotency_key, "key-private");
        drop(operation);
        let pending = pending_entries(&state);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, reference);
        assert_eq!(pending[0].1.expected_account_id.as_deref(), Some("a"));
        assert_eq!(pending[0].1.expected_generation, Some(3));

        let raw = fs::read_to_string(&path).expect("store");
        assert!(!raw.contains("credential_ref") && !raw.contains("secret_ref"));
        assert_eq!(
            storage::load(&path).expect("store").version,
            storage::STORE_VERSION
        );
        assert!(!vault.exists());
        assert!(!path
            .parent()
            .expect("parent")
            .join("credentials.lock")
            .exists());

        drop(state);
        let reopened = AppState::new(path.clone()).expect("reopen");
        assert_eq!(
            reopened.account_by_id("a").expect("first").credential,
            first
        );
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn an_unreadable_vault_leaves_the_vault_store_unchanged_until_it_can_be_read() {
        let path = temp_path("vault-unreadable");
        let credential = stable_credential("workspace", "vault-token");
        write_store(
            &path,
            json!({"version": 4, "accounts": [{"id": "a", "label": "A", "kind": "chat_gpt",
                "identity": identity_json(&credential), "credential_ref": "account:a",
                "credential_generation": 1}]}),
        );
        let original = fs::read(&path).expect("original store");
        let vault_path = path.parent().expect("parent").join("credentials.hold");

        // Without a snapshot the reference cannot be resolved.
        let state = AppState::new(path.clone()).expect("recovery state");
        assert_eq!(state.storage_view().status, StorageStatus::RecoveryRequired);
        assert!(state
            .storage_view()
            .message
            .is_some_and(|message| message.contains("try again the next time it starts")));
        assert!(state.list().expect("blocked view").is_empty());
        assert!(state.acquire_operation().is_err());
        assert_eq!(fs::read(&path).expect("store"), original);
        drop(state);

        // A damaged snapshot cannot be opened either, and is kept.
        fs::write(&vault_path, b"not a snapshot").expect("damaged snapshot");
        let state = AppState::new(path.clone()).expect("recovery state");
        assert_eq!(state.storage_view().status, StorageStatus::RecoveryRequired);
        assert_eq!(fs::read(&path).expect("store"), original);
        assert_eq!(fs::read(&vault_path).expect("vault"), b"not a snapshot");
        drop(state);

        fs::remove_file(&vault_path).expect("replace snapshot");
        legacy_vault(&path)
            .put("account:a:1", &json!({"credential": credential}))
            .expect("readable vault");
        let state = AppState::new(path.clone()).expect("migrated");
        assert_eq!(
            state.account_by_id("a").expect("account").credential,
            credential
        );
        assert!(!vault_path.exists());
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_leftover_vault_beside_a_current_store_is_removed_without_being_read() {
        let path = temp_path("leftover-vault");
        let state = AppState::new(path.clone()).expect("state");
        let saved = save(
            &state,
            draft("workspace", stable_credential("workspace", "current")),
        );
        drop(state);
        let vault_path = path.parent().expect("parent").join("credentials.hold");
        fs::write(&vault_path, b"never read").expect("leftover snapshot");

        let reopened = AppState::new(path.clone()).expect("reopen");
        assert_eq!(reopened.storage_view().status, StorageStatus::Ready);
        assert_eq!(
            reopened
                .account_by_id(&saved.id)
                .expect("account")
                .credential,
            stable_credential("workspace", "current")
        );
        assert!(!vault_path.exists());
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_pre_vault_store_moves_to_the_current_file_and_a_stale_vault_is_removed() {
        let path = temp_path("pre-vault-migration");
        let credential = stable_credential("workspace", "legacy-access-token");
        // An interrupted vault migration could leave a snapshot beside the
        // still-authoritative inline store.
        fs::create_dir_all(path.parent().expect("parent")).expect("storage root");
        legacy_vault(&path)
            .put(
                "account:legacy-account:1",
                &json!({"credential": stable_credential("workspace", "stale")}),
            )
            .expect("stale vault");
        write_store(
            &path,
            json!({
                "version": 3,
                "accounts": [{
                    "id": "legacy-account",
                    "label": "Legacy",
                    "kind": "chat_gpt",
                    "credential": credential,
                }]
            }),
        );

        let first = AppState::new(path.clone()).expect("migrate");
        assert_eq!(
            first
                .account_by_id("legacy-account")
                .expect("migrated account")
                .credential,
            credential
        );
        let saved = storage::load(&path).expect("migrated store");
        assert_eq!(saved.version, storage::STORE_VERSION);
        assert_eq!(saved.accounts[0].credential, credential);
        assert!(saved.accounts[0].identity.is_some());
        assert!(!legacy_vault(&path).exists());
        drop(first);

        let reopened = AppState::new(path.clone()).expect("restart-safe reopen");
        assert_eq!(
            reopened
                .account_by_id("legacy-account")
                .expect("reopened account")
                .credential,
            credential
        );

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn startup_migration_waits_for_the_credential_operation_lock() {
        use std::{sync::mpsc, thread, time::Duration};

        let path = temp_path("serialized-startup-migration");
        let state = AppState::new(path.clone()).expect("state");
        let operation = state.acquire_operation().expect("operation lock");
        let credential = stable_credential("workspace", "legacy-token");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "version": 3,
                "accounts": [{
                    "id": "legacy-account",
                    "label": "Legacy",
                    "kind": "chat_gpt",
                    "credential": credential,
                }]
            }))
            .expect("legacy store"),
        )
        .expect("write legacy store");

        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let reopening_path = path.clone();
        let worker = thread::spawn(move || {
            started_tx.send(()).expect("signal start");
            let reopened = AppState::new(reopening_path).expect("migrate after unlock");
            result_tx
                .send(
                    reopened
                        .account_by_id("legacy-account")
                        .expect("saved")
                        .credential,
                )
                .expect("send result");
        });
        started_rx.recv().expect("worker started");
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(fs::read_to_string(&path)
            .expect("legacy still present")
            .contains("legacy-token"));
        drop(operation);
        assert_eq!(
            result_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("migration completed"),
            credential
        );
        worker.join().expect("worker finished");
        assert_eq!(
            storage::load(&path).expect("store").version,
            storage::STORE_VERSION
        );
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn pre_vault_pending_recovery_files_are_read_in_place() {
        let path = temp_path("legacy-pending-recovery");
        let recovery_dir = path.parent().expect("parent").join("pending-credentials");
        fs::create_dir_all(&recovery_dir).expect("recovery directory");
        let id = Uuid::new_v4();
        let recovery_path = recovery_dir.join(format!("{id}.json"));
        fs::write(
            &recovery_path,
            serde_json::to_vec(&json!({
                "version": 1,
                "credential": stable_credential("workspace", "recovery-only-secret")
            }))
            .expect("json"),
        )
        .expect("write legacy recovery");

        let state = AppState::new(path.clone()).expect("open");
        assert_eq!(state.storage_view().status, StorageStatus::Ready);
        assert_eq!(
            pending_entries(&state)[0].0,
            format!("pending-credential-{id}")
        );
        assert_eq!(
            state
                .recover_pending_credentials()
                .expect("explicit recovery"),
            1
        );
        assert_eq!(state.list().expect("recovered account").len(), 1);
        assert!(!recovery_path.exists());

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn pending_refresh_recovers_only_at_its_recorded_generation() {
        let path = temp_path("pending-refresh-recovery");
        let state = AppState::new(path.clone()).expect("state");
        let first = save(
            &state,
            draft("workspace", stable_credential("workspace", "old")),
        );
        let original = state.account_by_id(&first.id).expect("saved account");
        let refreshed = stable_credential("workspace", "recovered-token");
        let operation = state.acquire_operation().expect("operation");
        state
            .record_pending_credential(&operation, &refreshed)
            .expect("recovery queue");
        drop(operation);

        let reopened = AppState::new(path.clone()).expect("restart");
        assert_eq!(reopened.recover_pending_credentials().expect("recover"), 1);
        let recovered = reopened.account_by_id(&first.id).expect("same account");
        assert_eq!(recovered.credential, refreshed);
        assert!(recovered.credential_generation > original.credential_generation);
        assert_eq!(reopened.list().expect("list").len(), 1);
        assert_eq!(
            reopened.recover_pending_credentials().expect("idempotent"),
            0
        );
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn pending_refresh_never_overwrites_a_newer_generation() {
        let path = temp_path("pending-refresh-stale");
        let state = AppState::new(path.clone()).expect("state");
        let first = save(
            &state,
            draft("workspace", stable_credential("workspace", "old")),
        );
        let operation = state.acquire_operation().expect("operation");
        state
            .record_pending_credential(&operation, &stable_credential("workspace", "queued-token"))
            .expect("recovery queue");
        drop(operation);
        save(
            &state,
            draft("workspace", stable_credential("workspace", "newer-token")),
        );

        assert_eq!(
            state
                .recover_pending_credentials()
                .expect_err("stale must fail"),
            "Credential recovery generation changed"
        );
        assert_eq!(
            state
                .account_by_id(&first.id)
                .expect("unchanged")
                .credential["tokens"]["access_token"],
            "newer-token"
        );
        assert_eq!(pending_entries(&state).len(), 1);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn pending_recovery_keeps_the_entry_if_the_store_commit_fails() {
        let path = temp_path("pending-recovery-commit-failure");
        let state = AppState::new(path.clone()).expect("state");
        let operation = state.acquire_operation().expect("operation");
        state
            .record_pending_credential(
                &operation,
                &stable_credential("workspace", "recovery-token"),
            )
            .expect("recovery queue");
        drop(operation);
        fs::create_dir(&path).expect("block metadata file");

        assert!(state.recover_pending_credentials().is_err());
        assert_eq!(state.list().expect("unchanged library").len(), 0);
        assert_eq!(pending_entries(&state).len(), 1);
        fs::remove_dir(&path).expect("unblock metadata file");
        assert_eq!(state.recover_pending_credentials().expect("retry"), 1);
        assert_eq!(state.list().expect("recovered library").len(), 1);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn pending_recovery_without_a_baseline_refuses_a_different_saved_token() {
        let path = temp_path("pending-recovery-no-baseline");
        let state = AppState::new(path.clone()).expect("state");
        let operation = state.acquire_operation().expect("operation");
        state
            .record_pending_credential(&operation, &stable_credential("workspace", "queued-token"))
            .expect("recovery queue");
        drop(operation);
        let saved = save(
            &state,
            draft("workspace", stable_credential("workspace", "newer-token")),
        );

        assert_eq!(
            state
                .recover_pending_credentials()
                .expect_err("ambiguous token"),
            "Credential recovery has no safe account baseline"
        );
        assert_eq!(
            state.account_by_id(&saved.id).expect("saved").credential["tokens"]["access_token"],
            "newer-token"
        );
        assert_eq!(pending_entries(&state).len(), 1);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn mismatched_refreshed_identity_cannot_be_recovered_as_a_new_account() {
        let path = temp_path("pending-recovery-foreign-identity");
        let state = AppState::new(path.clone()).expect("state");
        let expected = derive_identity(
            &AccountKind::ChatGpt,
            &stable_credential("expected-workspace", "expected-token"),
        )
        .expect("expected identity");
        let operation = state.acquire_operation().expect("operation");
        state
            .record_pending_credential_for_identity(
                &operation,
                &stable_credential("other-workspace", "foreign-token"),
                Some(&expected),
            )
            .expect("recovery queue");
        drop(operation);

        assert_eq!(
            state
                .recover_pending_credentials()
                .expect_err("wrong identity"),
            "Credential recovery identity changed"
        );
        assert!(state.list().expect("no foreign account").is_empty());
        assert_eq!(pending_entries(&state).len(), 1);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_bad_pending_identity_does_not_block_other_safe_recovery_entries() {
        let path = temp_path("pending-recovery-continues");
        let state = AppState::new(path.clone()).expect("state");
        let expected = derive_identity(
            &AccountKind::ChatGpt,
            &stable_credential("expected-workspace", "expected-token"),
        )
        .expect("expected identity");
        let operation = state.acquire_operation().expect("operation");
        state
            .record_pending_credential_for_identity(
                &operation,
                &stable_credential("wrong-workspace", "wrong-token"),
                Some(&expected),
            )
            .expect("unsafe entry retained");
        state
            .record_pending_credential(
                &operation,
                &stable_credential("safe-workspace", "safe-token"),
            )
            .expect("safe entry retained");
        drop(operation);

        assert_eq!(
            state
                .recover_pending_credentials()
                .expect_err("unsafe entry remains"),
            "Credential recovery identity changed"
        );
        assert_eq!(state.list().expect("safe account").len(), 1);
        assert_eq!(pending_entries(&state).len(), 1);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_damaged_pending_entry_blocks_startup_without_emptying_accounts() {
        let path = temp_path("pending-entry-damaged");
        let state = AppState::new(path.clone()).expect("state");
        let first = save(
            &state,
            draft("workspace", stable_credential("workspace", "old")),
        );
        let operation = state.acquire_operation().expect("operation");
        state
            .record_pending_credential(&operation, &stable_credential("workspace", "queued-token"))
            .expect("recovery queue");
        drop(operation);
        let reference = pending_entries(&state)[0].0.clone();
        let entry = state.pending_dir.join(format!(
            "{}.json",
            reference.trim_start_matches("pending-credential-")
        ));
        fs::write(&entry, "{ damaged").expect("damage entry");

        let reopened = AppState::new(path.clone()).expect("recovery state");
        assert_eq!(
            reopened.storage_view().status,
            StorageStatus::RecoveryRequired
        );
        assert_eq!(reopened.list().expect("blocked view").len(), 0);
        assert_eq!(
            storage::load(&path).expect("store kept").accounts[0].id,
            first.id
        );
        assert!(reopened.recover_pending_credentials().is_err());
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn an_unversioned_legacy_store_is_migrated_before_it_can_be_used() {
        let path = temp_path("unversioned-legacy-store");
        fs::create_dir_all(path.parent().expect("parent")).expect("storage root");
        let credential = stable_credential("workspace", "legacy-unversioned-token");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "accounts": [{
                    "id": "legacy-account",
                    "label": "Legacy",
                    "kind": "chat_gpt",
                    "credential": credential,
                }]
            }))
            .expect("legacy JSON"),
        )
        .expect("legacy store");

        let state = AppState::new(path.clone()).expect("migrate legacy store");
        assert_eq!(
            state
                .account_by_id("legacy-account")
                .expect("migrated account")
                .credential,
            credential
        );
        assert_eq!(
            storage::load(&path).expect("migrated metadata").version,
            storage::STORE_VERSION
        );

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_current_account_without_a_matching_credential_fails_closed() {
        let own = stable_credential("workspace", "own");
        for (name, credential) in [
            ("credential-missing", Value::Null),
            (
                "credential-foreign",
                stable_credential("other-workspace", "foreign"),
            ),
        ] {
            let path = temp_path(name);
            write_store(
                &path,
                json!({"version": storage::STORE_VERSION, "accounts": [{"id": "account",
                    "label": "Account", "kind": "chat_gpt", "identity": identity_json(&own),
                    "credential_generation": 1, "credential": credential}]}),
            );
            let state = AppState::new(path.clone()).expect("recovery state");
            assert_eq!(state.storage_view().status, StorageStatus::RecoveryRequired);
            assert!(state.list().expect("blocked view").is_empty());
            let _ = fs::remove_dir_all(path.parent().expect("parent"));
        }
    }

    #[test]
    fn a_failed_write_keeps_the_previous_credential_and_generation() {
        let path = temp_path("store-commit-failure");
        let state = AppState::new(path.clone()).expect("state");
        let first = save(
            &state,
            draft("workspace", stable_credential("workspace", "old-secret")),
        );
        let previous = state.account_by_id(&first.id).expect("saved account");
        fs::remove_file(&path).expect("remove store");
        fs::create_dir(&path).expect("block the store destination");

        let operation = state.acquire_operation().expect("operation");
        assert!(state
            .upsert_under_operation(
                &operation,
                draft("workspace", stable_credential("workspace", "new-secret")),
            )
            .is_err());
        let still_saved = state.account_by_id(&first.id).expect("old account remains");
        assert_eq!(still_saved.credential, previous.credential);
        assert_eq!(
            still_saved.credential_generation,
            previous.credential_generation
        );

        drop(operation);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn switch_rollback_auth_survives_a_restart() {
        let path = temp_path("pending-switch-vault");
        let state = AppState::new(path.clone()).expect("state");
        let previous_auth = json!({"tokens": {"access_token": "rollback-secret"}});
        let operation = state.acquire_operation().expect("operation");
        state
            .prepare_switch_under_operation(
                &operation,
                PendingSwitch {
                    target_id: "target".into(),
                    target_identity: AccountIdentity::ApiKey {
                        fingerprint: "stable-fingerprint".into(),
                    },
                    previous_active_id: None,
                    previous_auth: Some(previous_auth.clone()),
                    stage: crate::types::PendingSwitchStage::Prepared,
                },
            )
            .expect("persist switch transaction");
        drop(operation);
        drop(state);

        let reopened = AppState::new(path.clone()).expect("reopen pending switch");
        let operation = reopened.acquire_operation().expect("recovery operation");
        assert_eq!(
            reopened
                .pending_switch_under_operation(&operation)
                .expect("pending switch")
                .expect("pending")
                .previous_auth,
            Some(previous_auth)
        );

        drop(operation);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn reset_credit_recovery_material_survives_a_restart() {
        let path = temp_path("pending-reset-vault");
        let state = AppState::new(path.clone()).expect("state");
        let operation = state.acquire_operation().expect("operation");
        state
            .prepare_reset_credit_under_operation(
                &operation,
                PendingResetCredit {
                    account_id: "account".into(),
                    credit_id: "provider-private-credit".into(),
                    idempotency_key: "private-idempotency-key".into(),
                    created_at_unix_ms: 123,
                    discard_allowed: false,
                },
            )
            .expect("persist reset transaction");
        drop(operation);
        drop(state);

        let reopened = AppState::new(path.clone()).expect("reopen reset transaction");
        let operation = reopened.acquire_operation().expect("recovery operation");
        let pending = reopened
            .pending_reset_credit_under_operation(&operation)
            .expect("reset state")
            .expect("pending reset");
        assert_eq!(pending.credit_id, "provider-private-credit");
        assert_eq!(pending.idempotency_key, "private-idempotency-key");

        drop(operation);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn rereading_an_unchanged_credit_list_keeps_the_credential_generation() {
        let path = temp_path("unchanged-credit-list");
        let state = AppState::new(path.clone()).expect("state");
        let saved = save(
            &state,
            draft("workspace", stable_credential("workspace", "token")),
        );
        let quota = |fetched_at_unix_ms| crate::types::QuotaSnapshot {
            fetched_at_unix_ms,
            account_id: None,
            ordinary_usage_allowed: None,
            buckets: Vec::new(),
            reset_credits: None,
        };
        let credits = |credit_id: &str, read_at| StoredResetCredits {
            available_count: 1,
            credits: Some(vec![crate::types::StoredResetCredit {
                id: credit_id.into(),
                status: "available".into(),
                expires_at: Some(4_000_000_000),
                reset_type: None,
                granted_at: None,
                title: None,
            }]),
            details_read_at_unix_ms: Some(read_at),
        };
        let operation = state.acquire_operation().expect("operation");
        state
            .update_quota_under_operation(&operation, &saved.id, quota(1), Some(credits("a", 1)))
            .expect("first credit read");
        let generation = state
            .account_by_id(&saved.id)
            .expect("account")
            .credential_generation;

        state
            .update_quota_under_operation(&operation, &saved.id, quota(2), Some(credits("a", 2)))
            .expect("same list read later");
        let account = state.account_by_id(&saved.id).expect("account");
        assert_eq!(account.credential_generation, generation);
        assert_eq!(account.quota.map(|quota| quota.fetched_at_unix_ms), Some(2));
        // The newer read time still spaces out the next detail read.
        assert_eq!(
            account
                .reset_credits
                .and_then(|credits| credits.details_read_at_unix_ms),
            Some(2)
        );

        state
            .update_quota_under_operation(&operation, &saved.id, quota(3), Some(credits("b", 3)))
            .expect("changed list");
        assert!(
            state
                .account_by_id(&saved.id)
                .expect("account")
                .credential_generation
                > generation
        );
        drop(operation);
        drop(state);

        let reopened = AppState::new(path.clone()).expect("reopen");
        let credits = reopened
            .account_by_id(&saved.id)
            .expect("account")
            .reset_credits
            .expect("saved credits");
        assert_eq!(credits.credits.expect("credit list")[0].id, "b");
        assert_eq!(credits.details_read_at_unix_ms, None);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn removing_a_saved_account_does_not_touch_the_live_codex_file() {
        let path = temp_path("remove-account-live-file");
        let state = AppState::new(path.clone()).expect("state");
        let saved = save(
            &state,
            draft(
                "saved-workspace",
                stable_credential("saved-workspace", "saved"),
            ),
        );
        let live_path = path.parent().expect("parent").join("codex-home/auth.json");
        fs::create_dir_all(live_path.parent().expect("codex home")).expect("create Codex home");
        fs::write(&live_path, "live-auth-sentinel").expect("live auth sentinel");

        let operation = state.acquire_operation().expect("operation");
        state
            .remove_under_operation(&operation, &saved.id)
            .expect("remove saved account");
        assert_eq!(
            fs::read_to_string(&live_path).expect("live file remains"),
            "live-auth-sentinel"
        );

        drop(operation);
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
    fn credential_generation_overflow_fails_before_replacing_the_credential() {
        assert!(AppState::next_credential_generation(Some(u64::MAX), 1).is_err());
        assert_eq!(
            AppState::next_credential_generation(Some(4), 1).expect("next"),
            5
        );
        assert_eq!(
            AppState::next_credential_generation(None, 0).expect("initial"),
            1
        );
    }

    #[test]
    fn failed_persistence_does_not_change_in_memory_store() {
        let path = temp_path("failed-save");
        let parent = path.parent().expect("parent");
        let state = AppState::new(path.clone()).expect("state");
        // Startup now creates the parent for its cross-process lock. Block
        // only the metadata destination, after the state has opened.
        fs::create_dir(&path).expect("block metadata destination");

        let operation = state.acquire_operation().expect("operation");
        assert!(state
            .upsert_under_operation(
                &operation,
                draft("workspace", stable_credential("workspace", "fixture-token")),
            )
            .is_err());
        assert!(state.list().expect("list").is_empty());
        drop(operation);
        let _ = fs::remove_dir_all(parent);
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
        let first = save(
            &state,
            draft("workspace-a", stable_credential("workspace-a", "first")),
        );
        let second = save(
            &state,
            draft("workspace-b", stable_credential("workspace-b", "second")),
        );

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
            alternate_model: false,
            current_account_id: None,
            results: Vec::new(),
        };
        state
            .insert_wake_operation(first.clone(), cancelled.clone())
            .expect("first wake");

        let second = WakeOperationView {
            id: "second".to_string(),
            status: WakeOperationStatus::Running,
            alternate_model: false,
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

    #[test]
    fn accepted_oauth_completion_cannot_be_cancelled_by_closing_the_browser() {
        let path = temp_path("oauth-accepted");
        let state = AppState::new(path.clone()).expect("state");
        let cancelled = Arc::new(AtomicBool::new(false));
        state
            .start_oauth_login(
                "accepted".into(),
                "https://auth.openai.com/example".into(),
                cancelled.clone(),
            )
            .expect("start");
        assert!(state.accept_oauth_completion("accepted").expect("accept"));
        state.cancel_oauth("accepted").expect("late cancel");
        assert!(!cancelled.load(Ordering::SeqCst));
        assert!(matches!(
            state.oauth_status("accepted").expect("status"),
            OAuthLoginStatus::Finishing
        ));
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }
}
