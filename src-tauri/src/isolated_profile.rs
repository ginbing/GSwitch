//! One short-lived Codex home per isolated operation. It is deleted when the
//! operation ends; startup removes leftovers from an interrupted run.
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
};

use fs2::FileExt;
use serde_json::Value;
use uuid::Uuid;

use crate::{accounts::AppState, storage};

const CONFIG: &[u8] = b"cli_auth_credentials_store = \"file\"\n";
const PREFIX: &str = "gswitch-";
const OWNER_LOCK: &str = "owner.lock";

/// The App Server keeps a clone until its process has stopped, so a profile is
/// never removed underneath a running Codex.
#[derive(Clone)]
pub struct TempCodexHome {
    inner: Arc<Profile>,
    pub path: PathBuf,
}

struct Profile {
    path: PathBuf,
    // Held for the profile's lifetime: startup cleanup in another GSwitch
    // process leaves a locked profile alone.
    _lock: File,
}

impl TempCodexHome {
    pub(crate) fn new(state: &AppState) -> Result<Self, String> {
        Self::create(&state.isolated_profile_root()?)
    }

    pub(crate) fn create(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root)
            .map_err(|_| "Unable to prepare isolated Codex storage".to_string())?;
        let path = root.join(format!("{PREFIX}{}", Uuid::new_v4()));
        create_private_directory(&path)?;
        let lock = match lock_new_profile(&path) {
            Ok(lock) => lock,
            Err(error) => {
                let _ = fs::remove_dir_all(&path);
                return Err(error);
            }
        };
        let profile = Self {
            path: path.clone(),
            inner: Arc::new(Profile { path, _lock: lock }),
        };
        storage::write_private_bytes_atomic(
            &profile.path.join("config.toml"),
            CONFIG,
            "temporary Codex configuration",
        )?;
        Ok(profile)
    }

    pub fn write_auth(&self, credential: &Value) -> Result<(), String> {
        storage::write_json_atomic(
            &self.path.join("auth.json"),
            credential,
            "temporary Codex credentials",
        )
    }

    pub fn read_auth(&self) -> Result<Value, String> {
        storage::read_json(&self.path.join("auth.json"), "temporary Codex credentials")
    }

    /// Call after the App Server using this profile has been dropped. An
    /// explicit result keeps a Windows file lock from masquerading as cleanup;
    /// whatever remains is removed at the next start.
    pub fn cleanup(self) -> Result<(), String> {
        let profile = Arc::try_unwrap(self.inner)
            .map_err(|_| "The isolated Codex profile is still in use".to_string())?;
        remove_profile(&profile.path)
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        let _ = remove_profile(&self.path);
    }
}

/// Removes profiles that a forced stop left behind. A profile whose owner lock
/// a running GSwitch still holds is in use. Anything that cannot be removed now
/// is retried at the next start.
pub(crate) fn remove_leftovers(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let generated = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_prefix(PREFIX))
            .is_some_and(|id| Uuid::parse_str(id).is_ok());
        if generated && !in_use(&entry.path()) {
            let _ = remove_profile(&entry.path());
        }
    }
}

