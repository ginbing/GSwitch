use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use serde_json::{json, Value};

use crate::{
    accounts::{AccountDraft, AppState, OperationGuard},
    app_server::{account_metadata, AccountMetadata, AppServer, TempCodexHome},
    identity,
    types::{
        AccountIdentity, AccountKind, AccountView, ImportResult, OAuthLoginStart, OAuthLoginStatus,
    },
};

const OAUTH_COMPLETION_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_IMPORT_FILE_BYTES: u64 = 10 * 1024 * 1024;

struct ImportCandidate {
    credential: Value,
    label: Option<String>,
}

struct ParsedImport {
    candidates: Vec<ImportCandidate>,
    skipped_count: u32,
}

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

    let cancelled = Arc::new(AtomicBool::new(false));
    if let Err(error) =
        state.start_oauth_login(login_id.clone(), auth_url.clone(), cancelled.clone())
    {
        // The login was started in a GSwitch-owned isolated App Server. If a
        // second sign-in races it, terminate this unused session before the
        // server/profile are dropped rather than leaving it pending upstream.
        let _ = server.account_login_cancel(2, &login_id);
        return Err(error);
    }

    let monitor_id = login_id.clone();
    thread::spawn(move || {
        let status = match monitor_oauth(&mut server, profile, &state, &monitor_id, &cancelled) {
            Ok(Some(account)) => OAuthLoginStatus::Complete { account },
            Ok(None) => OAuthLoginStatus::Cancelled,
            Err(message) => OAuthLoginStatus::Failed { message },
        };
        let _ = state.set_oauth_status(monitor_id, status);
    });

    Ok(OAuthLoginStart { login_id, auth_url })
}

pub fn cancel_oauth(state: &AppState, login_id: &str) -> Result<(), String> {
    state.cancel_oauth(login_id)
}

