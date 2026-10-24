//! Read-only access to the Stronghold vault that GSwitch 1.0.30 and earlier
//! used for saved credentials. Startup reads it once to move a version-4 store
//! into the owner-only account file, then removes it and its root key.
#[cfg(test)]
use std::sync::Once;
use std::{fs, path::PathBuf, sync::Mutex};

use fs2::FileExt;
use iota_stronghold::{Client, KeyProvider, SnapshotPath, Stronghold};
#[cfg(not(test))]
use keyring::Entry;
use serde_json::Value;
use zeroize::Zeroizing;

const CLIENT_PATH: &[u8] = b"gswitch-credentials";
const PENDING_CREDENTIAL_INDEX: &[u8] = b"pending-credential-index-v1";
/// Pending credential entries by reference, in index order.
type PendingEntries = Vec<(String, Value)>;
#[cfg(not(test))]
const ROOT_SERVICE: &str = "GSwitch credential vault";
#[cfg(not(test))]
const ROOT_USER: &str = "root-key-v1";
// Stronghold owns process-global runtime resources. Serialize snapshot access
// across deterministic test fixtures as well.
static STRONGHOLD_LOCK: Mutex<()> = Mutex::new(());
#[cfg(test)]
static TEST_WORK_FACTOR: Once = Once::new();

pub struct LegacyVault {
    snapshot_path: PathBuf,
    root: RootSecret,
}

enum RootSecret {
    #[cfg(not(test))]
    Native,
    #[cfg(test)]
    Fixed(Vec<u8>),
}

impl LegacyVault {
    #[cfg(not(test))]
    pub fn new(snapshot_path: PathBuf) -> Self {
        Self {
            snapshot_path,
            root: RootSecret::Native,
        }
    }

    #[cfg(test)]
    pub fn with_test_root(snapshot_path: PathBuf) -> Self {
        TEST_WORK_FACTOR.call_once(|| {
            let _ = iota_stronghold::engine::snapshot::try_set_encrypt_work_factor(0);
        });
        Self {
            snapshot_path,
            root: RootSecret::Fixed(vec![7; 32]),
        }
    }

    pub fn exists(&self) -> bool {
        fs::symlink_metadata(&self.snapshot_path).is_ok()
    }

    /// Resolves a version-4 store's vault references in place: complete
    /// credentials, reset credits, switch rollback auth and reset-credit
    /// secrets become inline fields. Returns the pending credential entries.
    /// Nothing is written, so a failure leaves the store and vault unchanged.
    pub fn resolve(&self, store: &mut Value) -> Result<PendingEntries, String> {
        let incomplete = || "Legacy vault metadata is incomplete".to_string();
        let mut references = Vec::new();
        for account in store
            .get("accounts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if has_value(account, "credential") || has_value(account, "reset_credits") {
                return Err(incomplete());
            }
            references.push(
                reference(account, "credential_ref", "credential_generation")
                    .ok_or_else(incomplete)?,
            );
        }
        let switch_reference = match store.get("pending_switch").filter(|value| !value.is_null()) {
            Some(pending)
                if pending
                    .get("secret_ref")
                    .is_some_and(|value| !value.is_null()) =>
            {
                if has_value(pending, "previous_auth") {
                    return Err(incomplete());
                }
                Some(reference(pending, "secret_ref", "secret_generation").ok_or_else(incomplete)?)
            }
            Some(pending)
                if has_value(pending, "previous_auth")
                    || pending
                        .get("secret_generation")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        != 0 =>
            {
                return Err(incomplete());
            }
            _ => None,
        };
        let reset_reference = match store
            .get("pending_reset_credit")
            .filter(|value| !value.is_null())
        {
            Some(pending) => {
                if has_text(pending, "credit_id") || has_text(pending, "idempotency_key") {
                    return Err(incomplete());
                }
                Some(reference(pending, "secret_ref", "secret_generation").ok_or_else(incomplete)?)
            }
            None => None,
        };
        let account_count = references.len();
        references.extend(switch_reference.iter().cloned());
        references.extend(reset_reference.iter().cloned());

