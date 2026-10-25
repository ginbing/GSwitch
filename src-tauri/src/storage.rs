use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use atomic_write_file::OpenOptions;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::types::{PendingResetCredit, PendingSwitch, StoredAccount};

/// Version 5 keeps complete credentials in this owner-only file. Version 4
/// kept them in the Stronghold vault of GSwitch 1.0.30 and earlier, which only
/// 1.0.31 and 1.0.32 move into this file; this version leaves it unchanged.
pub const STORE_VERSION: u32 = 5;
pub const VAULT_STORE_VERSION: u32 = 4;
const PENDING_CREDENTIAL_PREFIX: &str = "pending-credential-";

#[derive(Clone, Serialize, Deserialize)]
pub struct AccountStore {
    #[serde(default = "default_store_version")]
    pub version: u32,
    #[serde(default)]
    pub accounts: Vec<StoredAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_switch: Option<PendingSwitch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_reset_credit: Option<PendingResetCredit>,
}

impl Default for AccountStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            accounts: Vec::new(),
            active_account_id: None,
            pending_switch: None,
            pending_reset_credit: None,
        }
    }
}

fn default_store_version() -> u32 {
    // Stores written before an explicit version field hold inline
    // credentials that must be verified before the current version is written.
    0
}

/// Reads the store as JSON, so a version-4 store can have its vault
/// references resolved before it is typed. A missing file is `None`.
pub fn read_store_json(path: &Path) -> Result<Option<Value>, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Unable to read the GSwitch account store".to_string()),
    };
    deserialize(&content, "GSwitch account store").map(Some)
}

pub fn store_version(value: &Value) -> Result<u32, String> {
    let version = match value.get("version") {
        None => default_store_version(),
        Some(version) => version
            .as_u64()
            .and_then(|version| u32::try_from(version).ok())
            .ok_or_else(|| "Unable to parse GSwitch account store".to_string())?,
    };
    if version > STORE_VERSION {
        return Err("This GSwitch account store was created by a newer version".to_string());
    }
    Ok(version)
}

pub fn store_from_json(value: Value) -> Result<AccountStore, String> {
    store_version(&value)?;
    serde_json::from_value(value).map_err(|_| "Unable to parse GSwitch account store".to_string())
}

pub fn load(path: &Path) -> Result<AccountStore, String> {
    match read_store_json(path)? {
        Some(value) => store_from_json(value),
        None => Ok(AccountStore::default()),
    }
}

pub fn save_atomic(path: &Path, store: &AccountStore) -> Result<(), String> {
    // Every successful write upgrades an older compatible store. This keeps
    // migrations explicit without treating an old, valid account library as
    // disposable data.
    let mut current = store.clone();
    current.version = STORE_VERSION;
    let content = serde_json::to_vec_pretty(&current)
        .map_err(|_| "Unable to serialize the GSwitch account store".to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid GSwitch account store path".to_string())?;
    ensure_private_dir(parent)?;
    write_private_bytes_atomic(path, &content, "GSwitch account store")
}

/// Creates a GSwitch-owned directory and, on Unix, limits it to the current
/// user. Windows keeps the per-user profile permissions it inherits.
pub fn ensure_private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|_| "Unable to prepare GSwitch storage".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Unable to protect GSwitch storage".to_string())?;
    }
    Ok(())
}

/// Moves an unreadable or unsupported GSwitch account library aside before a
/// user-confirmed reset. The recovery directory always sits beside the store,
/// so the move stays on one filesystem. This function never touches Codex's
/// own credentials.
pub fn preserve_damaged_store(path: &Path, recovery_dir: &Path) -> Result<(), String> {
    match fs::metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err("Unable to preserve the damaged GSwitch account store".to_string());
            }
            ensure_private_dir(recovery_dir)
                .map_err(|_| "Unable to prepare GSwitch recovery storage".to_string())?;
            let preserved = recovery_dir.join(format!("accounts-damaged-{}.json", Uuid::new_v4()));
            fs::rename(path, preserved)
                .map_err(|_| "Unable to preserve the damaged GSwitch account store".to_string())
        }
        // A prior reset attempt may already have moved the damaged file but
        // failed before the fresh empty store was written. Never require the
        // user to recreate or overwrite that preserved source to retry.
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let preserved_exists = fs::read_dir(recovery_dir)
                .ok()
                .and_then(|entries| {
                    entries.flatten().find(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with("accounts-damaged-")
                    })
                })
                .is_some();
            if preserved_exists {
                Ok(())
            } else {
                Err(
                    "The damaged GSwitch account store is no longer available to preserve"
                        .to_string(),
                )
            }
        }
        Err(_) => Err("Unable to preserve the damaged GSwitch account store".to_string()),
    }
}

