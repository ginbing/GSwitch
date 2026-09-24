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
        AccountIdentity, AccountKind, AccountSecret, AccountView, OAuthLoginStatus,
        PendingResetCredit, PendingResetCreditSecret, PendingSwitch, PendingSwitchSecret,
        StorageStatus, StorageView, StoredAccount, StoredResetCredits, WakeOperationStatus,
        WakeOperationView,
    },
    vault::CredentialVault,
};

#[derive(Clone)]
pub struct AppState {
    store_path: Arc<PathBuf>,
    store_recovery_dir: Arc<PathBuf>,
    vault: CredentialVault,
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

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct PendingCredentialSecret {
    credential: Value,
    #[serde(default)]
    expected_identity: Option<AccountIdentity>,
    #[serde(default)]
    expected_account_id: Option<String>,
    #[serde(default)]
    expected_generation: Option<u64>,
}

impl AppState {
    pub fn new(store_path: PathBuf) -> Result<Self, String> {
        let storage_root = store_path
            .parent()
            .ok_or_else(|| "Invalid GSwitch storage path".to_string())?
            .to_path_buf();
        #[cfg(test)]
        let vault = CredentialVault::with_test_root(storage_root.join("credentials.hold"));
        #[cfg(not(test))]
        let vault = CredentialVault::new(storage_root.join("credentials.hold"));
        let (store, store_recovery_required, store_recovery_message) =
            match Self::load_protected_store_serialized(&store_path, &vault) {
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

        Ok(Self {
            store_path: Arc::new(store_path),
            store_recovery_dir: Arc::new(storage_root.join("damaged-account-stores")),
            vault,
            store: Arc::new(Mutex::new(store)),
            store_recovery_required: Arc::new(AtomicBool::new(store_recovery_required)),
            store_recovery_message: Arc::new(Mutex::new(store_recovery_message)),
            operation_lock: Arc::new(Mutex::new(())),
            oauth_logins: Arc::new(Mutex::new(HashMap::new())),
            wake_operations: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn recovery_message(error: &str) -> String {
        if error.starts_with("Legacy") {
            "GSwitch could not complete protected credential migration. The legacy account library was left unchanged."
                .to_string()
        } else if error.contains("protected credential")
            || error.contains("Protected credential")
            || error.starts_with("Protected switch")
            || error.starts_with("Protected reset-credit")
        {
            "GSwitch could not open protected credential storage. Saved accounts remain unavailable until it can be opened safely."
                .to_string()
        } else {
            "GSwitch could not safely read its saved account library. Codex credentials were not changed."
                .to_string()
        }
    }

    fn load_protected_store_serialized(
        store_path: &std::path::Path,
        vault: &CredentialVault,
    ) -> Result<AccountStore, String> {
        let parent = store_path
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
        // The file lock spans metadata load, every secret migration/readback,
        // and the atomic version-4 metadata commit. Closing it also unlocks.
        Self::load_protected_store(store_path, vault)
    }

    fn load_protected_store(
        store_path: &std::path::Path,
        vault: &CredentialVault,
    ) -> Result<AccountStore, String> {
        let mut store = storage::load(store_path)?;
        if store.version < storage::STORE_VERSION {
            Self::migrate_legacy_store(store_path, vault, &mut store)?;
        } else {
            Self::hydrate_store_secrets(vault, &mut store)?;
        }
        Self::migrate_legacy_pending_credentials(store_path, vault)?;
        for (_, pending) in vault.pending_credentials::<PendingCredentialSecret>()? {
            let kind = document_kind(&pending.credential)?;
            let _ = derive_identity(&kind, &pending.credential)?;
            if pending.expected_account_id.is_some() != pending.expected_generation.is_some() {
                return Err("Protected credential recovery is incomplete".to_string());
            }
        }
        Ok(store)
    }

    fn migrate_legacy_pending_credentials(
        store_path: &std::path::Path,
        vault: &CredentialVault,
    ) -> Result<(), String> {
        let recovery_dir = store_path
            .parent()
            .ok_or_else(|| "Invalid GSwitch storage path".to_string())?
            .join("pending-credentials");
        let entries = match fs::read_dir(&recovery_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("Legacy credential recovery could not be read safely".into()),
        };

        for entry in entries {
            let entry = entry
                .map_err(|_| "Legacy credential recovery could not be read safely".to_string())?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| "Legacy credential recovery could not be read safely".to_string())?;
            if !metadata.file_type().is_file() {
                return Err("Legacy credential recovery could not be verified safely".into());
            }
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| Uuid::parse_str(stem).ok())
                .ok_or_else(|| {
                    "Legacy credential recovery could not be verified safely".to_string()
                })?;
            #[derive(serde::Deserialize)]
            struct LegacyPendingCredential {
                version: u32,
                credential: Value,
            }
            let content = fs::read(&path)
                .map_err(|_| "Legacy credential recovery could not be read safely".to_string())?;
            let pending: LegacyPendingCredential = serde_json::from_slice(&content)
                .map_err(|_| "Legacy credential recovery is incomplete".to_string())?;
            if pending.version != 1 || !pending.credential.is_object() {
                return Err("Legacy credential recovery is incomplete".into());
            }
            let reference = format!("pending-credential-{id}");
            let secret = PendingCredentialSecret {
                credential: pending.credential,
                expected_identity: None,
                expected_account_id: None,
                expected_generation: None,
            };
            vault.put_pending_credential(&reference, &secret)?;
            let stored: PendingCredentialSecret = vault.get(&reference)?;
            if stored.credential != secret.credential {
                return Err("Legacy credential recovery could not be verified safely".into());
            }
            fs::remove_file(&path).map_err(|_| {
                "Legacy credential recovery could not be retired safely".to_string()
            })?;
        }
        let _ = fs::remove_dir(&recovery_dir);
        Ok(())
    }

    fn migrate_legacy_store(
        store_path: &std::path::Path,
        vault: &CredentialVault,
        store: &mut AccountStore,
    ) -> Result<(), String> {
        for account in &mut store.accounts {
            if account.credential.is_null() {
                return Err("Legacy account storage is incomplete".to_string());
            }
            let kind = document_kind(&account.credential)?;
            let identity = derive_identity(&kind, &account.credential)?;
            if account.kind != kind
                || account
                    .identity
                    .as_ref()
                    .is_some_and(|saved| saved != &identity)
            {
                return Err("Legacy account storage could not be verified safely".to_string());
            }
            account.identity = Some(identity);
            account.credential_ref = format!("account:{}", account.id);
            account.credential_generation = 1;
            Self::write_account_secret(
                vault,
                account,
                AccountSecret {
                    credential: account.credential.clone(),
                    reset_credits: account.reset_credits.clone(),
                },
            )?;
        }
        if let Some(pending) = store.pending_switch.as_mut() {
            if pending.previous_auth.is_some() {
                pending.secret_ref = Some("pending-switch".to_string());
                pending.secret_generation = 1;
                let secret = PendingSwitchSecret {
                    previous_auth: pending.previous_auth.clone(),
                };
                let reference = Self::pending_switch_secret_key(pending);
                vault.put(&reference, &secret)?;
                let saved: PendingSwitchSecret = vault.get(&reference)?;
                if saved.previous_auth != secret.previous_auth {
                    return Err("Legacy switch recovery could not be verified safely".into());
                }
            }
        }
        if let Some(pending) = store.pending_reset_credit.as_mut() {
            if pending.credit_id.is_empty() || pending.idempotency_key.is_empty() {
                return Err("Legacy reset-credit recovery is incomplete".to_string());
            }
            pending.secret_ref = Some("pending-reset-credit".to_string());
            pending.secret_generation = 1;
            let secret = PendingResetCreditSecret {
                credit_id: pending.credit_id.clone(),
                idempotency_key: pending.idempotency_key.clone(),
            };
            let reference = Self::pending_reset_secret_key(pending);
            vault.put(&reference, &secret)?;
            let saved: PendingResetCreditSecret = vault.get(&reference)?;
            if saved.credit_id != secret.credit_id
                || saved.idempotency_key != secret.idempotency_key
            {
                return Err("Legacy reset-credit recovery could not be verified safely".into());
            }
        }
        store.version = storage::STORE_VERSION;
        storage::save_atomic(store_path, store)?;
        for account in &mut store.accounts {
            account.credential = Value::Null;
            account.reset_credits = None;
        }
        if let Some(pending) = store.pending_switch.as_mut() {
            pending.previous_auth = None;
        }
        if let Some(pending) = store.pending_reset_credit.as_mut() {
            pending.credit_id.clear();
            pending.idempotency_key.clear();
        }
        Self::hydrate_store_secrets(vault, store)
    }

    fn hydrate_store_secrets(
        vault: &CredentialVault,
        store: &mut AccountStore,
    ) -> Result<(), String> {
        for account in &mut store.accounts {
            if !account.credential.is_null() || account.reset_credits.is_some() {
                return Err(
                    "Protected credential metadata contains inline secret material".to_string(),
                );
            }
            if account.credential_ref.is_empty() || account.credential_generation == 0 {
                return Err("A saved account is missing protected credential material".to_string());
            }
            let secret: AccountSecret = vault.get(&Self::account_secret_key(account))?;
            let kind = document_kind(&secret.credential)?;
            let identity = derive_identity(&kind, &secret.credential)?;
            if account.kind != kind || account.identity.as_ref() != Some(&identity) {
                return Err(
                    "Protected credential material does not match its account metadata".to_string(),
                );
            }
            account.credential = secret.credential;
            account.reset_credits = secret.reset_credits;
        }
        if let Some(pending) = store.pending_switch.as_mut() {
            match pending.secret_ref.as_deref() {
                Some(reference) => {
                    if pending.secret_generation == 0 || pending.previous_auth.is_some() {
                        return Err("Protected switch recovery is incomplete".to_string());
                    }
                    let secret: PendingSwitchSecret =
                        vault.get(&format!("{}:{}", reference, pending.secret_generation))?;
                    pending.previous_auth = secret.previous_auth;
                }
                None if pending.previous_auth.is_some() || pending.secret_generation != 0 => {
                    return Err("Protected switch recovery is incomplete".to_string());
                }
                None => {}
            }
        }
        if let Some(pending) = store.pending_reset_credit.as_mut() {
            if !pending.credit_id.is_empty() || !pending.idempotency_key.is_empty() {
                return Err(
                    "Protected reset-credit metadata contains inline secret material".into(),
                );
            }
            let reference = pending
                .secret_ref
                .as_deref()
                .ok_or_else(|| "Protected reset-credit recovery is incomplete".to_string())?;
            if pending.secret_generation == 0 {
                return Err("Protected reset-credit recovery is incomplete".to_string());
            }
            let secret: PendingResetCreditSecret =
                vault.get(&format!("{}:{}", reference, pending.secret_generation))?;
            pending.credit_id = secret.credit_id;
            pending.idempotency_key = secret.idempotency_key;
        }
        Ok(())
    }

    fn account_secret_key(account: &StoredAccount) -> String {
        format!(
            "{}:{}",
            account.credential_ref, account.credential_generation
        )
    }

    fn next_secret_generation(previous: Option<u64>, initial: u64) -> Result<u64, String> {
        match previous {
            Some(generation) => generation
                .checked_add(1)
                .ok_or_else(|| "Protected credential generation is exhausted".to_string()),
            None => Ok(initial.max(1)),
        }
    }

    fn write_account_secret(
        vault: &CredentialVault,
        account: &StoredAccount,
        secret: AccountSecret,
    ) -> Result<(), String> {
        let reference = Self::account_secret_key(account);
        vault.put(&reference, &secret)?;
        let stored: AccountSecret = vault.get(&reference)?;
        let kind = document_kind(&stored.credential)?;
        let identity = derive_identity(&kind, &stored.credential)?;
        if stored.credential != secret.credential
            || stored.reset_credits != secret.reset_credits
            || account.identity.as_ref() != Some(&identity)
            || account.kind != kind
        {
            return Err("Protected credential write could not be verified safely".into());
        }
        Ok(())
    }

    fn pending_switch_secret_key(pending: &PendingSwitch) -> String {
        format!(
            "{}:{}",
            pending.secret_ref.as_deref().unwrap_or_default(),
            pending.secret_generation
        )
    }

    fn pending_reset_secret_key(pending: &PendingResetCredit) -> String {
        format!(
            "{}:{}",
            pending.secret_ref.as_deref().unwrap_or_default(),
            pending.secret_generation
        )
    }

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
                if account.credential_ref.is_empty() {
                    account.credential_ref = format!("account:{}", account.id);
                }
                account.credential_generation = Self::next_secret_generation(
                    previous.map(|saved| saved.credential_generation),
                    account.credential_generation,
                )?;
                Self::write_account_secret(
                    &self.vault,
                    account,
                    AccountSecret {
                        credential: account.credential.clone(),
                        reset_credits: account.reset_credits.clone(),
                    },
                )?;
            }
        }

