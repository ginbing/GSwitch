//! Rust-only intent, lease, protected handoff and cleanup of one Codex home.
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    accounts::AppState,
    identity::{derive_identity, document_kind},
    storage,
    types::{AccountIdentity, IsolatedProfileStatus},
};

const CONFIG: &[u8] = b"cli_auth_credentials_store = \"file\"\n";
const FAILURE: &str = "Isolated Codex storage requires cleanup before another isolated operation";

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CredentialBaseline {
    pub id: String,
    pub identity: AccountIdentity,
    pub generation: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ProfileIntent {
    version: u32,
    id: Uuid,
    pub expected_identity: Option<AccountIdentity>,
    pub baseline: Vec<CredentialBaseline>,
    pub oauth: Option<crate::intake::OAuthRecovery>,
}

#[derive(Clone)]
pub struct TempCodexHome {
    inner: Arc<Profile>,
    pub path: PathBuf,
}

struct Profile {
    path: PathBuf,
    lease: File,
    state: Option<AppState>,
    intent: ProfileIntent,
}

impl TempCodexHome {
    pub(crate) fn managed(
        state: &AppState,
        expected_identity: Option<AccountIdentity>,
        oauth: Option<crate::intake::OAuthRecovery>,
    ) -> Result<Self, String> {
        state.ensure_isolated_profiles_ready()?;
        Self::prepare(
            &state.isolated_profile_root()?,
            Some(state.clone()),
            ProfileIntent {
                version: 1,
                id: Uuid::new_v4(),
                expected_identity,
                baseline: state.credential_baseline()?,
                oauth,
            },
        )
    }

    #[cfg(test)]
    pub fn create(root: &Path) -> Result<Self, String> {
        Self::prepare(
            root,
            None,
            ProfileIntent {
                version: 1,
                id: Uuid::new_v4(),
                expected_identity: None,
                baseline: vec![],
                oauth: None,
            },
        )
    }

    fn prepare(
        root: &Path,
        state: Option<AppState>,
        intent: ProfileIntent,
    ) -> Result<Self, String> {
        #[cfg(unix)]
        let root = root
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .zip(root.file_name())
            .map(|(parent, name)| parent.join(name))
            .ok_or_else(|| FAILURE.to_string())?;
        #[cfg(unix)]
        let root = root.as_path();
        fs::create_dir_all(root).map_err(|_| FAILURE.to_string())?;
        checked_ancestors(root)?;
        let path = root.join(format!("gswitch-{}", intent.id));
        create_private_directory(&path)?;
        let lease = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path.join("owner.lock"))
            .map_err(|_| FAILURE.to_string())?;
        lease
            .try_lock_exclusive()
            .map_err(|_| FAILURE.to_string())?;
        let profile = Self {
            path: path.clone(),
            inner: Arc::new(Profile {
                path,
                lease,
                state,
                intent,
            }),
        };
        // Intent is durable before config, credentials, or a provider operation.
        storage::write_json_atomic(
            &profile.path.join("intent.json"),
            &serde_json::to_value(&profile.inner.intent).map_err(|_| FAILURE.to_string())?,
            "isolated operation intent",
        )?;
        storage::write_private_bytes_atomic(
            &profile.path.join("config.toml"),
            CONFIG,
            "temporary Codex configuration",
        )?;
        Ok(profile)
    }

    pub fn write_auth(&self, credential: &Value) -> Result<(), String> {
        checked_tree(&self.path)?;
        storage::write_json_atomic(
            &self.path.join("auth.json"),
            credential,
            "temporary Codex credentials",
        )
    }

    pub fn read_auth(&self) -> Result<Value, String> {
        checked_tree(&self.path)?;
        storage::read_json(&self.path.join("auth.json"), "temporary Codex credentials")
    }

    pub(crate) fn recovery_reference(&self) -> String {
        reference(&self.inner.intent)
    }

    pub(crate) fn cancel_recovery(&self) -> Result<(), String> {
        storage::write_private_bytes_atomic(
            &self.path.join("cancelled"),
            b"cancelled",
            "isolated operation cancellation",
        )
    }

    /// Keep the existing lease across exec on Unix, including the launch
    /// window before the server can report any process identity.
    pub(crate) fn inherit_lease(&self, command: &mut std::process::Command) {
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::process::CommandExt};
            let fd = self.inner.lease.as_raw_fd();
            unsafe {
                command.pre_exec(move || {
                    if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        #[cfg(not(unix))]
        let _ = command;
    }

    pub fn cleanup(self) -> Result<(), String> {
        let profile = Arc::try_unwrap(self.inner).map_err(|_| FAILURE.to_string())?;
        profile.finish()
    }
}

impl Profile {
    fn finish(&self) -> Result<(), String> {
        crate::owned_process::ensure_quiet()?;
        checked_tree(&self.path)?;
        let protected = self
            .state
            .as_ref()
            .map_or(Ok(()), |state| protect(state, &self.path, &self.intent));
        // A failed protected handoff must never create a plaintext backup.
        let removed = remove_material(&self.path);
        if let Some(state) = &self.state {
            if removed.is_err() {
                state.set_isolated_profile_status(IsolatedProfileStatus::CleanupFailed);
            } else if protected.is_err() {
                state.set_isolated_profile_status(IsolatedProfileStatus::ProtectionFailed);
            }
        }
        removed.and(protected)
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        if self.path.exists() && self.finish().is_err() {
            if let Some(state) = &self.state {
                // Do not swallow an uncertain lifetime/path in Drop.
                if state.ensure_isolated_profiles_ready().is_ok() {
                    state.set_isolated_profile_status(IsolatedProfileStatus::Restricted);
                }
            }
        }
        let _ = FileExt::unlock(&self.lease);
    }
}

fn reference(intent: &ProfileIntent) -> String {
    format!("pending-credential-{}", intent.id)
}

fn protect(state: &AppState, path: &Path, intent: &ProfileIntent) -> Result<(), String> {
    if path.join("cancelled").exists() {
        return Ok(());
    }
    let auth = path.join("auth.json");
    if !auth.exists() {
        return Ok(());
    }
    let credential: Value = storage::read_json(&auth, "isolated Codex credentials")?;
    let kind = document_kind(&credential)?;
    let identity = derive_identity(&kind, &credential)?;
    if intent
        .expected_identity
        .as_ref()
        .is_some_and(|expected| expected != &identity)
    {
        return Err("Isolated credential identity changed".into());
    }
    state.protect_isolated_credential(&reference(intent), &credential, &identity, intent)
}

/// No provider actions or account commits. Startup only protects then removes
/// verified, inactive material. Existing explicit recovery owns application.
pub(crate) fn reconcile(state: &AppState) -> Result<(), IsolatedProfileStatus> {
    reconcile_with_activity(state, crate::runtime::external_codex_running)
}

fn reconcile_with_activity(
    state: &AppState,
    activity: impl Fn(&[u32]) -> Result<bool, String>,
) -> Result<(), IsolatedProfileStatus> {
    let root = state
        .isolated_profile_root()
        .map_err(|_| IsolatedProfileStatus::Restricted)?;
    if !root.exists() {
        return Ok(());
    }
    checked_ancestors(&root).map_err(|_| IsolatedProfileStatus::Restricted)?;
    let entries = fs::read_dir(&root).map_err(|_| IsolatedProfileStatus::Restricted)?;
    let mut status = None;
    for entry in entries {
        let result = (|| {
            let path = entry.map_err(|_| IsolatedProfileStatus::Restricted)?.path();
            let id = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("gswitch-"))
                .and_then(|name| Uuid::parse_str(name).ok())
                .ok_or(IsolatedProfileStatus::Restricted)?;
            checked_tree(&path).map_err(|_| IsolatedProfileStatus::Restricted)?;
            // A live owner holds this lease. Unknown legacy profiles have no
            // lease; both cases additionally require no possible Codex user.
            let lease = if path.join("owner.lock").exists() {
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path.join("owner.lock"))
                    .map_err(|_| IsolatedProfileStatus::Restricted)?;
                file.try_lock_exclusive()
                    .map_err(|_| IsolatedProfileStatus::Restricted)?;
                Some(file)
            } else {
                None
            };
            if activity(&[]).map_err(|_| IsolatedProfileStatus::Restricted)? {
                return Err(IsolatedProfileStatus::Restricted);
            }
            let protected = if path.join("intent.json").exists() {
                let intent: ProfileIntent = serde_json::from_value(
                    storage::read_json(&path.join("intent.json"), "isolated operation intent")
                        .map_err(|_| IsolatedProfileStatus::Restricted)?,
                )
                .map_err(|_| IsolatedProfileStatus::Restricted)?;
                if intent.version != 1 || intent.id != id {
                    return Err(IsolatedProfileStatus::Restricted);
                }
                protect(state, &path, &intent)
            } else {
                // Legacy ownership requires the exact generated name/config
                // in our storage and a verified inactive, unredirected tree.
                if fs::read(path.join("config.toml")).ok().as_deref() != Some(CONFIG) {
                    return Err(IsolatedProfileStatus::Restricted);
                }
                Ok(()) // No intent means no authority to recover credentials.
            };
            let removed = remove_material(&path).map_err(|_| IsolatedProfileStatus::CleanupFailed);
            drop(lease);
            removed?;
            protected.map_err(|_| IsolatedProfileStatus::ProtectionFailed)
        })();
        if let Err(error) = result {
            // Cleanup/path failures take priority over a completed deletion
            // that only needs a fresh login.
            if status.is_none() || error != IsolatedProfileStatus::ProtectionFailed {
                status = Some(error);
            }
        }
    }
    status.map_or(Ok(()), Err)
}