/// One owner-only file per pending credential, named by its reference. A
/// recovery copy therefore does not depend on the account file being writable.
pub fn write_pending_credential<T: Serialize>(
    dir: &Path,
    reference: &str,
    value: &T,
) -> Result<(), String> {
    let path = pending_credential_path(dir, reference)?;
    let content = serde_json::to_vec_pretty(value)
        .map_err(|_| "Unable to serialize credential recovery".to_string())?;
    ensure_private_dir(dir)?;
    write_private_bytes_atomic(&path, &content, "credential recovery")
}

pub fn read_pending_credential<T: DeserializeOwned>(
    dir: &Path,
    reference: &str,
) -> Result<T, String> {
    let path = pending_credential_path(dir, reference)?;
    let content =
        fs::read(&path).map_err(|_| "Credential recovery could not be read safely".to_string())?;
    serde_json::from_slice(&content).map_err(|_| "Credential recovery is incomplete".to_string())
}

/// Every pending credential, ordered by reference. An entry that is not a
/// regular file with a generated name blocks recovery instead of being skipped.
pub fn pending_credentials<T: DeserializeOwned>(dir: &Path) -> Result<Vec<(String, T)>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("Credential recovery could not be read safely".to_string()),
    };
    let mut pending = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|_| "Credential recovery could not be read safely".to_string())?
            .path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| "Credential recovery could not be read safely".to_string())?;
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| Uuid::parse_str(stem).ok())
            .filter(|_| metadata.file_type().is_file())
            .ok_or_else(|| "Credential recovery could not be verified safely".to_string())?;
        let reference = format!("{PENDING_CREDENTIAL_PREFIX}{id}");
        let value = read_pending_credential(dir, &reference)?;
        pending.push((reference, value));
    }
    pending.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(pending)
}

pub fn retire_pending_credential(dir: &Path, reference: &str) -> Result<(), String> {
    fs::remove_file(pending_credential_path(dir, reference)?)
        .map_err(|_| "Credential recovery could not be retired safely".to_string())
}

pub fn new_pending_credential_reference() -> String {
    format!("{PENDING_CREDENTIAL_PREFIX}{}", Uuid::new_v4())
}

fn pending_credential_path(dir: &Path, reference: &str) -> Result<PathBuf, String> {
    let id = reference
        .strip_prefix(PENDING_CREDENTIAL_PREFIX)
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(|| "Credential recovery is incomplete".to_string())?;
    Ok(dir.join(format!("{id}.json")))
}

pub fn read_json(path: &Path, label: &str) -> Result<Value, String> {
    let content = fs::read_to_string(path).map_err(|_| format!("Unable to read {label}"))?;
    deserialize(&content, label)
}

pub fn write_json_atomic(path: &Path, value: &Value, label: &str) -> Result<(), String> {
    let content =
        serde_json::to_vec_pretty(value).map_err(|_| format!("Unable to serialize {label}"))?;
    write_private_bytes_atomic(path, &content, label)
}

pub fn write_private_bytes_atomic(path: &Path, content: &[u8], label: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid {label} path"))?;
    fs::create_dir_all(parent).map_err(|_| format!("Unable to create storage for {label}"))?;

    #[cfg(unix)]
    let mut options = OpenOptions::new();

    #[cfg(not(unix))]
    let options = OpenOptions::new();

    #[cfg(unix)]
    {
        use atomic_write_file::unix::OpenOptionsExt as AtomicOpenOptionsExt;
        use std::os::unix::fs::OpenOptionsExt as StdOpenOptionsExt;

        AtomicOpenOptionsExt::preserve_mode(&mut options, false);
        StdOpenOptionsExt::mode(&mut options, 0o600);
    }

    let mut file = options
        .open(path)
        .map_err(|_| format!("Unable to prepare {label} for an atomic write"))?;
    file.write_all(content)
        .map_err(|_| format!("Unable to write {label}"))?;
    file.commit()
        .map_err(|_| format!("Unable to atomically replace {label}"))
}

