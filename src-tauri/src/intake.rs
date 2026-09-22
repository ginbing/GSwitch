use std::{
    collections::HashSet,
    fs,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    accounts::{AccountDraft, AppState, OperationGuard},
    app_server::{account_metadata, AccountMetadata, AppServer, TempCodexHome},
    chatgpt::{self, ChatGptClient, RequestFailure, RequestFailureKind},
    identity, storage,
    types::{
        AccountIdentity, AccountKind, AccountView, ExportResult, ImportResult, OAuthLoginStart,
        OAuthLoginStatus,
    },
};

const OAUTH_COMPLETION_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_IMPORT_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_IMPORT_FILES: usize = 64;
const MAX_IMPORT_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) struct ImportCandidate {
    pub(crate) credential: Value,
    pub(crate) label: Option<String>,
}

pub(crate) struct ParsedImport {
    pub(crate) candidates: Vec<ImportCandidate>,
    pub(crate) unsupported_count: u32,
    pub(crate) duplicate_count: u32,
}

const GSWITCH_EXPORT_FORMAT: &str = "gswitch-accounts";
const GSWITCH_EXPORT_VERSION: u8 = 1;

/// The only portable document GSwitch writes. Operational account state stays
/// in the local library; this contains just the complete credential document
/// and optional presentation metadata needed to import it again.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GSwitchPortableExport {
    format: String,
    version: u8,
    accounts: Vec<GSwitchPortableAccount>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GSwitchPortableAccount {
    credential: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_name: Option<String>,
}

/// Reads the selected saved-account snapshots under GSwitch's operation lock.
/// This function deliberately returns a private document for the Rust command
/// layer rather than anything that may cross Tauri IPC.
pub(crate) fn prepare_accounts_export(
    state: &AppState,
    selected_ids: Vec<String>,
) -> Result<GSwitchPortableExport, String> {
    let mut seen = HashSet::new();
    let selected_ids: Vec<_> = selected_ids
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect();
    if selected_ids.is_empty() {
        return Err("Select at least one saved account to export".to_string());
    }

    let operation = state.acquire_operation()?;
    let accounts = selected_ids
        .iter()
        .map(|id| state.account_by_id_under_operation(&operation, id))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(GSwitchPortableExport {
        format: GSWITCH_EXPORT_FORMAT.to_string(),
        version: GSWITCH_EXPORT_VERSION,
        accounts: accounts
            .into_iter()
            .map(|account| GSwitchPortableAccount {
                credential: account.credential,
                label: clean_label(Some(account.label)),
                workspace_name: account.workspace_name,
            })
            .collect(),
    })
}

/// Writes a previously selected portable export through the shared private
/// atomic writer. Callers must obtain an explicit native save destination.
pub(crate) fn write_accounts_export(
    path: &Path,
    export: &GSwitchPortableExport,
) -> Result<(), String> {
    let content = serde_json::to_vec_pretty(export)
        .map_err(|_| "Unable to serialize the GSwitch account export".to_string())?;
    storage::write_private_bytes_atomic(path, &content, "GSwitch account export")
}

pub(crate) fn accounts_export_count(export: &GSwitchPortableExport) -> u32 {
    export.accounts.len() as u32
}

/// Completes an explicit export after Rust has obtained a native save result.
/// A cancelled dialog has no destination and cannot reach the file writer.
pub(crate) fn complete_accounts_export(
    export: &GSwitchPortableExport,
    destination: Option<&Path>,
) -> Result<ExportResult, String> {
    let Some(destination) = destination else {
        return Ok(ExportResult {
            exported_count: 0,
            cancelled: true,
        });
    };
    write_accounts_export(destination, export)?;
    Ok(ExportResult {
        exported_count: accounts_export_count(export),
        cancelled: false,
    })
}

pub fn start_oauth(state: AppState) -> Result<OAuthLoginStart, String> {
    state.ensure_store_ready()?;
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
    let credential = profile.read_auth()?;
    let expected_identity = identity::derive_identity(&AccountKind::ChatGpt, &credential)?;
    let metadata = read_chatgpt_metadata(&credential, &expected_identity)
        .map_err(SnapshotMetadataFailure::message)?;
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
    import_files(state, vec![path.to_string()])
}

