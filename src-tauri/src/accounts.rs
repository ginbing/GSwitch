use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use uuid::Uuid;

use crate::{
    storage::{self, AccountStore},
    types::{AccountKind, AccountView, OAuthLoginStatus, StoredAccount},
};

#[derive(Clone)]
pub struct AppState {
    store_path: Arc<PathBuf>,
    store: Arc<Mutex<AccountStore>>,
    oauth_logins: Arc<Mutex<HashMap<String, OAuthLoginStatus>>>,
}

impl AppState {
    pub fn new(store_path: PathBuf) -> Result<Self, String> {
        let store = storage::load(&store_path)?;
        Ok(Self {
            store_path: Arc::new(store_path),
            store: Arc::new(Mutex::new(store)),
            oauth_logins: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn list(&self) -> Result<Vec<AccountView>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is poisoned".to_string())?;

        Ok(store.accounts.iter().map(Self::view).collect())
    }

    pub fn add_account(
        &self,
        label: String,
        kind: AccountKind,
        email: Option<String>,
        plan_type: Option<String>,
        credential: serde_json::Value,
    ) -> Result<AccountView, String> {
        let account = StoredAccount {
            id: Uuid::new_v4().to_string(),
            label,
            kind,
            email,
            plan_type,
            credential,
        };

        let mut store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is poisoned".to_string())?;
        store.accounts.push(account.clone());
        storage::save_atomic(&self.store_path, &store)?;
        Ok(Self::view(&account))
    }

    pub fn set_oauth_status(&self, login_id: String, status: OAuthLoginStatus) -> Result<(), String> {
        let mut logins = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is poisoned".to_string())?;
        logins.insert(login_id, status);
        Ok(())
    }

    pub fn oauth_status(&self, login_id: &str) -> Result<OAuthLoginStatus, String> {
        let logins = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is poisoned".to_string())?;
        Ok(logins
            .get(login_id)
            .cloned()
            .unwrap_or(OAuthLoginStatus::Failed {
                message: "Unknown login session".to_string(),
            }))
    }

    fn view(account: &StoredAccount) -> AccountView {
        AccountView {
            id: account.id.clone(),
            label: account.label.clone(),
            kind: account.kind.clone(),
            email: account.email.clone(),
            plan_type: account.plan_type.clone(),
            active: false,
        }
    }
}