fn checked_ancestors(path: &Path) -> Result<(), String> {
    for path in path.ancestors() {
        if path.exists() {
            checked_entry(path)?;
        }
    }
    Ok(())
}

fn remove_material(path: &Path) -> Result<(), String> {
    let remove = || -> std::io::Result<()> {
        // Keep the intent/lease/cancellation until every potentially secret
        // entry has gone. A locked auth file must not lose its recovery intent
        // because remove_dir_all deleted metadata first.
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if ["intent.json", "config.toml", "owner.lock", "cancelled"]
                .iter()
                .any(|name| entry.file_name() == *name)
            {
                continue;
            }
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        fs::remove_dir_all(path)
    };
    remove().map_err(|_| "Unable to remove isolated Codex credentials".to_string())
}

fn checked_entry(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| FAILURE.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err(FAILURE.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.is_file() && metadata.nlink() != 1 {
            return Err(FAILURE.into());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(FAILURE.into());
        }
        if metadata.is_file() {
            use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
            use windows_sys::Win32::Storage::FileSystem::{
                GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
            };
            // Metadata-only access also works while a reader denies data
            // sharing. No credential bytes are read through this handle.
            let file = OpenOptions::new()
                .read(true)
                .access_mode(0)
                .open(path)
                .map_err(|_| FAILURE.to_string())?;
            let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
                || info.nNumberOfLinks != 1
            {
                return Err(FAILURE.into());
            }
        }
    }
    Ok(metadata)
}