fn monitor_oauth(
    server: &mut AppServer,
    mut profile: TempCodexHome,
    state: &AppState,
    login_id: &str,
    cancelled: &AtomicBool,
) -> Result<Option<AccountView>, String> {
    let notification = match server.wait_for_notification_cancelled(
        OAUTH_COMPLETION_TIMEOUT,
        |message| {
            message.get("method").and_then(Value::as_str) == Some("account/login/completed")
                && message.pointer("/params/loginId").and_then(Value::as_str) == Some(login_id)
        },
        || cancelled.load(Ordering::SeqCst),
    ) {
        Ok(notification) => notification,
        Err(error) if error == "The operation was cancelled" => {
            let _ = server.account_login_cancel(2, login_id);
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    // A completion notification can race the cancellation flag after the
    // transport delivers it. Cancellation wins: do not persist credentials
    // after the user has closed or cancelled the sign-in flow.
    if cancelled.load(Ordering::SeqCst) {
        let _ = server.account_login_cancel(2, login_id);
        return Ok(None);
    }
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
    persist_validated(
        state,
        &operation,
        &mut profile,
        metadata,
        None,
        None,
        "ChatGPT account".to_string(),
    )
    .map(Some)
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

    import_credential(state, credential, clean_label(label))
}

/// Reads only a user-selected or user-dropped file path. No file bytes cross
/// the Tauri IPC boundary; this parser recognizes the public Cockpit Tools,
/// auth.json, Sub2API, and CPA export layouts without touching Cockpit's
/// private application storage.
pub fn import_file(state: &AppState, path: &str) -> Result<ImportResult, String> {
    let path = Path::new(path);
    let metadata =
        fs::metadata(path).map_err(|_| "Unable to read the selected account file".to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_IMPORT_FILE_BYTES {
        return Err("The selected account file is not supported".to_string());
    }
    let raw = fs::read_to_string(path)
        .map_err(|_| "Unable to read the selected account file".to_string())?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|_| "The selected account file is not valid JSON".to_string())?;
    let parsed = parse_export(value)?;

    let mut imported = Vec::new();
    let mut skipped_count = parsed.skipped_count;
    for candidate in parsed.candidates {
        match import_credential(state, candidate.credential, candidate.label) {
            Ok(account) => imported.push(account),
            Err(_) => skipped_count += 1,
        }
    }
    if imported.is_empty() {
        return Err("No supported Codex accounts could be imported from that file".to_string());
    }
    Ok(ImportResult {
        imported,
        skipped_count,
    })
}

fn import_credential(
    state: &AppState,
    credential: Value,
    label: Option<String>,
) -> Result<AccountView, String> {
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
    let result = server.account_read(1, expected_kind == AccountKind::ChatGpt)?;
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

fn parse_export(value: Value) -> Result<ParsedImport, String> {
    if let Some(candidate) = direct_auth_candidate(&value) {
        return Ok(ParsedImport {
            candidates: vec![candidate],
            skipped_count: 0,
        });
    }

    match value {
        Value::Array(entries) => parse_portable_entries(entries),
        Value::Object(object)
            if object.get("type").and_then(Value::as_str) == Some("sub2api-data") =>
        {
            parse_sub2api_entries(object.get("accounts").cloned())
        }
        value => portable_candidate(&value, None)
            .map(|candidate| ParsedImport {
                candidates: vec![candidate],
                skipped_count: 0,
            })
            .ok_or_else(|| "The selected file is not a supported Codex account export".to_string()),
    }
}

fn parse_portable_entries(entries: Vec<Value>) -> Result<ParsedImport, String> {
    let mut candidates = Vec::new();
    let mut skipped_count: u32 = 0;
    for entry in entries {
        let candidate = direct_auth_candidate(&entry).or_else(|| portable_candidate(&entry, None));
        if let Some(candidate) = candidate {
            candidates.push(candidate);
        } else {
            skipped_count = skipped_count.saturating_add(1);
        }
    }
    if candidates.is_empty() {
        return Err("The selected file has no supported Codex accounts".to_string());
    }
    Ok(ParsedImport {
        candidates,
        skipped_count,
    })
}

fn parse_sub2api_entries(value: Option<Value>) -> Result<ParsedImport, String> {
    let entries = value
        .and_then(|value| value.as_array().cloned())
        .ok_or_else(|| "The selected Sub2API export has no account list".to_string())?;
    let mut candidates = Vec::new();
    let mut skipped_count: u32 = 0;

    for entry in entries {
        let Some(object) = entry.as_object() else {
            skipped_count = skipped_count.saturating_add(1);
            continue;
        };
        if object.get("platform").and_then(Value::as_str) != Some("openai") {
            skipped_count = skipped_count.saturating_add(1);
            continue;
        }
        let label = string_value(object.get("name"));
        let credential = object.get("credentials").cloned();
        let candidate = match object.get("type").and_then(Value::as_str) {
            Some("oauth") => credential
                .as_ref()
                .and_then(|credential| portable_candidate(credential, label.clone())),
            Some("apikey") => credential
                .as_ref()
                .and_then(|credential| sub2api_key_candidate(credential, label.clone())),
            _ => None,
        };
        if let Some(candidate) = candidate {
            candidates.push(candidate);
        } else {
            skipped_count = skipped_count.saturating_add(1);
        }
    }

    if candidates.is_empty() {
        return Err("The selected Sub2API export has no supported Codex accounts".to_string());
    }
    Ok(ParsedImport {
        candidates,
        skipped_count,
    })
}

fn direct_auth_candidate(value: &Value) -> Option<ImportCandidate> {
    if looks_like_cockpit_portable_record(value) || identity::document_kind(value).is_err() {
        return None;
    }
    Some(ImportCandidate {
        credential: value.clone(),
        label: source_label(value),
    })
}

fn looks_like_cockpit_portable_record(value: &Value) -> bool {
    let Some(record) = value.as_object() else {
        return false;
    };
    record.contains_key("id_token")
        || record.contains_key("access_token")
        || record.contains_key("refresh_token")
        || [
            "account_note",
            "two_factor_secret",
            "account_password",
            "phone_number",
            "mail_url",
            "tags",
            "group",
            "account_name",
            "account_structure",
            "api_base_url",
            "api_provider_id",
            "api_provider_name",
        ]
        .iter()
        .any(|key| record.contains_key(*key))
}

fn portable_candidate(value: &Value, label: Option<String>) -> Option<ImportCandidate> {
    let record = value.as_object()?;
    let label = label.or_else(|| source_label(value));

    if record
        .get("auth_mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("agentIdentity"))
        || record.contains_key("agent_identity")
    {
        return None;
    }

    if let Some(api_key) = record
        .get("OPENAI_API_KEY")
        .or_else(|| record.get("api_key"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        return Some(ImportCandidate {
            credential: json!({"auth_mode": "apikey", "OPENAI_API_KEY": api_key}),
            label,
        });
    }

    let id_token = record.get("id_token").and_then(Value::as_str)?.trim();
    let access_token = record.get("access_token").and_then(Value::as_str)?.trim();
    if id_token.is_empty() || access_token.is_empty() {
        return None;
    }

    let mut tokens = serde_json::Map::new();
    tokens.insert("id_token".to_string(), Value::String(id_token.to_string()));
    tokens.insert(
        "access_token".to_string(),
        Value::String(access_token.to_string()),
    );
    if let Some(refresh_token) = record
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        tokens.insert(
            "refresh_token".to_string(),
            Value::String(refresh_token.to_string()),
        );
    }
    if let Some(account_id) = record
        .get("account_id")
        .or_else(|| record.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        tokens.insert(
            "account_id".to_string(),
            Value::String(account_id.to_string()),
        );
    }

    let mut credential = serde_json::Map::new();
    credential.insert("OPENAI_API_KEY".to_string(), Value::Null);
    credential.insert("tokens".to_string(), Value::Object(tokens));
    credential.insert("type".to_string(), Value::String("codex".to_string()));
    if let Some(last_refresh) = record
        .get("last_refresh")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        credential.insert(
            "last_refresh".to_string(),
            Value::String(last_refresh.to_string()),
        );
    }
    Some(ImportCandidate {
        credential: Value::Object(credential),
        label,
    })
}

fn sub2api_key_candidate(value: &Value, label: Option<String>) -> Option<ImportCandidate> {
    let record = value.as_object()?;
    let api_key = record
        .get("api_key")
        .or_else(|| record.get("OPENAI_API_KEY"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())?;
    Some(ImportCandidate {
        credential: json!({"auth_mode": "apikey", "OPENAI_API_KEY": api_key}),
        label: label.or_else(|| source_label(value)),
    })
}

fn source_label(value: &Value) -> Option<String> {
    let record = value.as_object()?;
    ["account_name", "name", "email"]
        .iter()
        .find_map(|key| string_value(record.get(*key)))
}

fn string_value(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
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
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

    fn id_token(user: &str, workspace: &str) -> String {
        let payload = json!({
            "https://api.openai.com/auth": {
                "chatgpt_user_id": user,
                "chatgpt_account_id": workspace
            }
        });
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("payload"))
        )
    }

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

    #[test]
    fn parses_cockpit_export_without_retaining_its_private_metadata() {
        let parsed = parse_export(json!([{
            "id_token": id_token("user", "workspace"),
            "access_token": "access-token",
            "refresh_token": "refresh-token",
            "account_id": "workspace",
            "email": "person@example.com",
            "account_name": "Personal",
            "two_factor_secret": "must-not-migrate",
            "account_password": "must-not-migrate",
            "tags": ["private"]
        }]))
        .expect("cockpit export");

        assert_eq!(parsed.candidates.len(), 1);
        let candidate = &parsed.candidates[0];
        assert_eq!(candidate.label.as_deref(), Some("Personal"));
        assert_eq!(
            candidate.credential.pointer("/tokens/refresh_token"),
            Some(&Value::String("refresh-token".to_string()))
        );
        assert!(candidate.credential.get("two_factor_secret").is_none());
        assert!(candidate.credential.get("account_password").is_none());
        assert!(candidate.credential.get("tags").is_none());
    }

    #[test]
    fn parses_openai_sub2api_accounts_and_skips_other_platforms() {
        let parsed = parse_export(json!({
            "type": "sub2api-data",
            "accounts": [
                {
                    "name": "Work",
                    "platform": "openai",
                    "type": "oauth",
                    "credentials": {
                        "id_token": id_token("user", "workspace"),
                        "access_token": "access-token",
                        "refresh_token": "refresh-token",
                        "chatgpt_account_id": "workspace"
                    }
                },
                {
                    "name": "Not Codex",
                    "platform": "anthropic",
                    "type": "oauth",
                    "credentials": {}
                }
            ]
        }))
        .expect("sub2api export");

        assert_eq!(parsed.candidates.len(), 1);
        assert_eq!(parsed.skipped_count, 1);
        assert_eq!(parsed.candidates[0].label.as_deref(), Some("Work"));
        assert_eq!(
            identity::document_kind(&parsed.candidates[0].credential).expect("kind"),
            AccountKind::ChatGpt
        );
    }

    #[test]
    fn retains_complete_official_auth_documents() {
        let document = json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": id_token("user", "workspace"),
                "access_token": "access-token",
                "refresh_token": "refresh-token"
            },
            "future_field": {"preserve": true}
        });
        let parsed = parse_export(document.clone()).expect("auth document");
        assert_eq!(parsed.candidates[0].credential, document);
    }
}
