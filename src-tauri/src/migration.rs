use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
};

use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    accounts::AppState,
    codex, identity,
    intake::{self, ImportCandidate},
    types::{
        AccountIdentity, ImportResult, MigrationCandidateState, MigrationCandidateView,
        MigrationPreview, MigrationSource,
    },
};

const COCKPIT_ROOT_NAME: &str = ".antigravity_cockpit";
const LEGACY_COCKPIT_ROOT_NAME: &str = "com.antigravity.cockpit-tools";
const COCKPIT_INDEX_NAME: &str = "codex_accounts.json";
const COCKPIT_DETAILS_DIR_NAME: &str = "codex_accounts";
const COCKPIT_KEY_NAME: &str = "secure-account-storage.key";
const SECURE_VERSION: u32 = 1;
const SECURE_KIND: &str = "codex";
const SECURE_ALGORITHM: &str = "AES-256-GCM";
const SECURE_KEY_ID: &str = "local-secure-account-storage-v1";
const MAX_DETAIL_FILES: usize = 64;
const MAX_DETAIL_BYTES: u64 = 10 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
struct SourceCandidate {
    view: MigrationCandidateView,
    identity: Option<AccountIdentity>,
    credential: Option<Value>,
}

#[derive(Debug, Clone, Default)]
struct IndexSummary {
    email: Option<String>,
    plan_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SecureEnvelope {
    version: u32,
    kind: String,
    algorithm: String,
    key_id: String,
    nonce: String,
    ciphertext: String,
    #[allow(dead_code)]
    encrypted_at: i64,
}

/// Runs the one-shot, user-triggered migration scan. The default paths are
/// fixed to the official Codex profile and Cockpit's documented production
/// roots; an optional folder is added only when the user explicitly chooses
/// it. Environment overrides are intentionally ignored here.
pub fn discover(state: &AppState, custom_root: Option<String>) -> Result<MigrationPreview, String> {
    let official_home = codex::codex_home()?;
    let roots = resolve_cockpit_roots(custom_root)?;
    let candidates = discover_candidates(state, &official_home, &roots)?;
    Ok(MigrationPreview {
        candidates: candidates
            .into_iter()
            .map(|candidate| candidate.view)
            .collect(),
    })
}

/// Re-reads the selected source records and sends only their normalized
/// credentials through the normal sequential intake path. Candidate IDs are
/// derived from the source record and identity, so a source identity change
/// between preview and confirmation fails closed.
pub fn confirm(
    state: &AppState,
    custom_root: Option<String>,
    selected_ids: Vec<String>,
) -> Result<ImportResult, String> {
    if selected_ids.is_empty() {
        return Err("Select at least one new account to import".to_string());
    }

    let official_home = codex::codex_home()?;
    let roots = resolve_cockpit_roots(custom_root)?;
    let candidates = discover_candidates(state, &official_home, &roots)?;
    confirm_candidates(state, &candidates, selected_ids)
}

fn confirm_candidates(
    state: &AppState,
    candidates: &[SourceCandidate],
    selected_ids: Vec<String>,
) -> Result<ImportResult, String> {
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for candidate_id in selected_ids {
        if seen.insert(candidate_id.clone()) {
            selected.push(candidate_id);
        }
    }
    if selected.is_empty() {
        return Err("Select at least one new account to import".to_string());
    }

    let mut imports = Vec::new();
    for candidate_id in &selected {
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.view.id == *candidate_id)
            .ok_or_else(|| "The migration preview is stale; scan again".to_string())?;
        if candidate.view.state != MigrationCandidateState::New {
            return Err("The migration preview is no longer importable; scan again".to_string());
        }
        let credential = candidate.credential.clone().ok_or_else(|| {
            "The migration preview is no longer importable; scan again".to_string()
        })?;
        imports.push(ImportCandidate {
            credential,
            label: candidate.view.workspace_name.clone(),
        });
    }

    intake::import_migration_credentials(state, imports)
}

fn discover_candidates(
    state: &AppState,
    official_home: &Path,
    cockpit_roots: &[(PathBuf, bool)],
) -> Result<Vec<SourceCandidate>, String> {
    let mut candidates = Vec::new();
    candidates.extend(discover_official(state, official_home)?);
    for (root, _explicit) in cockpit_roots {
        candidates.extend(discover_cockpit_root(state, root)?);
    }

    // Prefer the official profile when both sources contain the same stable
    // identity. This prevents a preview with two copies from becoming an
    // accidental import choice.
    let mut seen = Vec::<AccountIdentity>::new();
    candidates.retain(|candidate| {
        let Some(identity) = candidate.identity.as_ref() else {
            return true;
        };
        if seen.contains(identity) {
            return false;
        }
        seen.push(identity.clone());
        true
    });
    Ok(candidates)
}