fn deserialize<T: DeserializeOwned>(content: &str, label: &str) -> Result<T, String> {
    serde_json::from_str(content).map_err(|_| format!("Unable to parse {label}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AccountIdentity, AccountKind, PendingResetCredit, StoredAccount};
    use serde_json::json;
    use std::{
        env,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_store_path(name: &str) -> std::path::PathBuf {
        let root = env::temp_dir().join(format!(
            "gswitch-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        root.join("accounts.json")
    }

    fn account(id: &str, credential: Value) -> StoredAccount {
        StoredAccount {
            id: id.into(),
            label: "Personal".into(),
            kind: AccountKind::ChatGpt,
            email: Some("user@example.com".into()),
            plan_type: Some("pro".into()),
            workspace_name: Some("Personal".into()),
            account_structure: Some("workspace".into()),
            identity: Some(AccountIdentity::ChatGpt {
                user_id: "user".into(),
                workspace_id: Some("workspace".into()),
            }),
            quota: None,
            reset_credits: None,
            credential_generation: 1,
            needs_apply: false,
            sign_in_required: false,
            credential,
        }
    }

    #[test]
    fn the_store_keeps_complete_credentials_with_unknown_fields() {
        let path = temp_store_path("roundtrip");
        let credential = json!({
            "tokens": {"access_token": "secret", "refresh_token": "rotate-me"},
            "auth_mode": "chatgpt",
            "future_field": {"preserve": true}
        });
        let store = AccountStore {
            version: STORE_VERSION,
            accounts: vec![account("one", credential.clone())],
            active_account_id: None,
            pending_switch: None,
            pending_reset_credit: None,
        };

        save_atomic(&path, &store).expect("save");
        let loaded = load(&path).expect("load");
        assert_eq!(loaded.version, STORE_VERSION);
        assert_eq!(loaded.accounts[0].credential, credential);
        assert_eq!(loaded.accounts[0].credential_generation, 1);

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[cfg(unix)]
    #[test]
    fn the_store_and_its_folder_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_store_path("owner-only");
        let parent = path.parent().expect("parent");
        fs::create_dir_all(parent).expect("parent");
        fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).expect("open folder");
        fs::write(&path, "{}").expect("existing store");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("open store");

        save_atomic(&path, &AccountStore::default()).expect("save");
        fn mode(path: &Path) -> u32 {
            fs::metadata(path).expect("metadata").permissions().mode() & 0o777
        }
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(parent), 0o700);

        let pending = parent.join("pending-credentials");
        let reference = new_pending_credential_reference();
        write_pending_credential(&pending, &reference, &json!({"credential": {}}))
            .expect("pending");
        assert_eq!(mode(&pending), 0o700);
        assert_eq!(
            mode(&pending_credential_path(&pending, &reference).expect("path")),
            0o600
        );

        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn replaces_existing_store() {
        let path = temp_store_path("replace");

        save_atomic(&path, &AccountStore::default()).expect("first save");

        let replacement = AccountStore {
            version: STORE_VERSION,
            accounts: vec![account("two", json!({"tokens": {"access_token": "new"}}))],
            active_account_id: None,
            pending_switch: None,
            pending_reset_credit: None,
        };
        save_atomic(&path, &replacement).expect("replacement save");

        let loaded = load(&path).expect("load replacement");
        assert_eq!(loaded.accounts.len(), 1);
        assert_eq!(loaded.accounts[0].id, "two");

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn loads_accounts_saved_before_identity_and_version_existed() {
        let path = temp_store_path("legacy-metadata");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(
            &path,
            r#"{
                "accounts": [{
                    "id": "legacy",
                    "label": "Legacy",
                    "kind": "chat_gpt",
                    "credential": {"tokens": {"access_token": "secret"}}
                }]
            }"#,
        )
        .expect("write legacy store");

        let loaded = load(&path).expect("load legacy store");
        assert_eq!(loaded.version, 0);
        assert_eq!(loaded.accounts.len(), 1);
        assert_eq!(loaded.accounts[0].identity, None);
        assert_eq!(loaded.accounts[0].quota, None);

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_compatible_old_store_is_upgraded_on_the_next_atomic_write() {
        let path = temp_store_path("upgrade-on-write");
        let store = AccountStore {
            version: 2,
            accounts: vec![account("one", json!({"token": "old"}))],
            active_account_id: None,
            pending_switch: None,
            pending_reset_credit: None,
        };

        save_atomic(&path, &store).expect("save");
        assert_eq!(load(&path).expect("load").version, STORE_VERSION);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_newer_store_is_refused() {
        let path = temp_store_path("newer-store");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, r#"{"version": 6, "accounts": []}"#).expect("newer store");
        assert_eq!(
            load(&path).err().as_deref(),
            Some("This GSwitch account store was created by a newer version")
        );
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn pending_reset_secrets_round_trip() {
        let path = temp_store_path("pending-reset-credit");
        let store = AccountStore {
            version: STORE_VERSION,
            accounts: vec![account("one", json!({"token": "current"}))],
            active_account_id: None,
            pending_switch: None,
            pending_reset_credit: Some(PendingResetCredit {
                account_id: "one".into(),
                credit_id: "provider-private-id".into(),
                idempotency_key: "7e6dff14-928a-4593-846a-5cae9cf0f9c9".into(),
                created_at_unix_ms: 123,
                discard_allowed: false,
            }),
        };

        save_atomic(&path, &store).expect("save");
        let pending = load(&path)
            .expect("load")
            .pending_reset_credit
            .expect("pending reset credit");
        assert_eq!(pending.account_id, "one");
        assert_eq!(pending.credit_id, "provider-private-id");
        assert_eq!(
            pending.idempotency_key,
            "7e6dff14-928a-4593-846a-5cae9cf0f9c9"
        );

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn pending_credentials_round_trip_and_retire_by_reference() {
        let path = temp_store_path("pending-credentials");
        let dir = path.parent().expect("parent").join("pending-credentials");
        let first = new_pending_credential_reference();
        let second = new_pending_credential_reference();
        write_pending_credential(&dir, &first, &json!({"credential": {"token": "one"}}))
            .expect("first");
        write_pending_credential(&dir, &second, &json!({"credential": {"token": "two"}}))
            .expect("second");

        let pending: Vec<(String, Value)> = pending_credentials(&dir).expect("list");
        assert_eq!(pending.len(), 2);
        assert!(pending.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let read: Value = read_pending_credential(&dir, &first).expect("read");
        assert_eq!(read["credential"]["token"], "one");

        retire_pending_credential(&dir, &first).expect("retire");
        assert!(retire_pending_credential(&dir, &first).is_err());
        let pending: Vec<(String, Value)> = pending_credentials(&dir).expect("list");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, second);
        assert!(write_pending_credential(&dir, "../outside", &json!({})).is_err());

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn an_unexpected_pending_entry_blocks_recovery() {
        let path = temp_store_path("pending-unexpected");
        let dir = path.parent().expect("parent").join("pending-credentials");
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("not-a-reference.json"), "{}").expect("unexpected entry");
        assert!(pending_credentials::<Value>(&dir).is_err());
        fs::remove_file(dir.join("not-a-reference.json")).expect("remove");
        fs::create_dir(dir.join(format!("{}.json", Uuid::new_v4()))).expect("directory entry");
        assert!(pending_credentials::<Value>(&dir).is_err());
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn parse_errors_do_not_include_credential_contents() {
        let path = temp_store_path("parse-error");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, "{ secret-token").expect("write malformed store");

        let error = match load(&path) {
            Ok(_) => panic!("must reject malformed store"),
            Err(error) => error,
        };
        assert_eq!(error, "Unable to parse GSwitch account store");
        assert!(!error.contains("secret-token"));

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }
}
