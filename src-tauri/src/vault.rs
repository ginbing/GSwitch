#[cfg(test)]
use std::sync::Once;
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use fs2::FileExt;
use iota_stronghold::{Client, KeyProvider, SnapshotPath, Stronghold};
#[cfg(not(test))]
use keyring::{Entry, Error as KeyringError};
use serde::{de::DeserializeOwned, Serialize};
use zeroize::Zeroizing;

use crate::storage;

const CLIENT_PATH: &[u8] = b"gswitch-credentials";
const PENDING_CREDENTIAL_INDEX: &[u8] = b"pending-credential-index-v1";
#[cfg(not(test))]
const ROOT_SERVICE: &str = "GSwitch credential vault";
#[cfg(not(test))]
const ROOT_USER: &str = "root-key-v1";
// Stronghold owns process-global runtime resources. Serialize snapshot access
// across windows and deterministic test fixtures as well as per-vault writes.
static STRONGHOLD_LOCK: Mutex<()> = Mutex::new(());
#[cfg(test)]
static TEST_WORK_FACTOR: Once = Once::new();

#[derive(Clone)]
pub struct CredentialVault {
    snapshot_path: Arc<PathBuf>,
    root: RootSecret,
    lock: Arc<Mutex<()>>,
}

#[derive(Clone)]
enum RootSecret {
    #[cfg(not(test))]
    Native,
    #[cfg(test)]
    Fixed(Arc<Vec<u8>>),
}

impl CredentialVault {
    #[cfg(not(test))]
    pub fn new(snapshot_path: PathBuf) -> Self {
        Self {
            snapshot_path: Arc::new(snapshot_path),
            root: RootSecret::Native,
            lock: Arc::new(Mutex::new(())),
        }
    }

