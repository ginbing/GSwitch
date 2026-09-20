use std::{path::PathBuf, sync::Mutex};

use crate::{
    storage::{self, AccountStore},
    types::AccountView,
};

pub struct AppState {
    store_path: PathBuf,
    store: Mutex<AccountStore>,
}

impl AppState {
    pub fn new(store_path: PathBuf) -> Result<Self, String> {
        let store = storage::load(&store_path)?;
        Ok(Self {
            store_path,
            store: Mutex::new(store),
        })
    }

    pub fn list(&self) -> Result<Vec<AccountView>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is poisoned".to_string())?;

        Ok(store
            .accounts
            .iter()
            .map(|account| AccountView {
                id: account.id.clone(),
                label: account.label.clone(),
                kind: account.kind.clone(),
                active: false,
            })
            .collect())
    }

    pub fn persist(&self) -> Result<(), String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Account store lock is poisoned".to_string())?;
        storage::save_atomic(&self.store_path, &store)
    }
}
