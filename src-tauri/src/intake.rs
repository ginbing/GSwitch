use std::{thread, time::Duration};

use serde_json::{json, Value};

use crate::{
    accounts::{AccountDraft, AppState, OperationGuard},
    app_server::{account_metadata, AccountMetadata, AppServer, TempCodexHome},
    identity,
    types::{AccountIdentity, AccountView, OAuthLoginStart, OAuthLoginStatus},
};

const OAUTH_COMPLETION_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub fn start_oauth(state: AppState) -> Result<OAuthLoginStart, String> {
    let profile = TempCodexHome::create(&state.isolated_profile_root()?)?;
    let mut server = AppServer::start(&profile.path)?;

    let result = server.call(
        1,
        "account/login/start",
        json!({
            "type": "chatgpt",
            "useHostedLoginSuccessPage": true,
            "appBrand": "chatgpt"
        }),
        Duration::from_secs(30),
    )?;
    let login_id = result
        .get("loginId")
        .and_then(Value::as_str)
        .ok_or_else(|| "Codex did not return a login session".to_string())?
        .to_string();
    let auth_url = result
        .get("authUrl")
        .and_then(Value::as_str)
        .filter(|url| url.starts_with("https://"))
        .ok_or_else(|| "Codex returned an unsupported authorization URL".to_string())?
        .to_string();

    state.set_oauth_status(login_id.clone(), OAuthLoginStatus::Pending)?;

    let monitor_id = login_id.clone();
    thread::spawn(move || {
        if let Err(message) = monitor_oauth(&mut server, profile, &state, &monitor_id) {
            let _ = state.set_oauth_status(monitor_id, OAuthLoginStatus::Failed { message });
        }
    });

    Ok(OAuthLoginStart { login_id, auth_url })
}

fn monitor_oauth(
    server: &mut AppServer,
    mut profile: TempCodexHome,
    state: &AppState,
    login_id: &str,
) -> Result<(), String> {
    let notification = server.wait_for_notification(OAUTH_COMPLETION_TIMEOUT, |message| {
        message.get("method").and_then(Value::as_str) == Some("account/login/completed")
            && message.pointer("/params/loginId").and_then(Value::as_str) == Some(login_id)
    })?;
    let success = notification
        .pointer("/params/success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !success {
        return Err("OAuth sign-in did not complete".to_string());
    }

    let operation = state.acquire_operation()?;
    let result = server.account_read(2, false)?;
    let metadata = account_metadata(&result)?;
    let account = persist_validated(
        state,
        &operation,
        &mut profile,
        metadata,
        None,
        None,
        "ChatGPT account".to_string(),
    )?;
    state.set_oauth_status(login_id.to_string(), OAuthLoginStatus::Complete { account })
}

pub fn import_json(
    state: &AppState,
    raw_json: &str,
    label: Option<String>,
) -> Result<AccountView, String> {
    let credential: Value =
        serde_json::from_str(raw_json).map_err(|_| "Invalid auth JSON".to_string())?;
    if !credential.is_object() {
        return Err("Invalid auth JSON: expected a JSON object".to_string());
    }

    let expected_kind = identity::document_kind(&credential)?;
    let expected_identity = identity::derive_identity(&expected_kind, &credential)?;
    let operation = state.acquire_operation()?;
    if let Some(existing) = state.find_exact_document_under_operation(
        &operation,
        &expected_kind,
        &expected_identity,
        &credential,
    )? {
        return Ok(existing);
    }

    let mut profile = TempCodexHome::create(&state.isolated_profile_root()?)?;
    profile.write_auth(&credential)?;
    let mut server = AppServer::start(&profile.path)?;
    let result = server.account_read(1, true)?;
    let metadata = account_metadata(&result)?;
    if metadata.kind != expected_kind {
        return Err("The imported credential changed account type during validation".to_string());
    }

    persist_validated(
        state,
        &operation,
        &mut profile,
        metadata,
        Some(expected_identity),
        clean_label(label),
        "Imported account".to_string(),
    )
}

pub fn import_api_key(
    state: &AppState,
    api_key: &str,
    label: Option<String>,
) -> Result<AccountView, String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("API key is empty".to_string());
    }

    let operation = state.acquire_operation()?;
    let mut profile = TempCodexHome::create(&state.isolated_profile_root()?)?;
    let mut server = AppServer::start(&profile.path)?;
    server.call(
        1,
        "account/login/start",
        json!({"type": "apiKey", "apiKey": api_key}),
        Duration::from_secs(30),
    )?;

    let result = server.account_read(2, false)?;
    let metadata = account_metadata(&result)?;
    if metadata.kind != crate::types::AccountKind::ApiKey {
        return Err("Codex did not create an API-key account".to_string());
    }

    persist_validated(
        state,
        &operation,
        &mut profile,
        metadata,
        None,
        clean_label(label),
        "API key".to_string(),
    )
}

fn persist_validated(
    state: &AppState,
    operation: &OperationGuard<'_>,
    profile: &mut TempCodexHome,
    metadata: AccountMetadata,
    expected_identity: Option<AccountIdentity>,
    label: Option<String>,
    fallback_label: String,
) -> Result<AccountView, String> {
    let credential = profile.read_auth()?;
    let identity = identity::derive_identity(&metadata.kind, &credential)?;
    if expected_identity
        .as_ref()
        .is_some_and(|expected| expected != &identity)
    {
        return Err("The validated credentials do not match the imported account".to_string());
    }

    let default_label = metadata.email.clone().unwrap_or(fallback_label);
    let result = state.upsert_under_operation(
        operation,
        AccountDraft {
            label,
            default_label,
            kind: metadata.kind,
            email: metadata.email,
            plan_type: metadata.plan_type,
            identity,
            credential: credential.clone(),
        },
    );

    if result.is_err() {
        if state
            .record_pending_credential(operation, &credential)
            .is_err()
        {
            profile.retain_for_recovery();
        }
        return Err(
            "GSwitch could not save validated credentials. A protected recovery copy was retained."
                .to_string(),
        );
    }

    result
}

fn clean_label(label: Option<String>) -> Option<String> {
    label
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_json_before_starting_codex() {
        let path =
            std::env::temp_dir().join(format!("gswitch-invalid-{}.json", uuid::Uuid::new_v4()));
        let state = AppState::new(path).expect("state");

        let error = import_json(&state, "{not valid", None).expect_err("invalid JSON");
        assert_eq!(error, "Invalid auth JSON");
    }

    #[test]
    fn rejects_non_object_json_before_starting_codex() {
        let path =
            std::env::temp_dir().join(format!("gswitch-invalid-{}.json", uuid::Uuid::new_v4()));
        let state = AppState::new(path).expect("state");

        let error = import_json(&state, "[]", None).expect_err("invalid document");
        assert_eq!(error, "Invalid auth JSON: expected a JSON object");
    }
}