    #[cfg(test)]
    pub fn with_test_root(snapshot_path: PathBuf) -> Self {
        TEST_WORK_FACTOR.call_once(|| {
            let _ = iota_stronghold::engine::snapshot::try_set_encrypt_work_factor(0);
        });
        Self {
            snapshot_path: Arc::new(snapshot_path),
            root: RootSecret::Fixed(Arc::new(vec![7; 32])),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn put<T: Serialize>(&self, reference: &str, value: &T) -> Result<(), String> {
        let bytes = serde_json::to_vec(value)
            .map_err(|_| "Unable to serialize protected credential material".to_string())?;
        self.with_client(true, |stronghold, client, key| {
            client
                .store()
                .insert(reference.as_bytes().to_vec(), bytes, None)
                .map_err(|_| "Unable to write the protected credential vault".to_string())?;
            self.commit(stronghold, key)
        })
    }

    pub fn put_pending_credential<T: Serialize>(
        &self,
        reference: &str,
        value: &T,
    ) -> Result<(), String> {
        let bytes = serde_json::to_vec(value)
            .map_err(|_| "Unable to serialize protected credential material".to_string())?;
        self.with_client(true, |stronghold, client, key| {
            let store = client.store();
            let mut references = match store
                .get(PENDING_CREDENTIAL_INDEX)
                .map_err(|_| "Unable to read protected credential recovery".to_string())?
            {
                Some(bytes) => serde_json::from_slice::<Vec<String>>(&bytes)
                    .map_err(|_| "Protected credential recovery is incomplete".to_string())?,
                None => Vec::new(),
            };
            if !references.iter().any(|saved| saved == reference) {
                references.push(reference.to_string());
            }
            let index = serde_json::to_vec(&references)
                .map_err(|_| "Unable to update protected credential recovery".to_string())?;
            store
                .insert(reference.as_bytes().to_vec(), bytes, None)
                .and_then(|_| store.insert(PENDING_CREDENTIAL_INDEX.to_vec(), index, None))
                .map_err(|_| "Unable to update protected credential recovery".to_string())?;
            self.commit(stronghold, key)
        })
    }

    pub fn pending_credentials<T: DeserializeOwned>(&self) -> Result<Vec<(String, T)>, String> {
        if !self.snapshot_path.exists() {
            return Ok(Vec::new());
        }
        self.with_client(false, |_stronghold, client, _key| {
            let store = client.store();
            let references = match store
                .get(PENDING_CREDENTIAL_INDEX)
                .map_err(|_| "Unable to read protected credential recovery".to_string())?
            {
                Some(bytes) => serde_json::from_slice::<Vec<String>>(&bytes)
                    .map_err(|_| "Protected credential recovery is incomplete".to_string())?,
                None => Vec::new(),
            };
            let mut pending = Vec::with_capacity(references.len());
            for reference in references {
                if !reference.starts_with("pending-credential-")
                    || pending.iter().any(|(saved, _)| saved == &reference)
                {
                    return Err("Protected credential recovery is incomplete".to_string());
                }
                let bytes = store
                    .get(reference.as_bytes())
                    .map_err(|_| "Unable to read protected credential recovery".to_string())?
                    .ok_or_else(|| "Protected credential recovery is incomplete".to_string())?;
                let value = serde_json::from_slice(&bytes)
                    .map_err(|_| "Protected credential recovery is incomplete".to_string())?;
                pending.push((reference, value));
            }
            Ok(pending)
        })
    }

    pub fn retire_pending_credential(&self, reference: &str) -> Result<(), String> {
        self.with_client(false, |stronghold, client, key| {
            let store = client.store();
            let bytes = store
                .get(PENDING_CREDENTIAL_INDEX)
                .map_err(|_| "Unable to read protected credential recovery".to_string())?
                .ok_or_else(|| "Protected credential recovery is incomplete".to_string())?;
            let mut references = serde_json::from_slice::<Vec<String>>(&bytes)
                .map_err(|_| "Protected credential recovery is incomplete".to_string())?;
            let before = references.len();
            references.retain(|saved| saved != reference);
            if references.len() + 1 != before {
                return Err("Protected credential recovery is incomplete".to_string());
            }
            let index = serde_json::to_vec(&references)
                .map_err(|_| "Unable to update protected credential recovery".to_string())?;
            store
                .insert(PENDING_CREDENTIAL_INDEX.to_vec(), index, None)
                .and_then(|_| store.delete(reference.as_bytes()))
                .map_err(|_| "Unable to update protected credential recovery".to_string())?;
            self.commit(stronghold, key)
        })
    }

    pub fn get<T: DeserializeOwned>(&self, reference: &str) -> Result<T, String> {
        self.with_client(false, |_stronghold, client, _key| {
            let bytes = client
                .store()
                .get(reference.as_bytes())
                .map_err(|_| "Unable to read the protected credential vault".to_string())?
                .ok_or_else(|| {
                    "A saved account is missing protected credential material".to_string()
                })?;
            serde_json::from_slice(&bytes)
                .map_err(|_| "Protected credential material could not be read safely".to_string())
        })
    }

    /// Read several records from one unlocked snapshot. Startup uses this so
    /// adding accounts does not repeat the native-key and snapshot setup.
    pub fn get_many<T: DeserializeOwned>(&self, references: &[String]) -> Result<Vec<T>, String> {
        if references.is_empty() {
            return Ok(Vec::new());
        }
        self.with_client(false, |_stronghold, client, _key| {
            let store = client.store();
            references
                .iter()
                .map(|reference| {
                    let bytes = store
                        .get(reference.as_bytes())
                        .map_err(|_| "Unable to read the protected credential vault".to_string())?
                        .ok_or_else(|| {
                            "A saved account is missing protected credential material".to_string()
                        })?;
                    serde_json::from_slice(&bytes).map_err(|_| {
                        "Protected credential material could not be read safely".to_string()
                    })
                })
                .collect()
        })
    }

    pub fn delete(&self, reference: &str) -> Result<(), String> {
        self.with_client(false, |stronghold, client, key| {
            client
                .store()
                .delete(reference.as_bytes())
                .map_err(|_| "Unable to update the protected credential vault".to_string())?;
            self.commit(stronghold, key)
        })
    }

    fn with_client<T>(
        &self,
        create: bool,
        action: impl FnOnce(&Stronghold, Client, &KeyProvider) -> Result<T, String>,
    ) -> Result<T, String> {
        let _global_lock = STRONGHOLD_LOCK
            .lock()
            .map_err(|_| "Protected credential vault is unavailable".to_string())?;
        let _lock = self
            .lock
            .lock()
            .map_err(|_| "Protected credential vault is unavailable".to_string())?;
        let _process_lock = self.acquire_process_lock()?;
        let key = self.root_key(create)?;
        let key = KeyProvider::with_passphrase_hashed_blake2b(key)
            .map_err(|_| "Protected credential vault is unavailable".to_string())?;
        let stronghold = Stronghold::default();
        let client = if self.snapshot_path.exists() {
            stronghold
                .load_client_from_snapshot(
                    CLIENT_PATH,
                    &key,
                    &SnapshotPath::from_path(self.snapshot_path.as_ref()),
                )
                .map_err(|_| "Protected credential vault is unavailable or locked".to_string())?
        } else if create {
            stronghold
                .create_client(CLIENT_PATH)
                .map_err(|_| "Unable to create the protected credential vault".to_string())?
        } else {
            return Err("A saved account is missing protected credential material".to_string());
        };
        action(&stronghold, client, &key)
    }

    fn acquire_process_lock(&self) -> Result<std::fs::File, String> {
        let parent = self
            .snapshot_path
            .parent()
            .ok_or_else(|| "Invalid protected credential vault path".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|_| "Unable to prepare protected credential storage".to_string())?;
        let lock_path = self.snapshot_path.with_extension("lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path)
            .map_err(|_| "Unable to lock protected credential storage".to_string())?;
        FileExt::lock_exclusive(&file)
            .map_err(|_| "Unable to lock protected credential storage".to_string())?;
        Ok(file)
    }

    fn root_key(&self, create: bool) -> Result<Zeroizing<Vec<u8>>, String> {
        #[cfg(test)]
        let _ = create;
        match &self.root {
            #[cfg(not(test))]
            RootSecret::Native => {
                let entry = Entry::new(ROOT_SERVICE, ROOT_USER)
                    .map_err(|_| "Protected credential storage is unavailable".to_string())?;
                match entry.get_secret() {
                    Ok(secret) => {
                        let secret = Zeroizing::new(secret);
                        if secret.len() == 32 {
                            Ok(secret)
                        } else {
                            Err("Protected credential storage is unavailable".to_string())
                        }
                    }
                    Err(KeyringError::NoEntry) if create && !self.snapshot_path.exists() => {
                        let mut secret = Zeroizing::new(vec![0_u8; 32]);
                        getrandom::fill(&mut secret[..]).map_err(|_| {
                            "Protected credential storage is unavailable".to_string()
                        })?;
                        entry.set_secret(&secret).map_err(|_| {
                            "Protected credential storage is unavailable".to_string()
                        })?;
                        Ok(secret)
                    }
                    Err(_) => Err("Protected credential storage is unavailable".to_string()),
                }
            }
            #[cfg(test)]
            RootSecret::Fixed(secret) => Ok(Zeroizing::new((**secret).clone())),
        }
    }

    fn commit(&self, stronghold: &Stronghold, key: &KeyProvider) -> Result<(), String> {
        let parent = self
            .snapshot_path
            .parent()
            .ok_or_else(|| "Invalid protected credential vault path".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|_| "Unable to prepare protected credential storage".to_string())?;
        let temporary = self.snapshot_path.with_extension("hold.tmp");
        stronghold
            .write_client(CLIENT_PATH)
            .map_err(|_| "Unable to write the protected credential vault".to_string())?;
        stronghold
            .commit_with_keyprovider(&SnapshotPath::from_path(&temporary), key)
            .map_err(|_| "Unable to write the protected credential vault".to_string())?;
        let bytes = fs::read(&temporary)
            .map_err(|_| "Unable to write the protected credential vault".to_string())?;
        let result = storage::write_private_bytes_atomic(
            self.snapshot_path.as_ref(),
            &bytes,
            "protected credential vault",
        );
        let _ = fs::remove_file(temporary);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encrypted_snapshot_round_trips_complete_unknown_credential_fields() {
        let root = std::env::temp_dir().join(format!("gswitch-vault-{}", uuid::Uuid::new_v4()));
        let path = root.join("credentials.hold");
        let vault = CredentialVault::with_test_root(path.clone());
        let credential = json!({
            "tokens": {"access_token": "secret", "future": {"keep": true}},
            "unknown": ["preserved"]
        });

        vault.put("account:one:1", &credential).expect("write");
        assert!(path.exists());
        assert!(!String::from_utf8_lossy(&fs::read(&path).expect("snapshot")).contains("secret"));
        let read: serde_json::Value = vault.get("account:one:1").expect("read");
        assert_eq!(read, credential);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reads_multiple_accounts_from_one_snapshot_and_rejects_missing_material() {
        let root = std::env::temp_dir().join(format!("gswitch-vault-{}", uuid::Uuid::new_v4()));
        let vault = CredentialVault::with_test_root(root.join("credentials.hold"));
        vault
            .put("account:one:1", &json!({"token": "one"}))
            .expect("first");
        vault
            .put("account:two:1", &json!({"token": "two"}))
            .expect("second");

        let values: Vec<serde_json::Value> = vault
            .get_many(&["account:one:1".into(), "account:two:1".into()])
            .expect("both accounts");
        assert_eq!(
            values,
            vec![json!({"token": "one"}), json!({"token": "two"})]
        );
        assert!(vault
            .get_many::<serde_json::Value>(&["account:one:1".into(), "account:missing:1".into()])
            .is_err());

        let _ = fs::remove_dir_all(root);
    }
}
