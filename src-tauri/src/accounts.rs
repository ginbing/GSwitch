use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde_json::Value;
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

    pub fn add(
        &self,
        label: String,
        kind: AccountKind,
        email: Option<String>,
        plan_type: Option<String>,
        credential: Value,
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

        if store.accounts.iter().any(|existing| {
            existing.kind == account.kind
                && (existing.credential == account.credential
                    || (account.kind == AccountKind::ChatGpt
                        && existing.email.as_deref().is_some_and(|email| {
                            account
                                .email
                                .as_deref()
                                .is_some_and(|new_email| email.eq_ignore_ascii_case(new_email))
                        })))
        }) {
            return Err("This account is already saved".to_string());
        }

        let mut candidate = store.clone();
        candidate.accounts.push(account.clone());
        storage::save_atomic(&self.store_path, &candidate)?;
        *store = candidate;
        Ok(Self::view(&account))
    }

    pub fn set_oauth_status(
        &self,
        login_id: String,
        status: OAuthLoginStatus,
    ) -> Result<(), String> {
        let mut sessions = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is poisoned".to_string())?;
        sessions.insert(login_id, status);
        Ok(())
    }

    pub fn oauth_status(&self, login_id: &str) -> Result<OAuthLoginStatus, String> {
        let sessions = self
            .oauth_logins
            .lock()
            .map_err(|_| "OAuth state lock is poisoned".to_string())?;

        sessions
            .get(login_id)
            .cloned()
            .ok_or_else(|| "Unknown OAuth login session".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "gswitch-accounts-{name}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ))
            .join("accounts.json")
    }

    #[test]
    fn rejects_duplicate_chatgpt_email_case_insensitively() {
        let path = temp_path("duplicate");
        let state = AppState::new(path.clone()).expect("state");

        state
            .add(
                "Personal".into(),
                AccountKind::ChatGpt,
                Some("User@example.com".into()),
                Some("plus".into()),
                serde_json::json!({"tokens": {"access_token": "first"}}),
            )
            .expect("first account");

        let result = state.add(
            "Duplicate".into(),
            AccountKind::ChatGpt,
            Some("user@example.com".into()),
            Some("plus".into()),
            serde_json::json!({"tokens": {"access_token": "second"}}),
        );

        assert_eq!(
            result.expect_err("duplicate must fail"),
            "This account is already saved"
        );
        assert_eq!(state.list().expect("list").len(), 1);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn failed_persistence_does_not_change_in_memory_store() {
        let path = temp_path("failed-save");
        let parent = path.parent().expect("parent");
        fs::create_dir_all(parent.parent().expect("root")).expect("create root");
        fs::write(parent, b"blocks directory creation").expect("write blocking file");
        let state = AppState::new(path.clone()).expect("state");

        let result = state.add(
            "Personal".into(),
            AccountKind::ChatGpt,
            Some("user@example.com".into()),
            Some("plus".into()),
            serde_json::json!({"tokens": {"access_token": "secret"}}),
        );

        assert!(result.is_err());
        assert!(state.list().expect("list").is_empty());
        let _ = fs::remove_file(parent);
    }
}