/// Reads and validates one bounded set of user-selected or user-dropped file
/// paths. All readable files are parsed before any credential validation
/// begins; credential validation then runs sequentially under one operation
/// guard so identity deduplication and persistence have one owner.
pub fn import_files(state: &AppState, paths: Vec<String>) -> Result<ImportResult, String> {
    if paths.is_empty() {
        return Err("No account files were selected".to_string());
    }
    if paths.len() > MAX_IMPORT_FILES {
        return Err("Select no more than 64 account files at once".to_string());
    }

    let mut total_bytes = 0u64;
    let mut raw_documents = Vec::new();
    let mut parsed = ParsedImport {
        candidates: Vec::new(),
        unsupported_count: 0,
        duplicate_count: 0,
    };
    let mut failed_count = 0u32;

    for raw_path in paths {
        let path = Path::new(&raw_path);
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => {
                failed_count = failed_count.saturating_add(1);
                continue;
            }
        };
        if !metadata.is_file() {
            parsed.unsupported_count = parsed.unsupported_count.saturating_add(1);
            continue;
        }
        total_bytes = total_bytes.checked_add(metadata.len()).ok_or_else(|| {
            "The selected account files exceed the 64 MiB batch limit".to_string()
        })?;
        if total_bytes > MAX_IMPORT_TOTAL_BYTES {
            return Err("The selected account files exceed the 64 MiB batch limit".to_string());
        }
        if metadata.len() > MAX_IMPORT_FILE_BYTES {
            parsed.unsupported_count = parsed.unsupported_count.saturating_add(1);
            continue;
        }
        match fs::read_to_string(path) {
            Ok(raw) => raw_documents.push(raw),
            Err(_) => failed_count = failed_count.saturating_add(1),
        }
    }

    // Keep every file parse ahead of the first App Server or provider request.
    for raw in raw_documents {
        let value: Value = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(_) => {
                failed_count = failed_count.saturating_add(1);
                continue;
            }
        };
        match parse_export(value) {
            Ok(file) => {
                parsed.candidates.extend(file.candidates);
                parsed.unsupported_count = parsed
                    .unsupported_count
                    .saturating_add(file.unsupported_count);
            }
            Err(_) => {
                parsed.unsupported_count = parsed.unsupported_count.saturating_add(1);
            }
        }
    }

    import_parsed(state, deduplicate_candidates(parsed), failed_count)
}

/// Sends migration candidates through the same bounded, sequential intake
/// path as selected export files. The source adapter supplies only normalized
/// credential documents; provider validation and GSwitch persistence remain
/// owned here.
pub(crate) fn import_migration_credentials(
    state: &AppState,
    candidates: Vec<ImportCandidate>,
) -> Result<ImportResult, String> {
    let parsed = ParsedImport {
        candidates,
        unsupported_count: 0,
        duplicate_count: 0,
    };
    import_parsed(state, deduplicate_candidates(parsed), 0)
}

fn import_parsed(
    state: &AppState,
    parsed: ParsedImport,
    mut failed_count: u32,
) -> Result<ImportResult, String> {
    if parsed.candidates.is_empty() {
        state.ensure_store_ready()?;
        return Ok(ImportResult {
            imported: Vec::new(),
            duplicate_count: parsed.duplicate_count,
            unsupported_count: parsed.unsupported_count,
            failed_count,
        });
    }

    let operation = state.acquire_operation()?;
    let mut imported = Vec::new();
    let mut duplicate_count = parsed.duplicate_count;
    for candidate in parsed.candidates {
        let expected_kind = match identity::document_kind(&candidate.credential) {
            Ok(kind) => kind,
            Err(_) => {
                failed_count = failed_count.saturating_add(1);
                continue;
            }
        };
        let expected_identity =
            match identity::derive_identity(&expected_kind, &candidate.credential) {
                Ok(identity) => identity,
                Err(_) => {
                    failed_count = failed_count.saturating_add(1);
                    continue;
                }
            };
        if state
            .find_import_match_under_operation(
                &operation,
                &expected_kind,
                &expected_identity,
                &candidate.credential,
            )?
            .is_some()
        {
            duplicate_count = duplicate_count.saturating_add(1);
            continue;
        }
        match import_credential_under_operation(
            state,
            &operation,
            candidate.credential,
            candidate.label,
        ) {
            Ok(account) => imported.push(account),
            Err(_) => failed_count = failed_count.saturating_add(1),
        }
    }

    Ok(ImportResult {
        imported,
        duplicate_count,
        unsupported_count: parsed.unsupported_count,
        failed_count,
    })
}

fn import_credential(
    state: &AppState,
    credential: Value,
    label: Option<String>,
) -> Result<AccountView, String> {
    let operation = state.acquire_operation()?;
    import_credential_under_operation(state, &operation, credential, label)
}

fn import_credential_under_operation(
    state: &AppState,
    operation: &OperationGuard<'_>,
    credential: Value,
    label: Option<String>,
) -> Result<AccountView, String> {
    let expected_kind = identity::document_kind(&credential)?;
    let expected_identity = identity::derive_identity(&expected_kind, &credential)?;
    if let Some(existing) = state.find_exact_document_under_operation(
        operation,
        &expected_kind,
        &expected_identity,
        &credential,
    )? {
        return Ok(existing);
    }

    if expected_kind == AccountKind::ChatGpt {
        return import_chatgpt_snapshot(
            state,
            operation,
            credential,
            expected_identity,
            clean_label(label),
        );
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
        operation,
        &mut profile,
        metadata,
        Some(expected_identity),
        clean_label(label),
        "Imported account".to_string(),
    )
}