fn discover_official(
    state: &AppState,
    official_home: &Path,
) -> Result<Vec<SourceCandidate>, String> {
    let path = codex::auth_path(official_home);
    if fs::metadata(&path)
        .ok()
        .is_some_and(|metadata| metadata.len() > MAX_DETAIL_BYTES)
    {
        return Ok(vec![unsupported_candidate(
            MigrationSource::OfficialCodex,
            "official-auth",
            None,
            None,
        )]);
    }
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => {
            return Ok(vec![unsupported_candidate(
                MigrationSource::OfficialCodex,
                "official-auth",
                None,
                None,
            )]);
        }
    };
    let value = match serde_json::from_str::<Value>(&content) {
        Ok(value) => value,
        Err(_) => {
            return Ok(vec![unsupported_candidate(
                MigrationSource::OfficialCodex,
                "official-auth",
                None,
                None,
            )]);
        }
    };
    Ok(vec![candidate_from_value(
        state,
        MigrationSource::OfficialCodex,
        "official-auth",
        &value,
        None,
    )?])
}

fn discover_cockpit_root(state: &AppState, root: &Path) -> Result<Vec<SourceCandidate>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    if !root.is_dir() {
        return Ok(vec![unsupported_candidate(
            MigrationSource::CockpitTools,
            "cockpit-root",
            None,
            None,
        )]);
    }

    let summaries = read_index_summaries(root);
    let detail_dir = root.join(COCKPIT_DETAILS_DIR_NAME);
    let mut detail_paths = BTreeMap::new();
    if let Ok(entries) = fs::read_dir(&detail_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !is_safe_record_id(stem) {
                continue;
            }
            detail_paths.insert(stem.to_string(), path);
        }
    }

    let mut record_ids: Vec<String> = summaries.keys().cloned().collect();
    record_ids.extend(
        detail_paths
            .keys()
            .filter(|id| !summaries.contains_key(*id))
            .cloned(),
    );
    record_ids.sort();
    record_ids.dedup();
    if record_ids.len() > MAX_DETAIL_FILES {
        record_ids.truncate(MAX_DETAIL_FILES);
    }

    let mut total_bytes = 0u64;
    let mut candidates = Vec::new();
    for record_id in record_ids {
        let summary = summaries.get(&record_id).cloned();
        let Some(path) = detail_paths.get(&record_id) else {
            candidates.push(unsupported_candidate(
                MigrationSource::CockpitTools,
                &record_id,
                None,
                summary.as_ref(),
            ));
            continue;
        };
        let metadata = match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => {
                candidates.push(unsupported_candidate(
                    MigrationSource::CockpitTools,
                    &record_id,
                    None,
                    summary.as_ref(),
                ));
                continue;
            }
        };
        if metadata.len() > MAX_DETAIL_BYTES
            || (total_bytes.saturating_add(metadata.len()) > MAX_TOTAL_BYTES)
        {
            candidates.push(unsupported_candidate(
                MigrationSource::CockpitTools,
                &record_id,
                None,
                summary.as_ref(),
            ));
            continue;
        }
        total_bytes = total_bytes.saturating_add(metadata.len());

        match read_detail(path, root) {
            Ok(value) => candidates.push(candidate_from_value(
                state,
                MigrationSource::CockpitTools,
                &record_id,
                &value,
                summary.as_ref(),
            )?),
            Err(_) => candidates.push(unsupported_candidate(
                MigrationSource::CockpitTools,
                &record_id,
                None,
                summary.as_ref(),
            )),
        }
    }

    if candidates.is_empty() && root.join(COCKPIT_INDEX_NAME).is_file() {
        candidates.push(unsupported_candidate(
            MigrationSource::CockpitTools,
            "cockpit-index",
            None,
            None,
        ));
    }
    Ok(candidates)
}