        if let Some(pending) = candidate.pending_switch.as_mut() {
            let previous = current.pending_switch.as_ref();
            let changed = previous.is_none_or(|saved| saved.previous_auth != pending.previous_auth);
            if changed && pending.previous_auth.is_some() {
                if pending.secret_ref.is_none() {
                    pending.secret_ref = Some(format!("pending-switch-{}", Uuid::new_v4()));
                }
                pending.secret_generation = Self::next_secret_generation(
                    previous.map(|saved| saved.secret_generation),
                    pending.secret_generation,
                )?;
                let secret = PendingSwitchSecret {
                    previous_auth: pending.previous_auth.clone(),
                };
                let reference = Self::pending_switch_secret_key(pending);
                self.vault.put(&reference, &secret)?;
                let saved: PendingSwitchSecret = self.vault.get(&reference)?;
                if saved.previous_auth != secret.previous_auth {
                    return Err("Protected switch recovery could not be verified safely".into());
                }
            }
        }

        if let Some(pending) = candidate.pending_reset_credit.as_mut() {
            let previous = current.pending_reset_credit.as_ref();
            let changed = previous.is_none_or(|saved| {
                saved.credit_id != pending.credit_id
                    || saved.idempotency_key != pending.idempotency_key
            });
            if changed {
                if pending.secret_ref.is_none() {
                    pending.secret_ref = Some(format!("pending-reset-credit-{}", Uuid::new_v4()));
                }
                pending.secret_generation = Self::next_secret_generation(
                    previous.map(|saved| saved.secret_generation),
                    pending.secret_generation,
                )?;
                let secret = PendingResetCreditSecret {
                    credit_id: pending.credit_id.clone(),
                    idempotency_key: pending.idempotency_key.clone(),
                };
                let reference = Self::pending_reset_secret_key(pending);
                self.vault.put(&reference, &secret)?;
                let saved: PendingResetCreditSecret = self.vault.get(&reference)?;
                if saved.credit_id != secret.credit_id
                    || saved.idempotency_key != secret.idempotency_key
                {
                    return Err(
                        "Protected reset-credit recovery could not be verified safely".into(),
                    );
                }
            }
        }