fn import_chatgpt_snapshot(
    state: &AppState,
    operation: &OperationGuard<'_>,
    credential: Value,
    expected_identity: AccountIdentity,
    label: Option<String>,
) -> Result<AccountView, String> {
    let client = ChatGptClient::new()?;
    import_chatgpt_snapshot_with_client(
        state,
        operation,
        credential,
        expected_identity,
        label,
        &client,
    )
}

fn import_chatgpt_snapshot_with_client(
    state: &AppState,
    operation: &OperationGuard<'_>,
    credential: Value,
    expected_identity: AccountIdentity,
    label: Option<String>,
    client: &ChatGptClient,
) -> Result<AccountView, String> {
    let mut profile = TempCodexHome::create(&state.isolated_profile_root()?)?;
    profile.write_auth(&credential)?;
    let live = crate::quota::live_credential_for_identity(&expected_identity)?;
    let snapshot = live.clone().unwrap_or_else(|| credential.clone());
    if live.is_some() {
        // The active account's live document is the newest complete snapshot;
        // keep that document in the isolated validation profile so a
        // successful same-identity retry cannot save an older token chain.
        profile.write_auth(&snapshot)?;
    }

    match read_chatgpt_metadata_with_client(client, &snapshot, &expected_identity) {
        Ok(metadata) => persist_validated(
            state,
            operation,
            &mut profile,
            metadata,
            Some(expected_identity),
            label,
            "Imported account".to_string(),
        ),
        Err(SnapshotMetadataFailure::Provider(error))
            if error.can_fallback_to_managed_refresh() && live.is_some() =>
        {
            let latest = crate::quota::live_credential_for_identity(&expected_identity)?
                .ok_or_else(|| {
                    "Codex is running and GSwitch could not reread its active account credential"
                        .to_string()
                })?;
            if latest == snapshot {
                return Err(SnapshotMetadataFailure::Provider(error).message());
            }
            profile.write_auth(&latest)?;
            match read_chatgpt_metadata_with_client(client, &latest, &expected_identity) {
                Ok(metadata) => persist_validated(
                    state,
                    operation,
                    &mut profile,
                    metadata,
                    Some(expected_identity),
                    label,
                    "Imported account".to_string(),
                ),
                Err(error) => Err(error.message()),
            }
        }
        Err(SnapshotMetadataFailure::Provider(error))
            if error.can_fallback_to_managed_refresh() =>
        {
            let mut server = AppServer::start(&profile.path)?;
            let result = server.account_read(1, true)?;
            let metadata = account_metadata(&result)?;
            if metadata.kind != AccountKind::ChatGpt {
                return Err(
                    "The imported credential changed account type during validation".to_string(),
                );
            }
            persist_validated(
                state,
                operation,
                &mut profile,
                metadata,
                Some(expected_identity),
                label,
                "Imported account".to_string(),
            )
        }
        Err(error) => Err(error.message()),
    }
}

fn read_chatgpt_metadata(
    credential: &Value,
    expected_identity: &AccountIdentity,
) -> Result<AccountMetadata, SnapshotMetadataFailure> {
    let client = ChatGptClient::new().map_err(SnapshotMetadataFailure::Message)?;
    read_chatgpt_metadata_with_client(&client, credential, expected_identity)
}

fn read_chatgpt_metadata_with_client(
    client: &ChatGptClient,
    credential: &Value,
    expected_identity: &AccountIdentity,
) -> Result<AccountMetadata, SnapshotMetadataFailure> {
    let response = client
        .account_check(credential)
        .map_err(SnapshotMetadataFailure::Provider)?;
    let projection = chatgpt::normalize_account_metadata(credential, expected_identity, &response)
        .map_err(SnapshotMetadataFailure::Message)?;
    Ok(AccountMetadata {
        kind: AccountKind::ChatGpt,
        email: projection.email,
        plan_type: projection.plan_type,
        workspace_name: projection.workspace_name,
        account_structure: projection.account_structure,
    })
}

enum SnapshotMetadataFailure {
    Provider(RequestFailure),
    Message(String),
}

impl SnapshotMetadataFailure {
    fn message(self) -> String {
        match self {
            Self::Message(message) => message,
            Self::Provider(error) => match error.kind {
                RequestFailureKind::Authentication => {
                    "ChatGPT rejected the read-only account check".to_string()
                }
                RequestFailureKind::RateLimited => {
                    "ChatGPT rate-limited the account check; try again later".to_string()
                }
                RequestFailureKind::Http => "ChatGPT account service returned an error".to_string(),
                RequestFailureKind::Transport => {
                    "Unable to reach ChatGPT account service".to_string()
                }
                RequestFailureKind::InvalidJson => {
                    "ChatGPT returned an invalid account response".to_string()
                }
            },
        }
    }
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

pub(crate) fn parse_export(value: Value) -> Result<ParsedImport, String> {
    if value
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| format == GSWITCH_EXPORT_FORMAT)
    {
        return parse_gswitch_export(value);
    }
    if let Some(candidate) = current_cockpit_candidate(&value) {
        return Ok(ParsedImport {
            candidates: vec![candidate],
            unsupported_count: 0,
            duplicate_count: 0,
        });
    }
    if let Some(candidate) = direct_auth_candidate(&value) {
        return Ok(ParsedImport {
            candidates: vec![candidate],
            unsupported_count: 0,
            duplicate_count: 0,
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
                unsupported_count: 0,
                duplicate_count: 0,
            })
            .ok_or_else(|| "The selected file is not a supported Codex account export".to_string()),
    }
}