fn read_index_summaries(root: &Path) -> BTreeMap<String, IndexSummary> {
    let path = root.join(COCKPIT_INDEX_NAME);
    if fs::metadata(&path)
        .ok()
        .is_some_and(|metadata| metadata.len() > MAX_DETAIL_BYTES)
    {
        return BTreeMap::new();
    }
    let Ok(raw) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return BTreeMap::new();
    };
    let entries = value
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| value.as_array().cloned())
        .unwrap_or_default();
    entries
        .into_iter()
        .filter_map(|entry| {
            let object = entry.as_object()?;
            let id = display_value(object.get("id"))?;
            Some((
                id.clone(),
                IndexSummary {
                    email: display_value(object.get("email")),
                    plan_type: display_value(
                        object.get("plan_type").or_else(|| object.get("planType")),
                    ),
                },
            ))
        })
        .collect()
}

fn read_detail(path: &Path, root: &Path) -> Result<Value, String> {
    let raw = fs::read_to_string(path).map_err(|_| "read failed".to_string())?;
    let value = serde_json::from_str::<Value>(&raw).map_err(|_| "parse failed".to_string())?;
    if value.get("ciphertext").is_none() && value.get("nonce").is_none() {
        return Ok(value);
    }

    let envelope: SecureEnvelope =
        serde_json::from_value(value).map_err(|_| "unsupported secure envelope".to_string())?;
    if envelope.version != SECURE_VERSION
        || envelope.kind != SECURE_KIND
        || envelope.algorithm != SECURE_ALGORITHM
        || envelope.key_id != SECURE_KEY_ID
    {
        return Err("unsupported secure envelope".to_string());
    }

    let key_raw = fs::read_to_string(root.join(COCKPIT_KEY_NAME))
        .map_err(|_| "secure storage key is unavailable".to_string())?;
    let key_bytes = STANDARD
        .decode(key_raw.trim())
        .map_err(|_| "secure storage key is invalid".to_string())?;
    if key_bytes.len() != 32 {
        return Err("secure storage key is invalid".to_string());
    }
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|_| "secure storage key is invalid".to_string())?;
    let nonce = STANDARD
        .decode(envelope.nonce.trim())
        .map_err(|_| "secure envelope nonce is invalid".to_string())?;
    if nonce.len() != 12 {
        return Err("secure envelope nonce is invalid".to_string());
    }
    let ciphertext = STANDARD
        .decode(envelope.ciphertext.trim())
        .map_err(|_| "secure envelope ciphertext is invalid".to_string())?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| "secure envelope could not be decrypted".to_string())?;
    serde_json::from_slice(&plaintext).map_err(|_| "secure detail is not valid JSON".to_string())
}

fn candidate_from_value(
    state: &AppState,
    source: MigrationSource,
    record_id: &str,
    value: &Value,
    summary: Option<&IndexSummary>,
) -> Result<SourceCandidate, String> {
    let credential = normalize_credential(value);
    let email = summary
        .and_then(|summary| summary.email.clone())
        .or_else(|| display_field(value, &["email", "account_email", "accountEmail"]))
        .or_else(|| {
            credential
                .as_ref()
                .and_then(identity::email_from_credential)
        });
    let workspace_name = display_field(
        value,
        &[
            "account_name",
            "accountName",
            "workspace_name",
            "workspaceName",
            "name",
        ],
    );
    let plan_type = summary
        .and_then(|summary| summary.plan_type.clone())
        .or_else(|| display_field(value, &["plan_type", "planType", "auth_file_plan_type"]));

    let Some(credential) = credential else {
        return Ok(unsupported_candidate(
            source,
            record_id,
            Some((email, workspace_name, plan_type)),
            summary,
        ));
    };
    let kind = match identity::document_kind(&credential) {
        Ok(kind) => kind,
        Err(_) => {
            return Ok(unsupported_candidate(
                source,
                record_id,
                Some((email, workspace_name, plan_type)),
                summary,
            ));
        }
    };
    let stable_identity = match identity::derive_identity(&kind, &credential) {
        Ok(identity) => identity,
        Err(_) => {
            return Ok(unsupported_candidate(
                source,
                record_id,
                Some((email, workspace_name, plan_type)),
                summary,
            ));
        }
    };
    let state_value = if state.account_by_identity(&stable_identity)?.is_some() {
        MigrationCandidateState::AlreadyPresent
    } else {
        MigrationCandidateState::New
    };
    let id = candidate_id(&source, record_id, Some(&stable_identity));
    Ok(SourceCandidate {
        view: MigrationCandidateView {
            id,
            source: source.clone(),
            email,
            workspace_name,
            plan_type,
            state: state_value,
        },
        identity: Some(stable_identity),
        credential: Some(credential),
    })
}