fn checked_tree(path: &Path) -> Result<(), String> {
    checked_ancestors(path)?;
    let metadata = checked_entry(path)?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|_| FAILURE.to_string())? {
            checked_tree(&entry.map_err(|_| FAILURE.to_string())?.path())?;
        }
    } else if !metadata.is_file() {
        return Err(FAILURE.into());
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let mut builder = fs::DirBuilder::new();
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| FAILURE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        accounts::AccountDraft,
        owned_process::{OwnedProcess, CLEANUP_TIMEOUT},
        types::{AccountKind, AccountView},
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!("gswitch-isolation-test-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        path
    }

    fn credential(workspace: &str, token: &str) -> Value {
        let claims = URL_SAFE_NO_PAD.encode(json!({"https://api.openai.com/auth": {"chatgpt_user_id":"synthetic-user", "chatgpt_account_id":workspace}}).to_string());
        json!({"auth_mode":"chatgpt", "tokens":{"access_token":token,"refresh_token":"synthetic-refresh", "id_token":format!("header.{claims}.signature")}, "unknown":{"preserve":true}})
    }

    fn save(state: &AppState, token: &str) -> AccountView {
        let credential = credential("synthetic", token);
        let operation = state.acquire_operation().unwrap();
        state
            .upsert_under_operation(
                &operation,
                AccountDraft {
                    label: None,
                    default_label: "Synthetic".into(),
                    kind: AccountKind::ChatGpt,
                    email: None,
                    plan_type: None,
                    workspace_name: None,
                    account_structure: None,
                    identity: derive_identity(&AccountKind::ChatGpt, &credential).unwrap(),
                    credential,
                },
            )
            .unwrap()
    }

    fn profile(state: &AppState) -> TempCodexHome {
        TempCodexHome::managed(
            state,
            Some(derive_identity(&AccountKind::ChatGpt, &credential("synthetic", "old")).unwrap()),
            None,
        )
        .unwrap_or_else(|error| {
            panic!(
                "prepare isolated profile: {error}; startup status: {:?}",
                state.storage_view().isolated_profile_status
            )
        })
    }

    // Drop the in-memory owner without its cleanup, as a crash would. The
    // process tests below also exercise actual forced termination.
    fn abandon(profile: TempCodexHome) -> PathBuf {
        let path = profile.path.clone();
        let mut inner = Arc::try_unwrap(profile.inner).ok().unwrap();
        inner.state = None;
        inner.path = PathBuf::new();
        drop(inner); // closes the lease without touching the original tree
        path
    }

    fn reconcile_inactive(state: &AppState) -> Result<(), IsolatedProfileStatus> {
        // Only activity detection is replaced, avoiding the developer's real
        // Codex runtime. Actual filesystem, leases, vault and metadata remain.
        reconcile_with_activity(state, |_| Ok(false))
    }

    #[test]
    fn normal_handoff_and_restart_recovery_share_identity_and_generation_rules() {
        let root = root();
        let state = AppState::new(root.join("accounts.json")).unwrap();
        let account = save(&state, "old");
        let temporary = profile(&state);
        let path = temporary.path.clone();
        let intent = fs::read_to_string(path.join("intent.json")).unwrap();
        assert!(!intent.contains("synthetic-refresh"));
        temporary
            .write_auth(&credential("synthetic", "rotated"))
            .unwrap();
        temporary.cleanup().unwrap();
        assert!(!path.exists());
        // Protection alone never commits an account or contacts a provider.
        assert_eq!(
            state.account_by_id(&account.id).unwrap().credential,
            credential("synthetic", "old")
        );
        assert_eq!(state.recover_pending_credentials().unwrap(), 1);
        assert_eq!(
            state.account_by_id(&account.id).unwrap().credential,
            credential("synthetic", "rotated")
        );
        assert_eq!(state.recover_pending_credentials().unwrap(), 0);

        let temporary = profile(&state);
        temporary
            .write_auth(&credential("synthetic", "after-crash"))
            .unwrap();
        let path = abandon(temporary);
        assert_eq!(reconcile_inactive(&state), Ok(()));
        assert!(!path.exists());
        assert_eq!(reconcile_inactive(&state), Ok(()));
        assert_eq!(state.recover_pending_credentials().unwrap(), 1);
        assert_eq!(
            state.account_by_id(&account.id).unwrap().credential,
            credential("synthetic", "after-crash")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_intent_cannot_replace_a_new_login_or_restore_a_removed_account() {
        for removed in [false, true] {
            let root = root();
            let state = AppState::new(root.join("accounts.json")).unwrap();
            let account = save(&state, "old");
            let temporary = profile(&state);
            temporary
                .write_auth(&credential("synthetic", "interrupted"))
                .unwrap();
            let path = abandon(temporary);
            if removed {
                let operation = state.acquire_operation().unwrap();
                state
                    .remove_under_operation(&operation, &account.id)
                    .unwrap();
            } else {
                save(&state, "new-login");
            }
            assert_eq!(reconcile_inactive(&state), Ok(()));
            assert!(!path.exists());
            assert!(state.recover_pending_credentials().is_err());
            if removed {
                assert!(state.list().unwrap().is_empty());
            } else {
                assert_eq!(
                    state.account_by_id(&account.id).unwrap().credential,
                    credential("synthetic", "new-login")
                );
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn repeated_handoff_and_failed_metadata_commit_keep_one_protected_entry() {
        let root = root();
        let state = AppState::new(root.join("accounts.json")).unwrap();
        let account = save(&state, "old");
        let temporary = profile(&state);
        temporary
            .write_auth(&credential("synthetic", "rotated"))
            .unwrap();
        let intent = temporary.inner.intent.clone();
        let path = abandon(temporary);
        protect(&state, &path, &intent).unwrap();
        protect(&state, &path, &intent).unwrap();
        assert_eq!(reconcile_inactive(&state), Ok(()));
        fs::rename(root.join("accounts.json"), root.join("saved-metadata")).unwrap();
        fs::create_dir(root.join("accounts.json")).unwrap();
        assert!(state.recover_pending_credentials().is_err());
        assert_eq!(
            state.account_by_id(&account.id).unwrap().credential,
            credential("synthetic", "old")
        );
        fs::remove_dir(root.join("accounts.json")).unwrap();
        fs::rename(root.join("saved-metadata"), root.join("accounts.json")).unwrap();
        assert_eq!(state.recover_pending_credentials().unwrap(), 1);
        assert_eq!(state.recover_pending_credentials().unwrap(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mismatched_identity_or_unavailable_vault_never_keeps_plaintext_as_backup() {
        for unavailable in [false, true] {
            let root = root();
            let state = AppState::new(root.join("accounts.json")).unwrap();
            save(&state, "old");
            let temporary = profile(&state);
            temporary
                .write_auth(&credential(
                    if unavailable { "synthetic" } else { "other" },
                    "rotated",
                ))
                .unwrap();
            let path = abandon(temporary);
            if unavailable {
                fs::remove_file(root.join("credentials.hold")).unwrap();
                fs::create_dir(root.join("credentials.hold")).unwrap();
            }
            assert_eq!(
                reconcile_inactive(&state),
                Err(IsolatedProfileStatus::ProtectionFailed)
            );
            assert!(!path.exists());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn live_leases_uncertain_activity_and_unowned_paths_are_left_alone() {
        let root = root();
        let state = AppState::new(root.join("accounts.json")).unwrap();
        let temporary = profile(&state);
        temporary
            .write_auth(&credential("synthetic", "fixture"))
            .unwrap();
        assert_eq!(
            reconcile_inactive(&state),
            Err(IsolatedProfileStatus::Restricted)
        );
        assert!(temporary.path.exists());
        let path = abandon(temporary);
        assert_eq!(
            reconcile_with_activity(&state, |_| Ok(true)),
            Err(IsolatedProfileStatus::Restricted)
        );
        assert_eq!(
            reconcile_with_activity(&state, |_| Err("uncertain".into())),
            Err(IsolatedProfileStatus::Restricted)
        );
        assert!(path.exists());
        let unrelated = state.isolated_profile_root().unwrap().join("user-notes");
        fs::create_dir(&unrelated).unwrap();
        fs::write(unrelated.join("keep"), b"unrelated").unwrap();
        assert_eq!(
            reconcile_inactive(&state),
            Err(IsolatedProfileStatus::Restricted)
        );
        assert_eq!(fs::read(unrelated.join("keep")).unwrap(), b"unrelated");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_intent_and_verified_legacy_profiles_are_cleaned_without_recovery() {
        let root = root();
        let state = AppState::new(root.join("accounts.json")).unwrap();
        let temporary = profile(&state);
        temporary
            .write_auth(&credential("synthetic", "cancelled"))
            .unwrap();
        temporary.cancel_recovery().unwrap();
        let path = abandon(temporary);
        assert_eq!(reconcile_inactive(&state), Ok(()));
        assert!(!path.exists());
        assert_eq!(state.recover_pending_credentials().unwrap(), 0);
        let legacy = state
            .isolated_profile_root()
            .unwrap()
            .join(format!("gswitch-{}", Uuid::new_v4()));
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("config.toml"), CONFIG).unwrap();
        storage::write_json_atomic(
            &legacy.join("auth.json"),
            &credential("synthetic", "legacy"),
            "fixture",
        )
        .unwrap();
        assert_eq!(reconcile_inactive(&state), Ok(()));
        assert!(!legacy.exists());
        assert_eq!(state.recover_pending_credentials().unwrap(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_locked_auth_file_reports_cleanup_failure_until_a_later_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = root();
        let state = AppState::new(root.join("accounts.json")).unwrap();
        save(&state, "old");
        let temporary = profile(&state);
        temporary
            .write_auth(&credential("synthetic", "rotated"))
            .unwrap();
        let path = abandon(temporary);
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path.join("auth.json"))
            .unwrap();
        assert_eq!(
            reconcile_inactive(&state),
            Err(IsolatedProfileStatus::CleanupFailed)
        );
        assert!(path.join("auth.json").exists());
        drop(locked);
        assert_eq!(reconcile_inactive(&state), Ok(()));
        assert!(!path.exists());
        assert_eq!(state.recover_pending_credentials().unwrap(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn redirected_trees_never_read_or_delete_the_target() {
        let root = root();
        let state = AppState::new(root.join("accounts.json")).unwrap();
        let temporary = profile(&state);
        let path = abandon(temporary);
        let outside = root.join("unrelated");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"untouched").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, path.join("redirect")).unwrap();
        #[cfg(windows)]
        {
            // A junction needs no Developer Mode or elevated symlink privilege.
            assert!(Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(path.join("redirect"))
                .arg(&outside)
                .stdout(Stdio::null())
                .status()
                .unwrap()
                .success());
        }
        assert_eq!(
            reconcile_inactive(&state),
            Err(IsolatedProfileStatus::Restricted)
        );
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"untouched");
        #[cfg(unix)]
        fs::remove_file(path.join("redirect")).unwrap();
        #[cfg(windows)]
        fs::remove_dir(path.join("redirect")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hard_linked_files_never_authorize_recovery_or_target_deletion() {
        let root = root();
        let state = AppState::new(root.join("accounts.json")).unwrap();
        let temporary = profile(&state);
        let path = abandon(temporary);
        let outside = root.join("unrelated-credential.json");
        storage::write_json_atomic(&outside, &credential("synthetic", "unrelated"), "fixture")
            .unwrap();
        fs::hard_link(&outside, path.join("auth.json")).unwrap();
        assert_eq!(
            reconcile_inactive(&state),
            Err(IsolatedProfileStatus::Restricted)
        );
        assert_eq!(
            storage::read_json(&outside, "fixture").unwrap(),
            credential("synthetic", "unrelated")
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn fixture_command(root: &Path, kind: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "isolated_profile::tests::crash_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("GSWITCH_ISOLATION_FIXTURE_ROOT", root)
            .env("GSWITCH_ISOLATION_FIXTURE_KIND", kind)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    #[test]
    #[ignore = "only executed as a scoped synthetic subprocess"]
    #[allow(clippy::zombie_processes)] // Parent test owns and terminates the fixture group after the forced crash.
    fn crash_fixture() {
        let Some(root) = std::env::var_os("GSWITCH_ISOLATION_FIXTURE_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let kind = std::env::var("GSWITCH_ISOLATION_FIXTURE_KIND").unwrap();
        if kind == "leaf" {
            fs::write(root.join("leaf-ready"), b"ready").unwrap();
        } else {
            let state = AppState::new(root.join("accounts.json")).unwrap();
            save(&state, "old");
            let temporary = profile(&state);
            temporary
                .write_auth(&credential("synthetic", "rotated"))
                .unwrap();
            fs::write(
                root.join("profile-path"),
                temporary.path.to_string_lossy().as_bytes(),
            )
            .unwrap();
            let mut command = fixture_command(&root, "leaf");
            temporary.inherit_lease(&mut command);
            #[cfg(windows)]
            let _owned = OwnedProcess::spawn(&mut command).unwrap();
            #[cfg(unix)]
            let _owned = command.spawn().unwrap();
            fs::write(root.join("owner-ready"), b"ready").unwrap();
            thread::sleep(Duration::from_secs(60));
            return;
        }
        thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn forced_owner_termination_keeps_surviving_helpers_out_of_recovery() {
        let root = root();
        let mut owner = OwnedProcess::spawn(&mut fixture_command(&root, "owner")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !(root.join("owner-ready").exists() && root.join("leaf-ready").exists())
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(root.join("owner-ready").exists() && root.join("leaf-ready").exists());
        let path = PathBuf::from(fs::read_to_string(root.join("profile-path")).unwrap());
        owner.child.kill().unwrap();
        assert!(owner
            .wait_until(Instant::now() + Duration::from_secs(5))
            .unwrap()
            .is_some());
        let state = AppState::new(root.join("accounts.json")).unwrap();
        #[cfg(unix)]
        {
            assert_eq!(
                reconcile_inactive(&state),
                Err(IsolatedProfileStatus::Restricted)
            );
            assert!(path.join("auth.json").exists());
        }
        // This retained test owner handle stops its own entire group/job.
        assert!(owner.cleanup(Instant::now() + CLEANUP_TIMEOUT));
        assert_eq!(reconcile_inactive(&state), Ok(()));
        assert!(!path.exists());
        assert_eq!(state.recover_pending_credentials().unwrap(), 1);
        assert_eq!(state.list().unwrap().len(), 1);
        assert_eq!(
            state
                .account_by_id(&state.list().unwrap()[0].id)
                .unwrap()
                .credential,
            credential("synthetic", "rotated")
        );
        assert_eq!(state.recover_pending_credentials().unwrap(), 0);
        fs::remove_dir_all(root).unwrap();
    }
}