fn parse_gswitch_export(value: Value) -> Result<ParsedImport, String> {
    let export: GSwitchPortableExport = serde_json::from_value(value)
        .map_err(|_| "The selected GSwitch account export is invalid".to_string())?;
    if export.format != GSWITCH_EXPORT_FORMAT || export.version != GSWITCH_EXPORT_VERSION {
        return Err("Unsupported GSwitch account export version".to_string());
    }
    if export.accounts.is_empty() {
        return Err("The selected GSwitch account export has no accounts".to_string());
    }

    let mut candidates = Vec::new();
    let mut unsupported_count = 0u32;
    for account in export.accounts {
        let candidate = direct_auth_candidate(&account.credential)
            .or_else(|| portable_candidate(&account.credential, account.label.clone()));
        if let Some(mut candidate) = candidate {
            candidate.label = clean_label(account.label).or(candidate.label);
            candidates.push(candidate);
        } else {
            unsupported_count = unsupported_count.saturating_add(1);
        }
    }
    if candidates.is_empty() {
        return Err(
            "The selected GSwitch account export has no supported Codex accounts".to_string(),
        );
    }
    Ok(ParsedImport {
        candidates,
        unsupported_count,
        duplicate_count: 0,
    })
}

fn parse_portable_entries(entries: Vec<Value>) -> Result<ParsedImport, String> {
    let mut candidates = Vec::new();
    let mut unsupported_count: u32 = 0;
    for entry in entries {
        let candidate = current_cockpit_candidate(&entry)
            .or_else(|| direct_auth_candidate(&entry))
            .or_else(|| portable_candidate(&entry, None));
        if let Some(candidate) = candidate {
            candidates.push(candidate);
        } else {
            unsupported_count = unsupported_count.saturating_add(1);
        }
    }
    if candidates.is_empty() {
        return Err("The selected file has no supported Codex accounts".to_string());
    }
    Ok(ParsedImport {
        candidates,
        unsupported_count,
        duplicate_count: 0,
    })
}