        storage::save_atomic(&self.store_path, candidate)?;

        // Metadata is authoritative for reachability. Retire only after the
        // metadata commit; failure leaves encrypted orphaned material rather
        // than an account that points at a missing credential.
        for (previous, next) in current.accounts.iter().filter_map(|saved| {
            candidate
                .accounts
                .iter()
                .find(|account| account.id == saved.id)
                .map(|account| (saved, account))
        }) {
            if previous.credential_generation != next.credential_generation {
                let _ = self.vault.delete(&Self::account_secret_key(previous));
            }
        }
        for removed in current.accounts.iter().filter(|saved| {
            !candidate
                .accounts
                .iter()
                .any(|account| account.id == saved.id)
        }) {
            let _ = self.vault.delete(&Self::account_secret_key(removed));
        }
        if let Some(previous) = current.pending_switch.as_ref() {
            let still_referenced = candidate.pending_switch.as_ref().is_some_and(|next| {
                next.secret_ref == previous.secret_ref
                    && next.secret_generation == previous.secret_generation
            });
            if !still_referenced {
                if let Some(reference) = previous.secret_ref.as_deref() {
                    let _ = self
                        .vault
                        .delete(&format!("{reference}:{}", previous.secret_generation));
                }
            }
        }
        if let Some(previous) = current.pending_reset_credit.as_ref() {
            let still_referenced = candidate.pending_reset_credit.as_ref().is_some_and(|next| {
                next.secret_ref == previous.secret_ref
                    && next.secret_generation == previous.secret_generation
            });
            if !still_referenced {
                if let Some(reference) = previous.secret_ref.as_deref() {
                    let _ = self
                        .vault
                        .delete(&format!("{reference}:{}", previous.secret_generation));
                }
            }
        }
        Ok(())
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
            .map_err(|error| error.to_string())
    }