fn in_use(path: &Path) -> bool {
    match File::open(path.join(OWNER_LOCK)) {
        Ok(lock) => lock.try_lock_exclusive().is_err(),
        // Profiles from v1.0.27 and earlier have no owner lock.
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// `remove_dir_all` removes links, such as the helper links Codex creates on
/// Unix, without following them.
fn remove_profile(path: &Path) -> Result<(), String> {
    fs::remove_dir_all(path).map_err(|_| "Unable to remove isolated Codex credentials".to_string())
}

fn lock_new_profile(path: &Path) -> Result<File, String> {
    let failure = || "Unable to create a temporary Codex profile".to_string();
    let lock = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(path.join(OWNER_LOCK))
        .map_err(|_| failure())?;
    lock.try_lock_exclusive().map_err(|_| failure())?;
    Ok(lock)
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
    builder
        .create(path)
        .map_err(|_| "Unable to create a temporary Codex profile".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{accounts::AccountDraft, identity::derive_identity, types::AccountKind};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;

    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!("gswitch-isolation-test-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        path
    }

    fn credential(token: &str) -> Value {
        let claims = URL_SAFE_NO_PAD.encode(json!({"https://api.openai.com/auth": {"chatgpt_user_id":"synthetic-user", "chatgpt_account_id":"synthetic"}}).to_string());
        json!({"auth_mode":"chatgpt", "tokens":{"access_token":token,"refresh_token":"synthetic-refresh", "id_token":format!("header.{claims}.signature")}})
    }

    fn save(state: &AppState, token: &str) -> String {
        let credential = credential(token);
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
            .id
    }

    fn link_dir(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            // A junction needs no Developer Mode or elevated symlink privilege.
            assert!(std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap()
                .success());
        }
    }

    // Close the owner lock without removing the profile, as a forced stop would.
    fn abandon(profile: TempCodexHome) -> PathBuf {
        let path = profile.path.clone();
        let mut inner = Arc::try_unwrap(profile.inner).ok().unwrap();
        inner.path = PathBuf::new();
        drop(inner);
        path
    }

    #[test]
    fn codex_helper_links_are_removed_without_touching_their_targets() {
        let root = root();
        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("codex"), b"binary").unwrap();
        let profile = TempCodexHome::create(&root.join("isolated-codex")).unwrap();
        // Codex creates helper aliases under CODEX_HOME/tmp each time it starts;
        // on Unix they are links to its own binary.
        let helpers = profile
            .path
            .join("tmp")
            .join("arg0")
            .join("codex-arg0-fixture");
        fs::create_dir_all(&helpers).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.join("codex"), helpers.join("apply_patch")).unwrap();
        link_dir(&outside, &helpers.join("linked-directory"));

        profile.write_auth(&credential("rotated")).unwrap();
        assert_eq!(profile.read_auth().unwrap(), credential("rotated"));
        let path = profile.path.clone();
        profile.cleanup().unwrap();

        assert!(!path.exists());
        assert_eq!(fs::read(outside.join("codex")).unwrap(), b"binary");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_profile_held_by_its_app_server_is_removed_when_released() {
        let root = root();
        let profile = TempCodexHome::create(&root.join("isolated-codex")).unwrap();
        let server_owner = profile.clone();
        let path = profile.path.clone();
        assert!(profile.cleanup().is_err());
        assert!(path.exists());
        drop(server_owner);
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_removes_leftovers_but_keeps_live_profiles_and_unrelated_entries() {
        let root = root();
        let isolated = root.join("isolated-codex");
        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"untouched").unwrap();

        let crashed = TempCodexHome::create(&isolated).unwrap();
        crashed.write_auth(&credential("left")).unwrap();
        link_dir(&outside, &crashed.path.join("redirect"));
        let crashed = abandon(crashed);
        let legacy = isolated.join(format!("{PREFIX}{}", Uuid::new_v4()));
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("auth.json"), credential("legacy").to_string()).unwrap();
        let live = TempCodexHome::create(&isolated).unwrap();
        live.write_auth(&credential("live")).unwrap();
        fs::create_dir(isolated.join("unrelated")).unwrap();
        fs::write(isolated.join(".DS_Store"), b"").unwrap();

        remove_leftovers(&isolated);

        assert!(!crashed.exists());
        assert!(!legacy.exists());
        assert_eq!(live.read_auth().unwrap(), credential("live"));
        assert!(isolated.join("unrelated").exists());
        assert!(isolated.join(".DS_Store").exists());
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"untouched");
        live.cleanup().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_profile_that_cannot_be_removed_now_is_removed_at_the_next_start() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = root();
        let isolated = root.join("isolated-codex");
        let profile = TempCodexHome::create(&isolated).unwrap();
        profile.write_auth(&credential("rotated")).unwrap();
        let path = profile.path.clone();
        // Like a Codex helper that has not exited yet.
        let held = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path.join("auth.json"))
            .unwrap();
        assert!(profile.cleanup().is_err());
        assert!(path.join("auth.json").exists());
        drop(held);
        remove_leftovers(&isolated);
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_and_startup_never_keep_a_credential_for_recovery() {
        let root = root();
        let state = AppState::new(root.join("accounts.json")).unwrap();
        let account = save(&state, "saved");
        let finished = TempCodexHome::new(&state).unwrap();
        finished.write_auth(&credential("rotated")).unwrap();
        finished.cleanup().unwrap();
        let interrupted = TempCodexHome::new(&state).unwrap();
        interrupted.write_auth(&credential("unvalidated")).unwrap();
        let left = abandon(interrupted);
        drop(state);

        let restarted = AppState::new(root.join("accounts.json")).unwrap();
        assert!(!left.exists());
        assert_eq!(restarted.recover_pending_credentials().unwrap(), 0);
        assert_eq!(
            restarted.account_by_id(&account).unwrap().credential,
            credential("saved")
        );
        drop(restarted);
        fs::remove_dir_all(root).unwrap();
    }
}