        let (mut records, pending) = self.read(&references)?;
        let reset_record = reset_reference.map(|_| records.pop().expect("reset record"));
        let switch_record = switch_reference.map(|_| records.pop().expect("switch record"));
        debug_assert_eq!(records.len(), account_count);

        for (account, record) in store
            .get_mut("accounts")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
            .zip(records)
        {
            let account = account.as_object_mut().ok_or_else(incomplete)?;
            account.insert(
                "credential".into(),
                record.get("credential").cloned().ok_or_else(incomplete)?,
            );
            if let Some(credits) = record.get("reset_credits").filter(|value| !value.is_null()) {
                account.insert("reset_credits".into(), credits.clone());
            }
            account.remove("credential_ref");
        }
        if let Some(record) = switch_record {
            let pending = store
                .get_mut("pending_switch")
                .and_then(Value::as_object_mut)
                .ok_or_else(incomplete)?;
            if let Some(previous) = record.get("previous_auth").filter(|value| !value.is_null()) {
                pending.insert("previous_auth".into(), previous.clone());
            }
            pending.remove("secret_ref");
            pending.remove("secret_generation");
        }
        if let Some(record) = reset_record {
            let pending = store
                .get_mut("pending_reset_credit")
                .and_then(Value::as_object_mut)
                .ok_or_else(incomplete)?;
            for field in ["credit_id", "idempotency_key"] {
                let value = record
                    .get(field)
                    .filter(|value| value.as_str().is_some_and(|text| !text.is_empty()))
                    .ok_or_else(incomplete)?;
                pending.insert(field.into(), value.clone());
            }
            pending.remove("secret_ref");
            pending.remove("secret_generation");
        }
        Ok(pending)
    }

    /// Removes the snapshot first, so a failure is retried at the next start
    /// without asking the credential manager again. The root key is then
    /// removed once; without the snapshot it protects nothing.
    pub fn remove(&self) -> Result<(), String> {
        for path in [
            self.snapshot_path.clone(),
            self.snapshot_path.with_extension("hold.tmp"),
            self.snapshot_path.with_extension("lock"),
        ] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("Unable to remove the previous credential vault".to_string()),
            }
        }
        #[cfg(not(test))]
        if let Ok(entry) = Entry::new(ROOT_SERVICE, ROOT_USER) {
            let _ = entry.delete_credential();
        }
        Ok(())
    }

    /// Reads the requested records and every pending credential from one
    /// unlocked snapshot.
    fn read(&self, references: &[String]) -> Result<(Vec<Value>, PendingEntries), String> {
        if !self.snapshot_path.exists() {
            return if references.is_empty() {
                Ok((Vec::new(), Vec::new()))
            } else {
                Err("A saved account is missing protected credential material".to_string())
            };
        }
        self.with_client(false, |client| {
            let store = client.store();
            let get = |reference: &[u8]| {
                store
                    .get(reference)
                    .map_err(|_| "Unable to read the protected credential vault".to_string())
            };
            let parse = |bytes: Vec<u8>| {
                serde_json::from_slice::<Value>(&bytes).map_err(|_| {
                    "Protected credential material could not be read safely".to_string()
                })
            };
            let records = references
                .iter()
                .map(|reference| {
                    get(reference.as_bytes())?
                        .ok_or_else(|| {
                            "A saved account is missing protected credential material".to_string()
                        })
                        .and_then(parse)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let index = match get(PENDING_CREDENTIAL_INDEX)? {
                Some(bytes) => serde_json::from_slice::<Vec<String>>(&bytes)
                    .map_err(|_| "Protected credential recovery is incomplete".to_string())?,
                None => Vec::new(),
            };
            let mut pending: PendingEntries = Vec::with_capacity(index.len());
            for reference in index {
                if !reference.starts_with("pending-credential-")
                    || pending.iter().any(|(saved, _)| saved == &reference)
                {
                    return Err("Protected credential recovery is incomplete".to_string());
                }
                let value = get(reference.as_bytes())?
                    .ok_or_else(|| "Protected credential recovery is incomplete".to_string())
                    .and_then(parse)?;
                pending.push((reference, value));
            }
            Ok((records, pending))
        })
    }

    fn with_client<T>(
        &self,
        create: bool,
        action: impl FnOnce(Client) -> Result<T, String>,
    ) -> Result<T, String> {
        let _global_lock = STRONGHOLD_LOCK
            .lock()
            .map_err(|_| "Protected credential vault is unavailable".to_string())?;
        let _process_lock = self.acquire_process_lock()?;
        let key = KeyProvider::with_passphrase_hashed_blake2b(self.root_key()?)
            .map_err(|_| "Protected credential vault is unavailable".to_string())?;
        let stronghold = Stronghold::default();
        let client = if self.snapshot_path.exists() {
            stronghold
                .load_client_from_snapshot(
                    CLIENT_PATH,
                    &key,
                    &SnapshotPath::from_path(&self.snapshot_path),
                )
                .map_err(|_| "Protected credential vault is unavailable or locked".to_string())?
        } else if create {
            stronghold
                .create_client(CLIENT_PATH)
                .map_err(|_| "Unable to create the protected credential vault".to_string())?
        } else {
            return Err("A saved account is missing protected credential material".to_string());
        };
        let result = action(client)?;
        #[cfg(test)]
        if create {
            self.commit(&stronghold, &key)?;
        }
        Ok(result)
    }

    fn acquire_process_lock(&self) -> Result<std::fs::File, String> {
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.snapshot_path.with_extension("lock"))
            .map_err(|_| "Unable to lock protected credential storage".to_string())?;
        FileExt::lock_exclusive(&file)
            .map_err(|_| "Unable to lock protected credential storage".to_string())?;
        Ok(file)
    }

    fn root_key(&self) -> Result<Zeroizing<Vec<u8>>, String> {
        match &self.root {
            #[cfg(not(test))]
            RootSecret::Native => {
                let secret = Entry::new(ROOT_SERVICE, ROOT_USER)
                    .and_then(|entry| entry.get_secret())
                    .map(Zeroizing::new)
                    .map_err(|_| "Protected credential storage is unavailable".to_string())?;
                if secret.len() == 32 {
                    Ok(secret)
                } else {
                    Err("Protected credential storage is unavailable".to_string())
                }
            }
            #[cfg(test)]
            RootSecret::Fixed(secret) => Ok(Zeroizing::new(secret.clone())),
        }
    }
}