fn parse_sub2api_entries(value: Option<Value>) -> Result<ParsedImport, String> {
    let entries = value
        .and_then(|value| value.as_array().cloned())
        .ok_or_else(|| "The selected Sub2API export has no account list".to_string())?;
    let mut candidates = Vec::new();
    let mut unsupported_count: u32 = 0;

    for entry in entries {
        let Some(object) = entry.as_object() else {
            unsupported_count = unsupported_count.saturating_add(1);
            continue;
        };
        if object.get("platform").and_then(Value::as_str) != Some("openai") {
            unsupported_count = unsupported_count.saturating_add(1);
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
            unsupported_count = unsupported_count.saturating_add(1);
        }
    }

    if candidates.is_empty() {
        return Err("The selected Sub2API export has no supported Codex accounts".to_string());
    }
    Ok(ParsedImport {
        candidates,
        unsupported_count,
        duplicate_count: 0,
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

/// Current Cockpit Tools exports a public `CodexAccount` record with tokens
/// nested under `tokens`. Keep this path explicit so a complete official
/// auth.json document with a similar token shape remains untouched.
fn current_cockpit_candidate(value: &Value) -> Option<ImportCandidate> {
    let record = value.as_object()?;
    let token_record = record.get("tokens")?.as_object()?;
    let has_cockpit_marker = ["account_name", "account_id", "chatgpt_account_id", "email"]
        .iter()
        .any(|key| record.contains_key(*key));
    if !has_cockpit_marker {
        return None;
    }

    let id_token = token_record
        .get("id_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())?;
    let access_token = token_record
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())?;

    let mut tokens = serde_json::Map::new();
    tokens.insert("id_token".to_string(), Value::String(id_token.to_string()));
    tokens.insert(
        "access_token".to_string(),
        Value::String(access_token.to_string()),
    );
    if let Some(refresh_token) = token_record
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

    Some(ImportCandidate {
        credential: json!({
            "OPENAI_API_KEY": null,
            "tokens": tokens,
            "type": "codex"
        }),
        label: source_label(value),
    })
}

fn looks_like_cockpit_portable_record(value: &Value) -> bool {
    let Some(record) = value.as_object() else {
        return false;
    };
    (record.contains_key("tokens")
        && ["account_name", "account_id", "chatgpt_account_id", "email"]
            .iter()
            .any(|key| record.contains_key(*key)))
        || record.contains_key("id_token")
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
    Some(ImportCandidate {
        credential: Value::Object(credential),
        label,
    })
}

fn deduplicate_candidates(parsed: ParsedImport) -> ParsedImport {
    let mut seen = Vec::<AccountIdentity>::new();
    let mut candidates = Vec::with_capacity(parsed.candidates.len());
    let mut duplicate_count = parsed.duplicate_count;

    for candidate in parsed.candidates {
        let identity = identity::document_kind(&candidate.credential)
            .ok()
            .and_then(|kind| identity::derive_identity(&kind, &candidate.credential).ok());
        if let Some(identity) = identity {
            if seen.contains(&identity) {
                duplicate_count = duplicate_count.saturating_add(1);
                continue;
            }
            seen.push(identity);
        }
        candidates.push(candidate);
    }

    ParsedImport {
        candidates,
        unsupported_count: parsed.unsupported_count,
        duplicate_count,
    }
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

    let default_label = metadata
        .email
        .clone()
        .or_else(|| metadata.workspace_name.clone())
        .unwrap_or(fallback_label);
    let result = state.upsert_under_operation(
        operation,
        AccountDraft {
            label,
            default_label,
            kind: metadata.kind,
            email: metadata.email,
            plan_type: metadata.plan_type,
            workspace_name: metadata.workspace_name,
            account_structure: metadata.account_structure,
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
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
    };

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

    fn account_check_fixture() -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let mut request = [0; 4096];
            let size = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..size]).to_string();
            let body = r#"{"accounts":[{"id":"workspace","name":"Personal","structure":"workspace","plan_type":"plus"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("response");
            request
        });
        (format!("http://{address}"), handle)
    }

    fn test_path(prefix: &str, suffix: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("gswitch-{prefix}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("test directory");
        root.join(format!("input.{suffix}"))
    }

    fn official_credential(user: &str, workspace: &str, access: &str) -> Value {
        json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": id_token(user, workspace),
                "access_token": access,
                "refresh_token": "refresh-token"
            },
            "future_field": {"must_not_cross": true}
        })
    }

    #[test]
    fn rejects_malformed_json_before_starting_codex() {
        let path = test_path("invalid-json", "json");
        let state = AppState::new(path.clone()).expect("state");

        let error = import_json(&state, "{not valid", None).expect_err("invalid JSON");
        assert_eq!(error, "Invalid auth JSON");
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn rejects_non_object_json_before_starting_codex() {
        let path = test_path("non-object", "json");
        let state = AppState::new(path.clone()).expect("state");

        let error = import_json(&state, "[]", None).expect_err("invalid document");
        assert_eq!(error, "Invalid auth JSON: expected a JSON object");
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
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
    fn parses_current_cockpit_bulk_export_once_and_drops_unrelated_fields() {
        let parsed = parse_export(json!([
            {
                "email": "one@example.com",
                "account_id": "workspace-one",
                "account_name": "One",
                "tokens": {
                    "id_token": id_token("user-one", "workspace-one"),
                    "access_token": "access-one",
                    "refresh_token": "refresh-one"
                },
                "quota": {"five_hour": 99},
                "quota_error": "private error",
                "tags": ["private"],
                "notes": "private note",
                "password": "private password",
                "two_factor_secret": "private 2fa",
                "phone_number": "private phone",
                "mail_url": "private mail",
                "api_provider_name": "private provider",
                "created_at": "private timestamp",
                "history": ["private history"],
                "routing": {"private": true},
                "settings": {"private": true},
                "active": true
            },
            {
                "email": "two@example.com",
                "account_id": "workspace-two",
                "account_name": "Two",
                "tokens": {
                    "id_token": id_token("user-two", "workspace-two"),
                    "access_token": "access-two"
                }
            },
            {
                "email": "duplicate@example.com",
                "account_id": "workspace-one",
                "account_name": "Duplicate",
                "tokens": {
                    "id_token": id_token("user-one", "workspace-one"),
                    "access_token": "access-new"
                }
            },
            {
                "email": "unsupported@example.com",
                "account_id": "workspace-unsupported",
                "account_name": "Unsupported",
                "tokens": {"id_token": "missing-access-token"}
            }
        ]))
        .expect("current cockpit export");

        assert_eq!(parsed.candidates.len(), 3);
        assert_eq!(parsed.unsupported_count, 1);

        let parsed = deduplicate_candidates(parsed);
        assert_eq!(parsed.candidates.len(), 2);
        assert_eq!(parsed.unsupported_count, 1);
        assert_eq!(parsed.duplicate_count, 1);
        assert_eq!(parsed.candidates[0].label.as_deref(), Some("One"));
        assert_eq!(parsed.candidates[1].label.as_deref(), Some("Two"));

        let credential = &parsed.candidates[0].credential;
        let object = credential.as_object().expect("canonical credential");
        assert_eq!(object.len(), 3);
        assert!(object.contains_key("OPENAI_API_KEY"));
        assert!(object.contains_key("tokens"));
        assert!(object.contains_key("type"));
        for field in [
            "quota",
            "quota_error",
            "tags",
            "notes",
            "password",
            "two_factor_secret",
            "phone_number",
            "mail_url",
            "api_provider_name",
            "created_at",
            "history",
            "routing",
            "settings",
            "active",
            "email",
            "account_name",
        ] {
            assert!(object.get(field).is_none(), "unexpected field: {field}");
        }
        let tokens = object
            .get("tokens")
            .and_then(Value::as_object)
            .expect("canonical tokens");
        assert_eq!(tokens.len(), 4);
        assert_eq!(
            tokens.get("account_id").and_then(Value::as_str),
            Some("workspace-one")
        );
        assert!(tokens.get("id_token").is_some());
        assert_eq!(
            tokens.get("access_token").and_then(Value::as_str),
            Some("access-one")
        );
        assert_eq!(
            tokens.get("refresh_token").and_then(Value::as_str),
            Some("refresh-one")
        );
    }

    #[test]
    fn parses_a_single_current_cockpit_record_without_importing_its_metadata() {
        let parsed = parse_export(json!({
            "email": "person@example.com",
            "account_id": "workspace",
            "account_name": "Personal",
            "tokens": {
                "id_token": id_token("user", "workspace"),
                "access_token": "access-token",
                "refresh_token": "refresh-token"
            },
            "notes": "must not migrate"
        }))
        .expect("single current cockpit record");

        assert_eq!(parsed.candidates.len(), 1);
        assert_eq!(parsed.candidates[0].label.as_deref(), Some("Personal"));
        assert!(parsed.candidates[0].credential.get("notes").is_none());
        assert!(parsed.candidates[0].credential.get("email").is_none());
    }

    #[test]
    fn deduplicates_candidates_across_selected_files_by_identity() {
        let first = parse_export(json!({
            "id_token": id_token("user", "workspace"),
            "access_token": "access-one"
        }))
        .expect("first export");
        let second = parse_export(json!({
            "id_token": id_token("user", "workspace"),
            "access_token": "access-two"
        }))
        .expect("second export");
        let parsed = deduplicate_candidates(ParsedImport {
            candidates: first
                .candidates
                .into_iter()
                .chain(second.candidates)
                .collect(),
            unsupported_count: first.unsupported_count + second.unsupported_count,
            duplicate_count: 0,
        });

        assert_eq!(parsed.candidates.len(), 1);
        assert_eq!(parsed.duplicate_count, 1);
        assert_eq!(parsed.unsupported_count, 0);
    }

    #[test]
    fn writes_a_versioned_portable_export_without_operational_account_state() {
        let store_path = test_path("portable-export-store", "json");
        let export_path = test_path("portable-export-output", "json");
        let credential = official_credential("user", "workspace", "access-token");
        let state = AppState::new(store_path.clone()).expect("state");
        let identity =
            identity::derive_identity(&AccountKind::ChatGpt, &credential).expect("identity");
        let operation = state.acquire_operation().expect("operation");
        let saved = state
            .upsert_under_operation(
                &operation,
                AccountDraft {
                    label: Some("Portable label".to_string()),
                    default_label: "Default label".to_string(),
                    kind: AccountKind::ChatGpt,
                    email: Some("person@example.com".to_string()),
                    plan_type: Some("plus".to_string()),
                    workspace_name: Some("Personal".to_string()),
                    account_structure: Some("workspace".to_string()),
                    identity,
                    credential: credential.clone(),
                },
            )
            .expect("save account");
        drop(operation);

        let export = prepare_accounts_export(&state, vec![saved.id]).expect("prepare export");
        assert_eq!(accounts_export_count(&export), 1);
        let result = complete_accounts_export(&export, Some(&export_path)).expect("write export");
        assert_eq!(result.exported_count, 1);
        assert!(!result.cancelled);
        let value: Value =
            serde_json::from_str(&fs::read_to_string(&export_path).expect("read export"))
                .expect("export JSON");

        assert_eq!(
            value.get("format").and_then(Value::as_str),
            Some("gswitch-accounts")
        );
        assert_eq!(value.get("version").and_then(Value::as_u64), Some(1));
        assert_eq!(
            value.pointer("/accounts/0/credential"),
            Some(&credential),
            "the complete credential remains available for a later import"
        );
        assert_eq!(
            value.pointer("/accounts/0/label").and_then(Value::as_str),
            Some("Portable label")
        );
        assert_eq!(
            value
                .pointer("/accounts/0/workspace_name")
                .and_then(Value::as_str),
            Some("Personal")
        );
        let round_trip = parse_export(value.clone()).expect("portable round trip");
        assert_eq!(round_trip.candidates.len(), 1);
        assert_eq!(round_trip.candidates[0].credential, credential);
        for omitted in [
            "/accounts/0/id",
            "/accounts/0/kind",
            "/accounts/0/email",
            "/accounts/0/plan_type",
            "/accounts/0/identity",
            "/accounts/0/quota",
            "/accounts/0/reset_credits",
            "/accounts/0/pending_reset_credit",
        ] {
            assert!(
                value.pointer(omitted).is_none(),
                "unexpected export field: {omitted}"
            );
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&export_path)
                    .expect("export metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let _ = fs::remove_file(&export_path);
        let _ = fs::remove_dir_all(export_path.parent().expect("parent"));
        let _ = fs::remove_dir_all(store_path.parent().expect("parent"));
    }

    #[test]
    fn export_rejects_an_empty_or_stale_selection_before_a_file_can_be_written() {
        let store_path = test_path("portable-export-selection", "json");
        let output_path = test_path("portable-export-selection-output", "json");
        let state = AppState::new(store_path.clone()).expect("state");

        let empty = prepare_accounts_export(&state, Vec::new()).expect_err("empty selection");
        assert_eq!(empty, "Select at least one saved account to export");
        let stale = prepare_accounts_export(&state, vec!["missing".to_string()])
            .expect_err("stale selection");
        assert_eq!(stale, "The selected account is no longer saved");
        assert!(!output_path.exists());

        let _ = fs::remove_dir_all(output_path.parent().expect("parent"));
        let _ = fs::remove_dir_all(store_path.parent().expect("parent"));
    }

    #[test]
    fn cancelled_export_does_not_write_a_file_or_expose_export_contents() {
        let store_path = test_path("portable-export-cancel", "json");
        let output_path = test_path("portable-export-cancel-output", "json");
        let credential = official_credential("user", "workspace", "access-token");
        let state = AppState::new(store_path.clone()).expect("state");
        let identity =
            identity::derive_identity(&AccountKind::ChatGpt, &credential).expect("identity");
        let operation = state.acquire_operation().expect("operation");
        let saved = state
            .upsert_under_operation(
                &operation,
                AccountDraft {
                    label: None,
                    default_label: "Export candidate".to_string(),
                    kind: AccountKind::ChatGpt,
                    email: None,
                    plan_type: None,
                    workspace_name: None,
                    account_structure: None,
                    identity,
                    credential,
                },
            )
            .expect("save account");
        drop(operation);

        let export = prepare_accounts_export(&state, vec![saved.id]).expect("prepare export");
        let result = complete_accounts_export(&export, None).expect("cancelled export");
        assert_eq!(result.exported_count, 0);
        assert!(result.cancelled);
        assert!(!output_path.exists());
        let serialized = serde_json::to_string(&result).expect("safe result JSON");
        assert!(!serialized.contains("access-token"));

        let _ = fs::remove_dir_all(output_path.parent().expect("parent"));
        let _ = fs::remove_dir_all(store_path.parent().expect("parent"));
    }

    #[test]
    fn parses_a_valid_portable_export_and_rejects_unknown_versions() {
        let document = json!({
            "format": "gswitch-accounts",
            "version": 1,
            "accounts": [{
                "credential": official_credential("user", "workspace", "access-token"),
                "label": "Imported label",
                "workspace_name": "Personal"
            }]
        });
        let parsed = parse_export(document).expect("portable export");
        assert_eq!(parsed.candidates.len(), 1);
        assert_eq!(
            parsed.candidates[0].label.as_deref(),
            Some("Imported label")
        );
        assert_eq!(
            parsed.candidates[0]
                .credential
                .pointer("/future_field/must_not_cross"),
            Some(&Value::Bool(true))
        );

        let error = match parse_export(json!({
            "format": "gswitch-accounts",
            "version": 2,
            "accounts": []
        })) {
            Err(error) => error,
            Ok(_) => panic!("unknown version must be rejected"),
        };
        assert_eq!(error, "Unsupported GSwitch account export version");
    }

    #[test]
    fn batch_zero_import_result_distinguishes_unsupported_and_failed_files() {
        let store_path = test_path("batch-empty", "json");
        let unsupported_path = test_path("batch-unsupported", "json");
        let invalid_path = test_path("batch-invalid", "json");
        fs::write(&unsupported_path, "{}").expect("unsupported file");
        fs::write(&invalid_path, "{not valid json").expect("invalid file");
        let state = AppState::new(store_path.clone()).expect("state");

        let result = import_files(
            &state,
            vec![
                unsupported_path.to_string_lossy().into_owned(),
                invalid_path.to_string_lossy().into_owned(),
            ],
        )
        .expect("aggregate result");

        assert!(result.imported.is_empty());
        assert_eq!(result.duplicate_count, 0);
        assert_eq!(result.unsupported_count, 1);
        assert_eq!(result.failed_count, 1);
        let serialized = serde_json::to_string(&result).expect("result JSON");
        assert!(!serialized.contains("access-token"));
        assert!(!serialized.contains("future_field"));
        let _ = fs::remove_file(&unsupported_path);
        let _ = fs::remove_file(&invalid_path);
        let _ = fs::remove_dir_all(unsupported_path.parent().expect("parent"));
        let _ = fs::remove_dir_all(invalid_path.parent().expect("parent"));
        let _ = fs::remove_dir_all(store_path.parent().expect("parent"));
    }

    #[test]
    fn existing_saved_identity_counts_as_duplicate_without_validation() {
        let store_path = test_path("batch-duplicate-store", "json");
        let import_path = test_path("batch-duplicate-import", "json");
        let credential = official_credential("user", "workspace", "access-token");
        fs::write(
            &import_path,
            serde_json::to_string(&json!({
                "format": "gswitch-accounts",
                "version": 1,
                "accounts": [{
                    "credential": credential,
                    "label": "Imported duplicate"
                }]
            }))
            .expect("portable export JSON"),
        )
        .expect("import file");
        let state = AppState::new(store_path.clone()).expect("state");
        let identity =
            identity::derive_identity(&AccountKind::ChatGpt, &credential).expect("identity");
        let operation = state.acquire_operation().expect("operation");
        state
            .upsert_under_operation(
                &operation,
                AccountDraft {
                    label: None,
                    default_label: "Existing".to_string(),
                    kind: AccountKind::ChatGpt,
                    email: Some("person@example.com".to_string()),
                    plan_type: Some("plus".to_string()),
                    workspace_name: Some("Personal".to_string()),
                    account_structure: Some("workspace".to_string()),
                    identity,
                    credential: credential.clone(),
                },
            )
            .expect("save existing account");
        drop(operation);

        let result = import_files(&state, vec![import_path.to_string_lossy().into_owned()])
            .expect("duplicate result");

        assert!(result.imported.is_empty());
        assert_eq!(result.duplicate_count, 1);
        assert_eq!(result.unsupported_count, 0);
        assert_eq!(result.failed_count, 0);
        let _ = fs::remove_file(&import_path);
        let _ = fs::remove_dir_all(import_path.parent().expect("parent"));
        let _ = fs::remove_dir_all(store_path.parent().expect("parent"));
    }

    #[test]
    fn batch_limits_file_count_and_total_bytes_before_validation() {
        let store_path = test_path("batch-limits", "json");
        let state = AppState::new(store_path.clone()).expect("state");
        let too_many = (0..=MAX_IMPORT_FILES)
            .map(|index| format!("missing-{index}.json"))
            .collect();
        let error = import_files(&state, too_many).expect_err("file-count limit");
        assert!(error.contains("64 account files"));

        let oversized_path = test_path("batch-oversized", "json");
        let file = fs::File::create(&oversized_path).expect("oversized file");
        file.set_len(MAX_IMPORT_TOTAL_BYTES + 1)
            .expect("set aggregate size");
        drop(file);
        let error = import_files(&state, vec![oversized_path.to_string_lossy().into_owned()])
            .expect_err("aggregate-size limit");
        assert!(error.contains("64 MiB"));
        let _ = fs::remove_file(&oversized_path);
        let _ = fs::remove_dir_all(oversized_path.parent().expect("parent"));
        let _ = fs::remove_dir_all(store_path.parent().expect("parent"));
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
        assert_eq!(parsed.unsupported_count, 1);
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

    #[test]
    fn valid_chatgpt_import_uses_the_snapshot_and_preserves_unknown_fields() {
        let path = test_path("snapshot-intake", "json");
        let state = AppState::new(path.clone()).expect("state");
        let credential = json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": id_token("user", "workspace"),
                "access_token": "access-token",
                "refresh_token": "refresh-token"
            },
            "auth_mode": "chatgpt",
            "future_field": {"must_survive": true}
        });
        let expected_identity =
            identity::derive_identity(&AccountKind::ChatGpt, &credential).expect("identity");
        let (base_url, server) = account_check_fixture();
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        let operation = state.acquire_operation().expect("operation");
        let account = import_chatgpt_snapshot_with_client(
            &state,
            &operation,
            credential.clone(),
            expected_identity,
            None,
            &client,
        )
        .expect("import");
        drop(operation);

        let saved = state.account_by_id(&account.id).expect("saved account");
        assert_eq!(saved.credential, credential);
        assert_eq!(saved.workspace_name.as_deref(), Some("Personal"));
        assert_eq!(saved.account_structure.as_deref(), Some("workspace"));
        assert_eq!(saved.plan_type.as_deref(), Some("plus"));
        assert_eq!(saved.email, None);
        assert!(server
            .join()
            .expect("server")
            .starts_with("GET /wham/accounts/check HTTP/1.1"));
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }
}