fn unsupported_candidate(
    source: MigrationSource,
    record_id: &str,
    metadata: Option<(Option<String>, Option<String>, Option<String>)>,
    summary: Option<&IndexSummary>,
) -> SourceCandidate {
    let (email, workspace_name, plan_type) = metadata.unwrap_or_else(|| {
        (
            summary.and_then(|summary| summary.email.clone()),
            None,
            summary.and_then(|summary| summary.plan_type.clone()),
        )
    });
    SourceCandidate {
        view: MigrationCandidateView {
            id: candidate_id(&source, record_id, None),
            source: source.clone(),
            email,
            workspace_name,
            plan_type,
            state: MigrationCandidateState::Unsupported,
        },
        identity: None,
        credential: None,
    }
}

fn normalize_credential(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    if object
        .get("auth_mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("agentidentity"))
        || object.contains_key("agent_identity")
    {
        return None;
    }

    if let Some(api_key) = object
        .get("OPENAI_API_KEY")
        .or_else(|| object.get("openai_api_key"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        return Some(json!({
            "auth_mode": "apikey",
            "OPENAI_API_KEY": api_key,
        }));
    }

    let tokens = object.get("tokens")?.as_object()?;
    let id_token = tokens
        .get("id_token")
        .or_else(|| tokens.get("idToken"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())?;
    let access_token = tokens
        .get("access_token")
        .or_else(|| tokens.get("accessToken"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())?;
    let mut normalized_tokens = serde_json::Map::new();
    normalized_tokens.insert("id_token".to_string(), Value::String(id_token.to_string()));
    normalized_tokens.insert(
        "access_token".to_string(),
        Value::String(access_token.to_string()),
    );
    if let Some(refresh_token) = tokens
        .get("refresh_token")
        .or_else(|| tokens.get("refreshToken"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        normalized_tokens.insert(
            "refresh_token".to_string(),
            Value::String(refresh_token.to_string()),
        );
    }
    if let Some(account_id) = object
        .get("account_id")
        .or_else(|| object.get("accountId"))
        .or_else(|| object.get("chatgpt_account_id"))
        .or_else(|| tokens.get("account_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        normalized_tokens.insert(
            "account_id".to_string(),
            Value::String(account_id.to_string()),
        );
    }

    Some(json!({
        "OPENAI_API_KEY": null,
        "tokens": normalized_tokens,
        "type": "codex"
    }))
}

fn resolve_cockpit_roots(custom_root: Option<String>) -> Result<Vec<(PathBuf, bool)>, String> {
    let home = user_home()?;
    let mut roots = Vec::new();
    push_unique_root(&mut roots, home.join(COCKPIT_ROOT_NAME), false);
    if let Some(legacy) = legacy_cockpit_root(&home) {
        if legacy.exists() {
            push_unique_root(&mut roots, legacy, false);
        }
    }
    if let Some(raw) = custom_root {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("The selected Cockpit folder is empty".to_string());
        }
        let path = PathBuf::from(trimmed);
        if !path.exists() || !path.is_dir() {
            return Err("The selected Cockpit folder could not be read".to_string());
        }
        push_unique_root(&mut roots, path, true);
    }
    Ok(roots)
}

fn push_unique_root(roots: &mut Vec<(PathBuf, bool)>, path: PathBuf, explicit: bool) {
    if roots.iter().any(|(existing, _)| existing == &path) {
        return;
    }
    roots.push((path, explicit));
}

fn legacy_cockpit_root(_home: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    let base = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = Some(_home.join("Library").join("Application Support"));
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let base = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(_home.join(".local").join("share")));
    base.map(|path| path.join(LEGACY_COCKPIT_ROOT_NAME))
}

fn user_home() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let value = env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let value = env::var_os("HOME");
    value
        .map(PathBuf::from)
        .ok_or_else(|| "Unable to resolve the user home directory".to_string())
}

fn is_safe_record_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains('\0')
}

fn display_field(value: &Value, fields: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    fields
        .iter()
        .find_map(|field| display_value(object.get(*field)))
}

fn display_value(value: Option<&Value>) -> Option<String> {
    let raw = value?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    let sanitized: String = raw
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect();
    (!sanitized.is_empty()).then_some(sanitized)
}

fn candidate_id(
    source: &MigrationSource,
    record_id: &str,
    stable_identity: Option<&AccountIdentity>,
) -> String {
    let source_name = match source {
        MigrationSource::OfficialCodex => "official_codex",
        MigrationSource::CockpitTools => "cockpit_tools",
    };
    let identity = stable_identity
        .and_then(|identity| serde_json::to_string(identity).ok())
        .unwrap_or_else(|| "unsupported".to_string());
    format!(
        "{:x}",
        Sha256::digest(format!("{source_name}\n{record_id}\n{identity}").as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{accounts::AccountDraft, types::AccountKind};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = env::temp_dir().join(format!("gswitch-migration-{name}-{suffix}"));
        fs::create_dir_all(&path).expect("temp directory");
        path
    }

    fn id_token(user: &str, workspace: &str, email: &str) -> String {
        let payload = json!({
            "email": email,
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

    fn credential(user: &str, workspace: &str, email: &str, access: &str) -> Value {
        json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": id_token(user, workspace, email),
                "access_token": access,
                "refresh_token": "refresh-secret"
            },
            "type": "codex"
        })
    }

    fn test_state(root: &Path) -> AppState {
        AppState::new(root.join("accounts.json")).expect("state")
    }

    fn write_index(root: &Path, id: &str, email: &str, plan_type: &str) {
        fs::create_dir_all(root.join(COCKPIT_DETAILS_DIR_NAME)).expect("details");
        fs::write(
            root.join(COCKPIT_INDEX_NAME),
            serde_json::to_vec(&json!({
                "version": "1.0",
                "detail_schema_version": 2,
                "accounts": [{
                    "id": id,
                    "email": email,
                    "plan_type": plan_type,
                    "created_at": 1,
                    "last_used": 1
                }],
                "current_account_id": id
            }))
            .expect("index"),
        )
        .expect("write index");
    }

    #[test]
    fn official_preview_is_sanitized_and_does_not_expose_tokens() {
        let root = temp_dir("official");
        let official = root.join("codex");
        fs::create_dir_all(&official).expect("official");
        let raw = credential("user", "workspace", "person@example.com", "access-secret");
        fs::write(
            official.join("auth.json"),
            serde_json::to_vec(&raw).unwrap(),
        )
        .expect("auth");
        let state = test_state(&root);

        let candidates = discover_candidates(&state, &official, &[]).expect("discover");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].view.source, MigrationSource::OfficialCodex);
        assert_eq!(
            candidates[0].view.email.as_deref(),
            Some("person@example.com")
        );
        let preview = serde_json::to_string(&candidates[0].view).expect("preview");
        assert!(!preview.contains("access-secret"));
        assert!(!preview.contains("refresh-secret"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn plaintext_cockpit_details_use_only_the_documented_direct_paths() {
        let root = temp_dir("plaintext");
        let cockpit = root.join("cockpit");
        write_index(&cockpit, "account-1", "person@example.com", "plus");
        let detail = json!({
            "id": "account-1",
            "email": "person@example.com",
            "account_name": "Personal",
            "plan_type": "plus",
            "tokens": credential("user", "workspace", "person@example.com", "access-secret")["tokens"]
        });
        fs::write(
            cockpit
                .join(COCKPIT_DETAILS_DIR_NAME)
                .join("account-1.json"),
            serde_json::to_vec(&detail).unwrap(),
        )
        .expect("detail");
        fs::create_dir_all(cockpit.join("nested")).expect("nested");
        fs::write(cockpit.join("nested").join("ignored.json"), b"not read").expect("ignored");
        let state = test_state(&root);

        let candidates = discover_candidates(
            &state,
            &root.join("missing-codex"),
            &[(cockpit.clone(), true)],
        )
        .expect("discover");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].view.source, MigrationSource::CockpitTools);
        assert_eq!(
            candidates[0].view.workspace_name.as_deref(),
            Some("Personal")
        );
        assert_eq!(candidates[0].view.plan_type.as_deref(), Some("plus"));
        let preview = serde_json::to_string(&candidates[0].view).unwrap();
        assert!(!preview.contains("access-secret"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn encrypted_v1_details_are_read_without_creating_or_rewriting_source_files() {
        let root = temp_dir("encrypted");
        let cockpit = root.join("cockpit");
        write_index(&cockpit, "account-1", "person@example.com", "team");
        let key = [7u8; 32];
        fs::write(cockpit.join(COCKPIT_KEY_NAME), STANDARD.encode(key)).expect("key");
        let nonce = [9u8; 12];
        let plaintext = credential("user", "workspace", "person@example.com", "access-secret");
        let cipher = Aes256Gcm::new_from_slice(&key).expect("cipher");
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                serde_json::to_vec(&plaintext).unwrap().as_ref(),
            )
            .expect("encrypt");
        let envelope = json!({
            "version": 1,
            "kind": "codex",
            "algorithm": "AES-256-GCM",
            "key_id": "local-secure-account-storage-v1",
            "nonce": STANDARD.encode(nonce),
            "ciphertext": STANDARD.encode(ciphertext),
            "encrypted_at": 1
        });
        let detail_path = cockpit
            .join(COCKPIT_DETAILS_DIR_NAME)
            .join("account-1.json");
        let key_path = cockpit.join(COCKPIT_KEY_NAME);
        let before_detail = serde_json::to_vec(&envelope).unwrap();
        fs::write(&detail_path, &before_detail).expect("detail");
        let before_key = fs::read(&key_path).expect("key before");
        let state = test_state(&root);

        let candidates = discover_candidates(
            &state,
            &root.join("missing-codex"),
            &[(cockpit.clone(), true)],
        )
        .expect("discover");
        assert_eq!(candidates[0].view.state, MigrationCandidateState::New);
        assert_eq!(fs::read(&detail_path).unwrap(), before_detail);
        assert_eq!(fs::read(&key_path).unwrap(), before_key);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_key_and_bad_envelopes_fail_closed_without_creating_a_key() {
        let root = temp_dir("bad-envelope");
        let cockpit = root.join("cockpit");
        write_index(&cockpit, "account-1", "person@example.com", "plus");
        let detail_path = cockpit
            .join(COCKPIT_DETAILS_DIR_NAME)
            .join("account-1.json");
        fs::write(
            &detail_path,
            serde_json::to_vec(&json!({
                "version": 2,
                "kind": "codex",
                "algorithm": "AES-256-GCM",
                "key_id": "local-secure-account-storage-v1",
                "nonce": STANDARD.encode([0u8; 12]),
                "ciphertext": STANDARD.encode([1u8; 16]),
                "encrypted_at": 1
            }))
            .unwrap(),
        )
        .expect("detail");
        let state = test_state(&root);
        let candidates = discover_candidates(
            &state,
            &root.join("missing-codex"),
            &[(cockpit.clone(), true)],
        )
        .expect("discover");
        assert_eq!(
            candidates[0].view.state,
            MigrationCandidateState::Unsupported
        );
        assert!(!cockpit.join(COCKPIT_KEY_NAME).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn existing_identity_is_previewed_as_already_present_and_changed_identity_is_stale() {
        let root = temp_dir("identity");
        let cockpit = root.join("cockpit");
        write_index(&cockpit, "account-1", "person@example.com", "plus");
        let detail_path = cockpit
            .join(COCKPIT_DETAILS_DIR_NAME)
            .join("account-1.json");
        let first = credential("user", "workspace", "person@example.com", "access-one");
        fs::write(&detail_path, serde_json::to_vec(&first).unwrap()).expect("detail");
        let state = test_state(&root);
        let kind = identity::document_kind(&first).unwrap();
        let stable = identity::derive_identity(&kind, &first).unwrap();
        {
            let operation = state.acquire_operation().expect("operation");
            state
                .upsert_under_operation(
                    &operation,
                    AccountDraft {
                        label: None,
                        default_label: "person@example.com".to_string(),
                        kind: AccountKind::ChatGpt,
                        email: Some("person@example.com".to_string()),
                        plan_type: None,
                        workspace_name: None,
                        account_structure: None,
                        identity: stable,
                        credential: first.clone(),
                    },
                )
                .expect("save");
        }
        let initial = discover_candidates(
            &state,
            &root.join("missing-codex"),
            &[(cockpit.clone(), true)],
        )
        .expect("discover");
        assert_eq!(
            initial[0].view.state,
            MigrationCandidateState::AlreadyPresent
        );

        let changed = credential(
            "other-user",
            "other-workspace",
            "other@example.com",
            "access-two",
        );
        fs::write(&detail_path, serde_json::to_vec(&changed).unwrap()).expect("changed detail");
        let refreshed = discover_candidates(
            &state,
            &root.join("missing-codex"),
            &[(cockpit.clone(), true)],
        )
        .expect("rediscover");
        assert_ne!(initial[0].view.id, refreshed[0].view.id);
        let error = confirm_candidates(&state, &refreshed, vec![initial[0].view.id.clone()])
            .expect_err("stale preview");
        assert!(error.contains("stale"));
        let _ = fs::remove_dir_all(root);
    }
}