fn has_value(record: &Value, field: &str) -> bool {
    record.get(field).is_some_and(|value| !value.is_null())
}

fn has_text(record: &Value, field: &str) -> bool {
    record
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
}

fn reference(record: &Value, name: &str, generation: &str) -> Option<String> {
    let name = record.get(name)?.as_str().filter(|name| !name.is_empty())?;
    let generation = record
        .get(generation)?
        .as_u64()
        .filter(|value| *value != 0)?;
    Some(format!("{name}:{generation}"))
}

/// Writes fixtures with the real Stronghold format, the way GSwitch 1.0.30
/// did, so migration tests start from a genuine vault.
#[cfg(test)]
impl LegacyVault {
    pub fn put(&self, reference: &str, value: &Value) -> Result<(), String> {
        let bytes = serde_json::to_vec(value).expect("fixture");
        self.with_client(true, |client| {
            client
                .store()
                .insert(reference.as_bytes().to_vec(), bytes, None)
                .map_err(|_| "Unable to write the protected credential vault".to_string())
                .map(|_| ())
        })
    }

    pub fn put_pending_credential(&self, reference: &str, value: &Value) -> Result<(), String> {
        let bytes = serde_json::to_vec(value).expect("fixture");
        self.with_client(true, |client| {
            let store = client.store();
            let mut index = store
                .get(PENDING_CREDENTIAL_INDEX)
                .expect("index")
                .map(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).expect("index"))
                .unwrap_or_default();
            index.push(reference.to_string());
            store
                .insert(reference.as_bytes().to_vec(), bytes, None)
                .and_then(|_| {
                    store.insert(
                        PENDING_CREDENTIAL_INDEX.to_vec(),
                        serde_json::to_vec(&index).expect("index"),
                        None,
                    )
                })
                .map_err(|_| "Unable to update protected credential recovery".to_string())
                .map(|_| ())
        })
    }

    fn commit(&self, stronghold: &Stronghold, key: &KeyProvider) -> Result<(), String> {
        let temporary = self.snapshot_path.with_extension("hold.tmp");
        stronghold
            .write_client(CLIENT_PATH)
            .map_err(|_| "Unable to write the protected credential vault".to_string())?;
        stronghold
            .commit_with_keyprovider(&SnapshotPath::from_path(&temporary), key)
            .map_err(|_| "Unable to write the protected credential vault".to_string())?;
        fs::rename(&temporary, &self.snapshot_path)
            .map_err(|_| "Unable to write the protected credential vault".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("gswitch-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).expect("fixture directory");
        root
    }

    #[test]
    fn reads_an_unmodified_legacy_snapshot_with_unknown_fields() {
        // Generated by published iota_stronghold 2.1.0 / iota-crypto 0.23.2,
        // without the k256 patch. Public synthetic data, root key [7; 32].
        let root = root("legacy-vault");
        let path = root.join("credentials.hold");
        fs::write(
            &path,
            include_bytes!("../tests/fixtures/legacy-stronghold.hold"),
        )
        .expect("legacy fixture");
        let vault = LegacyVault::with_test_root(path);
        let (records, pending) = vault.read(&["legacy-vector".into()]).expect("legacy read");
        assert_eq!(
            records,
            vec![
                json!({"tokens":{"access_token":"FAKE-LEGACY-VECTOR","future":{"preserve":true}},"unknown":[1,2,3]})
            ]
        );
        assert!(pending.is_empty());
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn missing_material_is_an_error_and_removal_deletes_the_snapshot() {
        let root = root("vault-missing");
        let vault = LegacyVault::with_test_root(root.join("credentials.hold"));
        assert!(vault.read(&["account:one:1".into()]).is_err());
        assert_eq!(vault.read(&[]).expect("empty").0.len(), 0);

        vault
            .put("account:one:1", &json!({"credential": {"token": "one"}}))
            .expect("fixture");
        assert!(!String::from_utf8_lossy(
            &fs::read(root.join("credentials.hold")).expect("snapshot")
        )
        .contains("one"));
        assert!(vault
            .read(&["account:one:1".into(), "account:missing:1".into()])
            .is_err());
        assert!(vault.exists());
        vault.remove().expect("remove");
        assert!(!vault.exists());
        assert!(!root.join("credentials.lock").exists());
        vault.remove().expect("removal is idempotent");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn resolve_rejects_inline_secrets_and_missing_references() {
        let root = root("vault-resolve");
        let vault = LegacyVault::with_test_root(root.join("credentials.hold"));
        for mut store in [
            json!({"version": 4, "accounts": [{"id": "a", "credential_generation": 1}]}),
            json!({"version": 4, "accounts": [{"id": "a", "credential_ref": "account:a", "credential_generation": 1, "credential": {"token": "inline"}}]}),
            json!({"version": 4, "accounts": [], "pending_reset_credit": {"account_id": "a", "created_at_unix_ms": 1}}),
            json!({"version": 4, "accounts": [], "pending_switch": {"previous_auth": {"token": "inline"}}}),
        ] {
            let before = store.clone();
            assert!(vault.resolve(&mut store).is_err());
            assert_eq!(store, before);
        }
        fs::remove_dir_all(root).expect("fixture cleanup");
    }
}