    /// Updating the installed CLI must not overlap a GSwitch operation that
    /// may launch App Server. This remains available when account storage
    /// needs recovery because it never reads or changes saved accounts.
    pub(crate) fn acquire_cli_update_operation(
        &self,
    ) -> Result<OperationGuard<'_>, OperationAcquireFailure> {
        self.acquire_operation_lock()
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
                workspace_name: draft
                    .workspace_name
                    .or_else(|| existing.workspace_name.clone()),
                account_structure: draft
                    .account_structure
                    .or_else(|| existing.account_structure.clone()),
                identity: Some(draft.identity),
                // Reauthentication invalidates a prior capacity snapshot.
                quota: None,
                reset_credits: None,
                credential_ref: existing.credential_ref.clone(),
                credential_generation: existing.credential_generation,
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
                credential_ref: String::new(),
                credential_generation: 0,
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
        self.persist_candidate(&store, &mut candidate)?;
        *store = candidate;
        Ok(())
    }

    /// Commits a verified provider projection and, only when authentication
    /// required it, a refreshed credential in one account-store replacement.
    pub fn update_switch_validation_under_operation(
        &self,
        _operation: &OperationGuard<'_>,
        id: &str,
        metadata: &AccountMetadata,
        credential: Option<Value>,
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
        let account = &mut candidate.accounts[index];
        account.email = metadata.email.clone();
        account.plan_type = metadata.plan_type.clone();
        if metadata.workspace_name.is_some() {
            account.workspace_name = metadata.workspace_name.clone();
        }
        if metadata.account_structure.is_some() {
            account.account_structure = metadata.account_structure.clone();
        }
        if let Some(credential) = credential {
            account.credential = credential;
            account.quota = None;
            account.reset_credits = None;
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
        let account = store
            .accounts
            .iter()
            .find(|account| account.id == id)
            .cloned()
            .ok_or_else(|| "The selected account is no longer saved".to_string())?;
        let mut candidate = store.clone();
        candidate.active_account_id = Some(id.to_string());
        candidate.pending_switch = None;
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

    /// Retains an interrupted refreshed credential only in the protected vault.
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
        let reference = format!("pending-credential-{}", Uuid::new_v4());
        let secret = PendingCredentialSecret {
            credential: credential.clone(),
            expected_identity: expected_identity.cloned(),
            expected_account_id: existing.map(|account| account.id.clone()),
            expected_generation: existing.map(|account| account.credential_generation),
        };
        drop(store);
        self.vault.put_pending_credential(&reference, &secret)?;
        let saved: PendingCredentialSecret = self.vault.get(&reference)?;
        if saved.credential != secret.credential
            || saved.expected_identity != secret.expected_identity
            || saved.expected_account_id != secret.expected_account_id
            || saved.expected_generation != secret.expected_generation
        {
            return Err("Protected credential recovery could not be verified safely".into());
        }
        Ok(())
    }

    /// Explicit, Rust-only recovery of a credential whose previous metadata
    /// commit failed. No vault reference or credential crosses Tauri IPC.
    /// A changed account generation is never overwritten by an older queue
    /// entry; retirement follows a successful metadata commit.
    pub fn recover_pending_credentials(&self) -> Result<u32, String> {
        let operation = self.acquire_operation()?;
        let pending = self
            .vault
            .pending_credentials::<PendingCredentialSecret>()?;
        let mut recovered = 0u32;
        let mut first_error = None;
        for (reference, secret) in pending {
            let result: Result<(), String> = (|| {
                let kind = document_kind(&secret.credential)?;
                let identity = derive_identity(&kind, &secret.credential)?;
                if secret
                    .expected_identity
                    .as_ref()
                    .is_some_and(|expected| expected != &identity)
                {
                    return Err("Protected credential recovery identity changed".to_string());
                }
                if secret.expected_account_id.is_some() != secret.expected_generation.is_some() {
                    return Err("Protected credential recovery is incomplete".to_string());
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
                                return Err(
                                    "Protected credential recovery account changed".to_string()
                                );
                            }
                            if account.credential != secret.credential {
                                if Some(account.credential_generation) != secret.expected_generation
                                {
                                    return Err("Protected credential recovery generation changed"
                                        .to_string());
                                }
                                let mut candidate = store.clone();
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
                                "Protected credential recovery has no safe account baseline"
                                    .to_string(),
                            );
                        }
                        (None, Some(_)) => {
                            return Err("Protected credential recovery account is no longer saved"
                                .to_string());
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
                self.vault.retire_pending_credential(&reference)?;
                Ok(())
            })();
            match result {
                Ok(()) => {
                    recovered = recovered.checked_add(1).ok_or_else(|| {
                        "Protected credential recovery count overflow".to_string()
                    })?;
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
            workspace_name: account.workspace_name.clone(),
            active: active_account_id == Some(account.id.as_str()),
        }
    }
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
    fn rotation_advances_the_secret_generation_without_serializing_tokens() {
        let path = temp_path("vault-rotation");
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
        let metadata = fs::read_to_string(&path).expect("metadata");
        assert!(!metadata.contains("first-token"));
        assert!(!metadata.contains("second-token"));
        assert!(!metadata.contains("refresh-token"));
        assert!(state
            .vault
            .get::<AccountSecret>(&format!("account:{}:{first_generation}", first.id))
            .is_err());

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn legacy_inline_store_migrates_once_and_reopens_from_the_vault() {
        let path = temp_path("legacy-vault-migration");
        fs::create_dir_all(path.parent().expect("parent")).expect("storage root");
        let credential = stable_credential("workspace", "legacy-access-token");
        let interrupted_vault = CredentialVault::with_test_root(
            path.parent().expect("parent").join("credentials.hold"),
        );
        interrupted_vault
            .put(
                "account:legacy-account:1",
                &AccountSecret {
                    credential: credential.clone(),
                    reset_credits: None,
                },
            )
            .expect("simulate a prior interrupted migration after secret commit");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "version": 3,
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

        let first = AppState::new(path.clone()).expect("migrate");
        assert_eq!(
            first
                .account_by_id("legacy-account")
                .expect("migrated account")
                .credential,
            credential
        );
        let metadata = fs::read_to_string(&path).expect("migrated metadata");
        assert!(!metadata.contains("legacy-access-token"));
        assert!(!metadata.contains("refresh-token"));
        assert!(path
            .parent()
            .expect("parent")
            .join("credentials.hold")
            .exists());

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
        assert!(!fs::read_to_string(&path)
            .expect("metadata")
            .contains("legacy-token"));
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn unavailable_vault_does_not_overwrite_the_only_legacy_credential_copy() {
        let path = temp_path("legacy-vault-unavailable");
        fs::create_dir_all(path.parent().expect("parent")).expect("storage root");
        let credential = stable_credential("workspace", "legacy-preserved-token");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "version": 3,
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
        fs::create_dir(path.parent().expect("parent").join("credentials.hold"))
            .expect("block vault snapshot");

        let state = AppState::new(path.clone()).expect("recovery state");
        assert_eq!(state.storage_view().status, StorageStatus::RecoveryRequired);
        let original = fs::read_to_string(&path).expect("original legacy library remains");
        assert!(original.contains("legacy-preserved-token"));
        assert!(original.contains("\"version\": 3"));

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn pending_recovery_files_move_into_the_vault_before_the_plaintext_is_removed() {
        let path = temp_path("legacy-pending-recovery");
        let recovery_dir = path.parent().expect("parent").join("pending-credentials");
        fs::create_dir_all(&recovery_dir).expect("recovery directory");
        let id = Uuid::new_v4();
        let credential = stable_credential("workspace", "recovery-only-secret");
        let recovery_path = recovery_dir.join(format!("{id}.json"));
        fs::write(
            &recovery_path,
            serde_json::to_vec(&json!({"version": 1, "credential": credential})).expect("json"),
        )
        .expect("write legacy recovery");

        let state = AppState::new(path.clone()).expect("migrate recovery");
        assert!(!recovery_path.exists());
        let saved: PendingCredentialSecret = state
            .vault
            .get(&format!("pending-credential-{id}"))
            .expect("protected recovery");
        assert_eq!(
            saved.credential["tokens"]["access_token"],
            "recovery-only-secret"
        );
        let snapshot = fs::read(path.parent().expect("parent").join("credentials.hold"))
            .expect("encrypted vault");
        assert!(!String::from_utf8_lossy(&snapshot).contains("recovery-only-secret"));
        assert_eq!(
            state
                .recover_pending_credentials()
                .expect("explicit recovery"),
            1
        );
        assert_eq!(state.list().expect("recovered account").len(), 1);
        assert!(state
            .vault
            .pending_credentials::<PendingCredentialSecret>()
            .expect("retired queue")
            .is_empty());

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn protected_pending_refresh_recovers_only_at_its_recorded_generation() {
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
            .expect("protected queue");
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
        assert!(!fs::read_to_string(&path)
            .expect("metadata")
            .contains("recovered-token"));
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn protected_pending_refresh_never_overwrites_a_newer_generation() {
        let path = temp_path("pending-refresh-stale");
        let state = AppState::new(path.clone()).expect("state");
        let first = save(
            &state,
            draft("workspace", stable_credential("workspace", "old")),
        );
        let operation = state.acquire_operation().expect("operation");
        state
            .record_pending_credential(&operation, &stable_credential("workspace", "queued-token"))
            .expect("protected queue");
        drop(operation);
        save(
            &state,
            draft("workspace", stable_credential("workspace", "newer-token")),
        );

        assert_eq!(
            state
                .recover_pending_credentials()
                .expect_err("stale must fail"),
            "Protected credential recovery generation changed"
        );
        assert_eq!(
            state
                .account_by_id(&first.id)
                .expect("unchanged")
                .credential["tokens"]["access_token"],
            "newer-token"
        );
        assert_eq!(
            state
                .vault
                .pending_credentials::<PendingCredentialSecret>()
                .expect("still recoverable")
                .len(),
            1
        );
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn pending_recovery_keeps_the_encrypted_entry_if_metadata_commit_fails() {
        let path = temp_path("pending-recovery-commit-failure");
        let state = AppState::new(path.clone()).expect("state");
        let operation = state.acquire_operation().expect("operation");
        state
            .record_pending_credential(
                &operation,
                &stable_credential("workspace", "recovery-token"),
            )
            .expect("protected queue");
        drop(operation);
        fs::create_dir(&path).expect("block metadata file");

        assert!(state.recover_pending_credentials().is_err());
        assert_eq!(state.list().expect("unchanged library").len(), 0);
        assert_eq!(
            state
                .vault
                .pending_credentials::<PendingCredentialSecret>()
                .expect("preserved queue")
                .len(),
            1
        );
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
            .expect("protected queue");
        drop(operation);
        let saved = save(
            &state,
            draft("workspace", stable_credential("workspace", "newer-token")),
        );

        assert_eq!(
            state
                .recover_pending_credentials()
                .expect_err("ambiguous token"),
            "Protected credential recovery has no safe account baseline"
        );
        assert_eq!(
            state.account_by_id(&saved.id).expect("saved").credential["tokens"]["access_token"],
            "newer-token"
        );
        assert_eq!(
            state
                .vault
                .pending_credentials::<PendingCredentialSecret>()
                .expect("preserved queue")
                .len(),
            1
        );
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
            .expect("protected queue");
        drop(operation);

        assert_eq!(
            state
                .recover_pending_credentials()
                .expect_err("wrong identity"),
            "Protected credential recovery identity changed"
        );
        assert!(state.list().expect("no foreign account").is_empty());
        assert_eq!(
            state
                .vault
                .pending_credentials::<PendingCredentialSecret>()
                .expect("retained for investigation")
                .len(),
            1
        );
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
            "Protected credential recovery identity changed"
        );
        assert_eq!(state.list().expect("safe account").len(), 1);
        assert_eq!(
            state
                .vault
                .pending_credentials::<PendingCredentialSecret>()
                .expect("only unsafe entry remains")
                .len(),
            1
        );
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn missing_pending_secret_blocks_recovery_without_emptying_accounts() {
        let path = temp_path("pending-secret-missing");
        let state = AppState::new(path.clone()).expect("state");
        let first = save(
            &state,
            draft("workspace", stable_credential("workspace", "old")),
        );
        let operation = state.acquire_operation().expect("operation");
        state
            .record_pending_credential(&operation, &stable_credential("workspace", "queued-token"))
            .expect("protected queue");
        drop(operation);
        let reference = state
            .vault
            .pending_credentials::<PendingCredentialSecret>()
            .expect("queue")[0]
            .0
            .clone();
        state
            .vault
            .delete(&reference)
            .expect("simulate missing secret");

        let reopened = AppState::new(path.clone()).expect("recovery state");
        assert_eq!(
            reopened.storage_view().status,
            StorageStatus::RecoveryRequired
        );
        assert_eq!(reopened.list().expect("blocked view").len(), 0);
        assert_eq!(
            state
                .account_by_id(&first.id)
                .expect("old state")
                .credential["tokens"]["access_token"],
            "old"
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
        assert!(!fs::read_to_string(&path)
            .expect("metadata")
            .contains("legacy-unversioned-token"));

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn missing_vault_never_becomes_an_empty_account_library() {
        let path = temp_path("missing-vault");
        let state = AppState::new(path.clone()).expect("state");
        save(
            &state,
            draft("workspace", stable_credential("workspace", "access-token")),
        );
        fs::remove_file(path.parent().expect("parent").join("credentials.hold"))
            .expect("remove vault");

        let reopened = AppState::new(path.clone()).expect("recovery state");
        assert_eq!(
            reopened.storage_view().status,
            StorageStatus::RecoveryRequired
        );
        assert!(reopened
            .storage_view()
            .message
            .as_deref()
            .is_some_and(|message| message.contains("protected credential storage")));
        assert!(reopened.list().expect("recovery view").is_empty());
        assert!(reopened.acquire_operation().is_err());

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn inline_switch_recovery_without_a_vault_reference_fails_closed() {
        let path = temp_path("inline-switch-recovery");
        fs::create_dir_all(path.parent().expect("parent")).expect("storage root");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "version": storage::STORE_VERSION,
                "accounts": [],
                "pending_switch": {
                    "target_id": "target",
                    "target_identity": {"type": "api_key", "fingerprint": "stable"},
                    "previous_active_id": null,
                    "previous_auth": {"tokens": {"access_token": "inline-secret"}},
                    "stage": "prepared"
                }
            }))
            .expect("metadata"),
        )
        .expect("write metadata");

        let state = AppState::new(path.clone()).expect("recovery state");
        assert_eq!(state.storage_view().status, StorageStatus::RecoveryRequired);
        assert!(state
            .storage_view()
            .message
            .unwrap()
            .contains("protected credential"));

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn inline_account_credentials_in_current_metadata_fail_closed() {
        let path = temp_path("inline-account-secret");
        fs::create_dir_all(path.parent().expect("parent")).expect("storage root");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "version": storage::STORE_VERSION,
                "accounts": [{
                    "id": "account",
                    "label": "Account",
                    "kind": "chat_gpt",
                    "credential": {"tokens": {"access_token": "inline-account-secret"}}
                }]
            }))
            .expect("metadata"),
        )
        .expect("write metadata");

        let state = AppState::new(path.clone()).expect("recovery state");
        assert_eq!(state.storage_view().status, StorageStatus::RecoveryRequired);
        assert!(state
            .storage_view()
            .message
            .unwrap()
            .contains("protected credential"));

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_vault_write_failure_leaves_metadata_and_memory_unchanged() {
        let path = temp_path("vault-write-failure");
        let state = AppState::new(path.clone()).expect("state");
        fs::create_dir_all(path.parent().expect("parent")).expect("storage root");
        fs::create_dir(path.parent().expect("parent").join("credentials.hold"))
            .expect("block vault snapshot");

        let operation = state.acquire_operation().expect("operation");
        let result = state.upsert_under_operation(
            &operation,
            draft("workspace", stable_credential("workspace", "not-saved")),
        );
        assert!(result.is_err());
        assert!(state.list().expect("in-memory list").is_empty());
        assert!(!path.exists());

        drop(operation);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn metadata_commit_failure_preserves_the_old_secret_generation() {
        let path = temp_path("metadata-commit-failure");
        let state = AppState::new(path.clone()).expect("state");
        let first = save(
            &state,
            draft("workspace", stable_credential("workspace", "old-secret")),
        );
        let previous_generation = state
            .account_by_id(&first.id)
            .expect("saved account")
            .credential_generation;
        fs::remove_file(&path).expect("remove metadata file");
        fs::create_dir(&path).expect("make metadata destination unwritable");

        let operation = state.acquire_operation().expect("operation");
        let result = state.upsert_under_operation(
            &operation,
            draft("workspace", stable_credential("workspace", "new-secret")),
        );
        assert!(result.is_err());
        let still_saved = state.account_by_id(&first.id).expect("old account remains");
        assert_eq!(
            still_saved.credential,
            stable_credential("workspace", "old-secret")
        );
        let old_secret: AccountSecret = state
            .vault
            .get(&format!("account:{}:{previous_generation}", first.id))
            .expect("old generation remains readable");
        assert_eq!(
            old_secret.credential,
            stable_credential("workspace", "old-secret")
        );

        drop(operation);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn switch_rollback_auth_round_trips_only_through_the_vault() {
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
                    secret_ref: None,
                    secret_generation: 0,
                    previous_auth: Some(previous_auth.clone()),
                    stage: crate::types::PendingSwitchStage::Prepared,
                },
            )
            .expect("persist switch transaction");
        let metadata = fs::read_to_string(&path).expect("metadata");
        assert!(!metadata.contains("rollback-secret"));
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
    fn reset_credit_recovery_material_round_trips_only_through_the_vault() {
        let path = temp_path("pending-reset-vault");
        let state = AppState::new(path.clone()).expect("state");
        let operation = state.acquire_operation().expect("operation");
        state
            .prepare_reset_credit_under_operation(
                &operation,
                PendingResetCredit {
                    account_id: "account".into(),
                    secret_ref: None,
                    secret_generation: 0,
                    credit_id: "provider-private-credit".into(),
                    idempotency_key: "private-idempotency-key".into(),
                    created_at_unix_ms: 123,
                },
            )
            .expect("persist reset transaction");
        let metadata = fs::read_to_string(&path).expect("metadata");
        assert!(!metadata.contains("provider-private-credit"));
        assert!(!metadata.contains("private-idempotency-key"));
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
    fn secret_generation_overflow_fails_before_replacing_the_current_secret() {
        assert!(AppState::next_secret_generation(Some(u64::MAX), 1).is_err());
        assert_eq!(
            AppState::next_secret_generation(Some(4), 1).expect("next"),
            5
        );
        assert_eq!(
            AppState::next_secret_generation(None, 0).expect("initial"),
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
                    secret_ref: None,
                    secret_generation: 0,
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
