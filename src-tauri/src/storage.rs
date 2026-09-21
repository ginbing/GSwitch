use std::{
    fs,
    io::{ErrorKind, Write},
    path::Path,
};

use atomic_write_file::OpenOptions;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::types::{PendingResetCredit, PendingSwitch, StoredAccount};

const STORE_VERSION: u32 = 3;

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
    STORE_VERSION
}

pub fn load(path: &Path) -> Result<AccountStore, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(AccountStore::default()),
        Err(_) => return Err("Unable to read the GSwitch account store".to_string()),
    };

    let store: AccountStore = deserialize(&content, "GSwitch account store")?;
    if store.version > STORE_VERSION {
        return Err("This GSwitch account store was created by a newer version".to_string());
    }
    Ok(store)
}

pub fn save_atomic(path: &Path, store: &AccountStore) -> Result<(), String> {
    // Every successful write upgrades an older compatible store. This keeps
    // migrations explicit without treating an old, valid account library as
    // disposable data.
    let mut current = store.clone();
    current.version = STORE_VERSION;
    let content = serde_json::to_vec_pretty(&current)
        .map_err(|_| "Unable to serialize the GSwitch account store".to_string())?;
    write_private_bytes_atomic(path, &content, "GSwitch account store")
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
            fs::create_dir_all(recovery_dir)
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
            identity: Some(AccountIdentity::ChatGpt {
                user_id: "user".into(),
                workspace_id: Some("workspace".into()),
            }),
            quota: None,
            reset_credits: None,
            credential,
        }
    }

    #[test]
    fn round_trips_complete_credential_document() {
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
        assert_eq!(loaded.accounts[0].credential, credential);

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
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
        assert_eq!(loaded.version, STORE_VERSION);
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
    fn preserves_a_pending_reset_credit_for_idempotent_recovery() {
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
