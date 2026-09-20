use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

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

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let temp_path = temp_path(path, suffix);

    fs::write(&temp_path, content)
        .map_err(|error| format!("Unable to write {}: {error}", temp_path.display()))?;

    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("Unable to replace {}: {error}", path.display()));
    }

    Ok(())
}

fn temp_path(path: &Path, suffix: u128) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("accounts.json");
    path.with_file_name(format!(".{file_name}.{suffix}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AccountKind, StoredAccount};
    use serde_json::json;
    use std::env;

    #[test]
    fn round_trips_complete_credential_document() {
        let root = env::temp_dir().join(format!(
            "gswitch-store-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let path = root.join("accounts.json");
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

        let _ = fs::remove_dir_all(root);
    }
}
