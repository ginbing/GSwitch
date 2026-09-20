use std::{fs, io::Write, path::Path};

use atomic_write_file::OpenOptions;
use serde::{Deserialize, Serialize};

use crate::types::StoredAccount;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AccountStore {
    #[serde(default)]
    pub accounts: Vec<StoredAccount>,
}

pub fn load(path: &Path) -> Result<AccountStore, String> {
    if !path.exists() {
        return Ok(AccountStore::default());
    }

    let content = fs::read_to_string(path)
        .map_err(|error| format!("Unable to read {}: {error}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("Unable to parse {}: {error}", path.display()))
}

pub fn save_atomic(path: &Path, store: &AccountStore) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid storage path: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Unable to create {}: {error}", parent.display()))?;

    let content = serde_json::to_vec_pretty(store)
        .map_err(|error| format!("Unable to serialize account store: {error}"))?;

    let mut options = OpenOptions::new();

    #[cfg(unix)]
    {
        use atomic_write_file::unix::OpenOptionsExt as AtomicOpenOptionsExt;
        use std::os::unix::fs::OpenOptionsExt as StdOpenOptionsExt;

        AtomicOpenOptionsExt::preserve_mode(&mut options, false);
        StdOpenOptionsExt::mode(&mut options, 0o600);
    }

    let mut file = options.open(path).map_err(|error| {
        format!(
            "Unable to open {} for atomic write: {error}",
            path.display()
        )
    })?;

    file.write_all(&content)
        .map_err(|error| format!("Unable to write {}: {error}", path.display()))?;

    file.commit()
        .map_err(|error| format!("Unable to atomically replace {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AccountKind, StoredAccount};
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

    #[test]
    fn round_trips_complete_credential_document() {
        let path = temp_store_path("roundtrip");
        let credential = json!({
            "tokens": {"access_token": "secret", "refresh_token": "rotate-me"},
            "auth_mode": "chatgpt",
            "future_field": {"preserve": true}
        });
        let store = AccountStore {
            accounts: vec![StoredAccount {
                id: "one".into(),
                label: "Personal".into(),
                kind: AccountKind::ChatGpt,
                credential: credential.clone(),
            }],
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
            accounts: vec![StoredAccount {
                id: "two".into(),
                label: "Work".into(),
                kind: AccountKind::ChatGpt,
                credential: json!({"tokens": {"access_token": "new"}}),
            }],
        };
        save_atomic(&path, &replacement).expect("replacement save");

        let loaded = load(&path).expect("load replacement");
        assert_eq!(loaded.accounts.len(), 1);
        assert_eq!(loaded.accounts[0].label, "Work");

        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }
}
