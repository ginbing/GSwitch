use std::{
    fs,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    accounts::{AppState, OperationGuard},
    app_server::{AppServer, TempCodexHome},
    codex,
    identity::{derive_identity, document_kind},
    quota::{
        normalize_rate_limits_data, now_unix_ms, persist_refreshed_credential_and_quota,
        verified_chatgpt_identity,
    },
    runtime,
    types::{
        AccountIdentity, AccountKind, CredentialStoreMode, QuotaBucketKind, QuotaSnapshot,
        QuotaWindowKind, StoredAccount, WakeAccountResult, WakeOperationStatus, WakeOperationView,
        WakeResultKind, WakeStart,
    },
};

const TURN_TIMEOUT: Duration = Duration::from_secs(90);
const MODEL_PREFERENCE: [&str; 2] = ["gpt-5.6-luna", "gpt-5.4-mini"];
const REASONING_PREFERENCE: [&str; 7] =
    ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

#[derive(Clone)]
struct WakeTarget {
    id: String,
    label: String,
}

struct WakeModel {
    model: String,
    reasoning_effort: String,
    service_tier: Option<String>,
}

struct WakePersistence<'a> {
    account: &'a StoredAccount,
    identity: &'a AccountIdentity,
    before: &'a QuotaSnapshot,
    reset_credits: Option<crate::types::StoredResetCredits>,
}

enum ModelSelection {
    Selected(WakeModel),
    NeedsExplicitChoice(Vec<String>),
}

struct WakeAccountOutcome {
    result: WakeResultKind,
    message: String,
    available_models: Vec<String>,
}

impl WakeAccountOutcome {
    fn new(result: WakeResultKind, message: impl Into<String>) -> Self {
        Self {
            result,
            message: message.into(),
            available_models: Vec::new(),
        }
    }

    fn needs_model_selection(models: Vec<String>) -> Self {
        Self {
            result: WakeResultKind::NeedsModelSelection,
            message:
                "No approved automatic Wake model is available. Choose a listed model explicitly."
                    .to_string(),
            available_models: models,
        }
    }
}

/// Starts a user-triggered one-account Wake operation. The returned ID refers
/// only to in-memory progress for this GSwitch process.
pub fn start_one(
    state: AppState,
    account_id: String,
    model: Option<String>,
) -> Result<WakeStart, String> {
    let account = state.account_by_id(&account_id)?;
    start(
        state,
        vec![WakeTarget {
            id: account.id,
            label: account.label,
        }],
        model.filter(|model| !model.trim().is_empty()),
    )
}

/// Starts a sequential Wake queue for every saved ChatGPT account. API-key
/// accounts do not participate because they have no subscription window.
pub fn start_all(state: AppState) -> Result<WakeStart, String> {
    let targets = state
        .list()?
        .into_iter()
        .filter(|account| account.kind == AccountKind::ChatGpt)
        .map(|account| WakeTarget {
            id: account.id,
            label: account.label,
        })
        .collect::<Vec<_>>();
    start(state, targets, None)
}

pub fn operation(state: &AppState, operation_id: &str) -> Result<WakeOperationView, String> {
    state.wake_operation(operation_id)
}

pub fn cancel(state: &AppState, operation_id: &str) -> Result<(), String> {
    state.cancel_wake(operation_id)
}

fn start(
    state: AppState,
    targets: Vec<WakeTarget>,
    explicit_model: Option<String>,
) -> Result<WakeStart, String> {
    if targets.is_empty() {
        return Err("There are no ChatGPT accounts to wake".to_string());
    }

    let operation_id = Uuid::new_v4().to_string();
    let cancelled = Arc::new(AtomicBool::new(false));
    state.insert_wake_operation(
        WakeOperationView {
            id: operation_id.clone(),
            status: WakeOperationStatus::Running,
            current_account_id: None,
            results: Vec::new(),
        },
        cancelled.clone(),
    )?;

    let worker_state = state.clone();
    let worker_id = operation_id.clone();
    if thread::Builder::new()
        .name("gswitch-wake".to_string())
        .spawn(move || run_queue(worker_state, worker_id, targets, explicit_model, cancelled))
        .is_err()
    {
        let _ = state.update_wake_operation(WakeOperationView {
            id: operation_id.clone(),
            status: WakeOperationStatus::Failed,
            current_account_id: None,
            results: Vec::new(),
        });
        return Err("Unable to start the Wake operation".to_string());
    }

    Ok(WakeStart { operation_id })
}

fn run_queue(
    state: AppState,
    operation_id: String,
    targets: Vec<WakeTarget>,
    explicit_model: Option<String>,
    cancelled: Arc<AtomicBool>,
) {
    let result = state.acquire_operation().and_then(|operation| {
        run_targets(
            &state,
            &operation,
            &operation_id,
            &targets,
            explicit_model.as_deref(),
            &cancelled,
        )
    });

    if let Err(message) = result {
        let _ = fail_operation(&state, &operation_id, &targets, message);
    }
}

fn run_targets(
    state: &AppState,
    operation: &OperationGuard<'_>,
    operation_id: &str,
    targets: &[WakeTarget],
    explicit_model: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let mut view = state.wake_operation(operation_id)?;

    for (index, target) in targets.iter().enumerate() {
        if cancelled.load(Ordering::SeqCst) {
            view.status = WakeOperationStatus::Cancelled;
            append_cancelled_targets(&mut view, &targets[index..]);
            break;
        }

        view.current_account_id = Some(target.id.clone());
        state.update_wake_operation(view.clone())?;
        let outcome = match state.account_by_id_under_operation(operation, &target.id) {
            Ok(account) => wake_account(state, operation, &account, explicit_model, cancelled),
            Err(error) => WakeAccountOutcome::new(WakeResultKind::Failed, error),
        };
        view.results.push(WakeAccountResult {
            account_id: target.id.clone(),
            label: target.label.clone(),
            result: outcome.result,
            message: outcome.message,
            available_models: outcome.available_models,
        });
        state.update_wake_operation(view.clone())?;
    }

    view.current_account_id = None;
    if view.status == WakeOperationStatus::Running {
        view.status = if cancelled.load(Ordering::SeqCst) {
            WakeOperationStatus::Cancelled
        } else {
            WakeOperationStatus::Completed
        };
    }
    state.update_wake_operation(view)
}

fn append_cancelled_targets(view: &mut WakeOperationView, targets: &[WakeTarget]) {
    for target in targets {
        view.results.push(WakeAccountResult {
            account_id: target.id.clone(),
            label: target.label.clone(),
            result: WakeResultKind::Cancelled,
            message: "Wake queue was cancelled before this account started".to_string(),
            available_models: Vec::new(),
        });
    }
}

fn fail_operation(
    state: &AppState,
    operation_id: &str,
    targets: &[WakeTarget],
    message: String,
) -> Result<(), String> {
    let mut view = state.wake_operation(operation_id)?;
    view.current_account_id = None;
    view.status = WakeOperationStatus::Failed;
    for target in targets.iter().skip(view.results.len()) {
        view.results.push(WakeAccountResult {
            account_id: target.id.clone(),
            label: target.label.clone(),
            result: WakeResultKind::Failed,
            message: message.clone(),
            available_models: Vec::new(),
        });
    }
    state.update_wake_operation(view)
}

fn wake_account(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    explicit_model: Option<&str>,
    cancelled: &AtomicBool,
) -> WakeAccountOutcome {
    if account.kind != AccountKind::ChatGpt {
        return WakeAccountOutcome::new(
            WakeResultKind::Skipped,
            "API-key accounts do not use ChatGPT subscription windows",
        );
    }
    if cancelled.load(Ordering::SeqCst) {
        return WakeAccountOutcome::new(WakeResultKind::Cancelled, "Wake was cancelled");
    }

    let identity = match verified_chatgpt_identity(account) {
        Ok(identity) => identity,
        Err(error) => return WakeAccountOutcome::new(WakeResultKind::Failed, error),
    };
    match external_runtime_uses_account(&identity) {
        Ok(true) => {
            return WakeAccountOutcome::new(
                WakeResultKind::Skipped,
                "Codex is currently running with this account, so Wake skipped it safely",
            )
        }
        Ok(false) => {}
        Err(error) => return WakeAccountOutcome::new(WakeResultKind::Skipped, error),
    }

    let profile_root = match state.isolated_profile_root() {
        Ok(root) => root,
        Err(error) => return WakeAccountOutcome::new(WakeResultKind::Failed, error),
    };
    let mut profile = match TempCodexHome::create(&profile_root) {
        Ok(profile) => profile,
        Err(error) => return WakeAccountOutcome::new(WakeResultKind::Failed, error),
    };
    if let Err(error) = profile.write_auth(&account.credential) {
        return WakeAccountOutcome::new(WakeResultKind::Failed, error);
    }
    let workspace = profile.path.join("wake-workspace");
    if fs::create_dir(&workspace).is_err() {
        return WakeAccountOutcome::new(
            WakeResultKind::Failed,
            "Unable to prepare an isolated Wake workspace",
        );
    }
    let mut server = match AppServer::start(&profile.path) {
        Ok(server) => server,
        Err(error) => return WakeAccountOutcome::new(WakeResultKind::Failed, error),
    };

    let before_raw = match server.rate_limits_read(1) {
        Ok(value) => value,
        Err(error) => return WakeAccountOutcome::new(WakeResultKind::Failed, error),
    };
    let before = normalize_rate_limits_data(&before_raw, now_unix_ms());
    if let Err(error) = persist_wake_state(
        state,
        operation,
        account,
        &identity,
        &before.snapshot,
        before.reset_credits.clone(),
        &mut profile,
    ) {
        return WakeAccountOutcome::new(WakeResultKind::Failed, error);
    }

    let now_seconds = now_unix_ms() / 1000;
    if window_is_active(&before.snapshot, now_seconds) {
        return WakeAccountOutcome::new(
            WakeResultKind::AlreadyActive,
            "The five-hour window is already active",
        );
    }
    if ordinary_quota_exhausted(&before.snapshot, now_seconds) {
        return WakeAccountOutcome::new(
            WakeResultKind::Skipped,
            "Ordinary Codex quota is exhausted; Wake will not use Reserve or reset credits",
        );
    }
    if cancelled.load(Ordering::SeqCst) {
        return WakeAccountOutcome::new(WakeResultKind::Cancelled, "Wake was cancelled");
    }

    let models = match list_models(&mut server) {
        Ok(models) => models,
        Err(error) => return WakeAccountOutcome::new(WakeResultKind::Failed, error),
    };
    // Any App Server request may rotate a token. Persist after catalog lookup
    // before the potentially billable turn begins.
    if let Err(error) = persist_wake_state(
        state,
        operation,
        account,
        &identity,
        &before.snapshot,
        before.reset_credits.clone(),
        &mut profile,
    ) {
        return WakeAccountOutcome::new(WakeResultKind::Failed, error);
    }
    let selected = match select_model(&models, explicit_model) {
        Ok(ModelSelection::Selected(model)) => model,
        Ok(ModelSelection::NeedsExplicitChoice(models)) => {
            return WakeAccountOutcome::needs_model_selection(models)
        }
        Err(error) => return WakeAccountOutcome::new(WakeResultKind::Failed, error),
    };
    if cancelled.load(Ordering::SeqCst) {
        return WakeAccountOutcome::new(WakeResultKind::Cancelled, "Wake was cancelled");
    }

    let mut thread_params = json!({
        "model": selected.model,
        "cwd": workspace.to_string_lossy(),
        "approvalPolicy": "never",
        "sandbox": "read-only",
        "ephemeral": true,
        "baseInstructions": "Reply only to the user message. Do not use tools or inspect files.",
        "developerInstructions": "Return exactly OK."
    });
    if let Some(service_tier) = selected.service_tier {
        thread_params["serviceTier"] = Value::String(service_tier);
    }
    let thread = match server.thread_start(50, thread_params) {
        Ok(thread) => thread,
        Err(error) => {
            return persist_after_turn(
                state,
                operation,
                WakePersistence {
                    account,
                    identity: &identity,
                    before: &before.snapshot,
                    reset_credits: before.reset_credits.clone(),
                },
                &mut profile,
                WakeResultKind::Failed,
                error,
            )
        }
    };
    let Some(thread_id) = thread.pointer("/thread/id").and_then(Value::as_str) else {
        return persist_after_turn(
            state,
            operation,
            WakePersistence {
                account,
                identity: &identity,
                before: &before.snapshot,
                reset_credits: before.reset_credits.clone(),
            },
            &mut profile,
            WakeResultKind::Failed,
            "Codex did not return a Wake thread".to_string(),
        );
    };
    if cancelled.load(Ordering::SeqCst) {
        return persist_after_turn(
            state,
            operation,
            WakePersistence {
                account,
                identity: &identity,
                before: &before.snapshot,
                reset_credits: before.reset_credits.clone(),
            },
            &mut profile,
            WakeResultKind::Cancelled,
            "Wake was cancelled before its request started".to_string(),
        );
    }
    let turn = match server.turn_start(
        51,
        json!({
            "threadId": thread_id,
            "effort": selected.reasoning_effort,
            "input": [{"type": "text", "text": "Reply with exactly OK."}]
        }),
    ) {
        Ok(turn) => turn,
        Err(error) => {
            return unconfirmed_after_turn(
                state,
                operation,
                WakePersistence {
                    account,
                    identity: &identity,
                    before: &before.snapshot,
                    reset_credits: before.reset_credits.clone(),
                },
                &mut profile,
                format!("Wake could not confirm whether its request started: {error}"),
            )
        }
    };
    let Some(turn_id) = turn.pointer("/turn/id").and_then(Value::as_str) else {
        return unconfirmed_after_turn(
            state,
            operation,
            WakePersistence {
                account,
                identity: &identity,
                before: &before.snapshot,
                reset_credits: before.reset_credits.clone(),
            },
            &mut profile,
            "Wake could not confirm whether its request started".to_string(),
        );
    };

    let notification = match server.wait_for_notification_cancelled(
        TURN_TIMEOUT,
        |message| {
            message.get("method").and_then(Value::as_str) == Some("turn/completed")
                && message.pointer("/params/threadId").and_then(Value::as_str) == Some(thread_id)
                && message.pointer("/params/turn/id").and_then(Value::as_str) == Some(turn_id)
        },
        || cancelled.load(Ordering::SeqCst),
    ) {
        Ok(notification) => notification,
        Err(error) if error == "The operation was cancelled" => {
            let _ = server.turn_interrupt(90, thread_id, turn_id);
            return persist_after_turn(
                state,
                operation,
                WakePersistence {
                    account,
                    identity: &identity,
                    before: &before.snapshot,
                    reset_credits: before.reset_credits.clone(),
                },
                &mut profile,
                WakeResultKind::Cancelled,
                "Wake was cancelled".to_string(),
            );
        }
        Err(error) => {
            return unconfirmed_after_turn(
                state,
                operation,
                WakePersistence {
                    account,
                    identity: &identity,
                    before: &before.snapshot,
                    reset_credits: before.reset_credits.clone(),
                },
                &mut profile,
                format!("Wake request result is unknown and was not retried: {error}"),
            )
        }
    };
    if notification
        .pointer("/params/turn/status")
        .and_then(Value::as_str)
        != Some("completed")
    {
        return persist_after_turn(
            state,
            operation,
            WakePersistence {
                account,
                identity: &identity,
                before: &before.snapshot,
                reset_credits: before.reset_credits.clone(),
            },
            &mut profile,
            WakeResultKind::Failed,
            "The Wake request did not complete successfully".to_string(),
        );
    }

    let after_raw =
        match server.rate_limits_read(60) {
            Ok(value) => value,
            Err(_) => return unconfirmed_after_turn(
                state,
                operation,
                WakePersistence {
                    account,
                    identity: &identity,
                    before: &before.snapshot,
                    reset_credits: before.reset_credits.clone(),
                },
                &mut profile,
                "The Wake request completed, but the quota response did not confirm a new window"
                    .to_string(),
            ),
        };
    let after = normalize_rate_limits_data(&after_raw, now_unix_ms());
    if let Err(error) = persist_wake_state(
        state,
        operation,
        account,
        &identity,
        &after.snapshot,
        after.reset_credits,
        &mut profile,
    ) {
        return WakeAccountOutcome::new(
            WakeResultKind::RequestCompletedUnconfirmed,
            format!("The Wake request completed, but {error}"),
        );
    }
    if window_started(&before.snapshot, &after.snapshot, now_unix_ms() / 1000) {
        WakeAccountOutcome::new(
            WakeResultKind::WindowStarted,
            "The five-hour window is active",
        )
    } else {
        WakeAccountOutcome::new(
            WakeResultKind::RequestCompletedUnconfirmed,
            "The Wake request completed, but the quota response did not confirm a new window",
        )
    }
}

fn external_runtime_uses_account(identity: &AccountIdentity) -> Result<bool, String> {
    if !runtime::external_codex_running(&[])? {
        return Ok(false);
    }
    let home = codex::codex_home()?;
    if codex::credential_store_mode(&home)? != CredentialStoreMode::File {
        return Err(
            "Codex is running and GSwitch cannot safely identify its account for Wake".to_string(),
        );
    }
    let credential = codex::read_optional_auth_document(&home)?.ok_or_else(|| {
        "Codex is running and GSwitch cannot safely identify its account for Wake".to_string()
    })?;
    let kind = document_kind(&credential)?;
    if kind != AccountKind::ChatGpt {
        return Ok(false);
    }
    Ok(derive_identity(&kind, &credential)? == *identity)
}

fn persist_wake_state(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
    snapshot: &QuotaSnapshot,
    reset_credits: Option<crate::types::StoredResetCredits>,
    profile: &mut TempCodexHome,
) -> Result<(), String> {
    let credential = profile.read_auth()?;
    if document_kind(&credential)? != AccountKind::ChatGpt
        || derive_identity(&AccountKind::ChatGpt, &credential)? != *identity
    {
        profile.retain_for_recovery();
        return Err(
            "Codex changed identity during Wake. A protected recovery copy was retained."
                .to_string(),
        );
    }
    persist_refreshed_credential_and_quota(
        state,
        operation,
        &account.id,
        &credential,
        snapshot.clone(),
        reset_credits,
        profile,
    )
}

fn unconfirmed_after_turn(
    state: &AppState,
    operation: &OperationGuard<'_>,
    persistence: WakePersistence<'_>,
    profile: &mut TempCodexHome,
    message: String,
) -> WakeAccountOutcome {
    persist_after_turn(
        state,
        operation,
        persistence,
        profile,
        WakeResultKind::RequestCompletedUnconfirmed,
        message,
    )
}

fn persist_after_turn(
    state: &AppState,
    operation: &OperationGuard<'_>,
    persistence: WakePersistence<'_>,
    profile: &mut TempCodexHome,
    result: WakeResultKind,
    message: String,
) -> WakeAccountOutcome {
    let mut stale = persistence.before.clone();
    stale.fetched_at_unix_ms = 0;
    match persist_wake_state(
        state,
        operation,
        persistence.account,
        persistence.identity,
        &stale,
        persistence.reset_credits,
        profile,
    ) {
        Ok(()) => WakeAccountOutcome::new(result, message),
        Err(error) => WakeAccountOutcome::new(result, format!("{message}. {error}")),
    }
}

fn list_models(server: &mut AppServer) -> Result<Vec<Value>, String> {
    let mut cursor = None;
    let mut models = Vec::new();
    for request_id in 100..120 {
        let page = server.model_list(request_id, cursor.as_deref())?;
        models.extend(
            page.get("data")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        cursor = page
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())
            .map(ToString::to_string);
        if cursor.is_none() {
            return Ok(models);
        }
    }
    Err("Codex returned too many model catalog pages".to_string())
}

fn select_model(models: &[Value], explicit: Option<&str>) -> Result<ModelSelection, String> {
    if let Some(explicit) = explicit {
        return models
            .iter()
            .find(|model| model_matches(model, explicit))
            .and_then(wake_model_from)
            .map(ModelSelection::Selected)
            .ok_or_else(|| {
                "The selected Wake model is not available as a visible text model".to_string()
            });
    }

    for preferred in MODEL_PREFERENCE {
        if let Some(model) = models
            .iter()
            .find(|model| model_matches(model, preferred))
            .and_then(wake_model_from)
        {
            return Ok(ModelSelection::Selected(model));
        }
    }

    let mut alternatives = models
        .iter()
        .filter_map(wake_model_from)
        .map(|model| model.model)
        .collect::<Vec<_>>();
    alternatives.sort();
    alternatives.dedup();
    if alternatives.is_empty() {
        Err("No visible text model can perform Wake for this account".to_string())
    } else {
        Ok(ModelSelection::NeedsExplicitChoice(alternatives))
    }
}

fn wake_model_from(model: &Value) -> Option<WakeModel> {
    if model
        .get("hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || !supports_text(model)
    {
        return None;
    }
    let model_id = model_identifier(model)?.to_string();
    let reasoning_effort = lowest_reasoning_effort(model)?;
    let service_tier = normal_service_tier(model)?;
    Some(WakeModel {
        model: model_id,
        reasoning_effort,
        service_tier,
    })
}

fn model_matches(model: &Value, candidate: &str) -> bool {
    [model.get("id"), model.get("model")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|id| id == candidate)
}

fn model_identifier(model: &Value) -> Option<&str> {
    model
        .get("model")
        .or_else(|| model.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn supports_text(model: &Value) -> bool {
    model
        .get("inputModalities")
        .and_then(Value::as_array)
        .is_none_or(|modalities| {
            modalities
                .iter()
                .any(|modality| modality.as_str() == Some("text"))
        })
}

fn lowest_reasoning_effort(model: &Value) -> Option<String> {
    let efforts = model
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|effort| {
            effort
                .get("reasoningEffort")
                .or(Some(effort))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>();
    REASONING_PREFERENCE
        .iter()
        .find(|candidate| efforts.iter().any(|effort| effort == *candidate))
        .map(|effort| (*effort).to_string())
}

fn normal_service_tier(model: &Value) -> Option<Option<String>> {
    let Some(tiers) = model.get("serviceTiers").and_then(Value::as_array) else {
        return Some(None);
    };
    if tiers.is_empty() {
        return Some(None);
    }
    let standard = tiers
        .iter()
        .find(|tier| tier.get("id").and_then(Value::as_str) == Some("standard"))
        .map(|_| "standard".to_string());
    standard.map(Some)
}

fn five_hour(snapshot: &QuotaSnapshot) -> Option<&crate::types::QuotaWindow> {
    snapshot
        .buckets
        .iter()
        .find(|bucket| bucket.kind == QuotaBucketKind::Codex)
        .and_then(|bucket| {
            bucket
                .windows
                .iter()
                .find(|window| window.kind == QuotaWindowKind::FiveHour)
        })
}

fn window_is_active(snapshot: &QuotaSnapshot, now_seconds: i64) -> bool {
    five_hour(snapshot).is_some_and(|window| {
        window.resets_at.is_some_and(|reset| reset > now_seconds)
            && window
                .remaining_percent
                .is_some_and(|remaining| remaining > 0)
    })
}

fn ordinary_quota_exhausted(snapshot: &QuotaSnapshot, now_seconds: i64) -> bool {
    if snapshot.ordinary_usage_allowed == Some(false) {
        return true;
    }
    snapshot
        .buckets
        .iter()
        .find(|bucket| bucket.kind == QuotaBucketKind::Codex)
        .is_some_and(|bucket| {
            bucket.windows.iter().any(|window| {
                window.remaining_percent == Some(0)
                    && window.resets_at.is_none_or(|reset| reset > now_seconds)
            })
        })
}

fn window_started(before: &QuotaSnapshot, after: &QuotaSnapshot, now_seconds: i64) -> bool {
    let Some(after_window) = five_hour(after) else {
        return false;
    };
    let after_reset = after_window.resets_at;
    after_reset.is_some_and(|reset| reset > now_seconds)
        && after_window.used_percent.is_some_and(|used| used > 0)
        && five_hour(before).and_then(|window| window.resets_at) != after_reset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(used: u8, remaining: u8, resets_at: i64) -> QuotaSnapshot {
        QuotaSnapshot {
            fetched_at_unix_ms: 0,
            account_id: None,
            ordinary_usage_allowed: None,
            buckets: vec![crate::types::QuotaBucket {
                limit_id: "codex".into(),
                limit_name: None,
                plan_type: None,
                rate_limit_reached_type: None,
                kind: QuotaBucketKind::Codex,
                windows: vec![crate::types::QuotaWindow {
                    kind: QuotaWindowKind::FiveHour,
                    used_percent: Some(used),
                    remaining_percent: Some(remaining),
                    window_duration_mins: Some(300),
                    resets_at: Some(resets_at),
                }],
            }],
            reset_credits: None,
        }
    }

    #[test]
    fn selects_an_approved_model_with_the_lowest_supported_effort() {
        let models = vec![json!({
            "id": "gpt-5.6-luna",
            "model": "gpt-5.6-luna",
            "hidden": false,
            "inputModalities": ["text"],
            "supportedReasoningEfforts": [
                {"reasoningEffort": "medium"},
                {"reasoningEffort": "low"}
            ],
            "serviceTiers": [{"id": "standard"}]
        })];

        let ModelSelection::Selected(selected) = select_model(&models, None).expect("model") else {
            panic!("automatic model expected")
        };
        assert_eq!(selected.model, "gpt-5.6-luna");
        assert_eq!(selected.reasoning_effort, "low");
        assert_eq!(selected.service_tier.as_deref(), Some("standard"));
    }

    #[test]
    fn never_automatically_falls_back_to_an_unapproved_model() {
        let models = vec![json!({
            "id": "gpt-6-astra",
            "model": "gpt-6-astra",
            "hidden": false,
            "inputModalities": ["text"],
            "supportedReasoningEfforts": [{"reasoningEffort": "low"}],
            "serviceTiers": [{"id": "standard"}]
        })];

        let ModelSelection::NeedsExplicitChoice(alternatives) =
            select_model(&models, None).expect("explicit choice")
        else {
            panic!("automatic selection must not use Astra")
        };
        assert_eq!(alternatives, vec!["gpt-6-astra"]);
    }

    #[test]
    fn rejects_hidden_or_non_text_explicit_models() {
        let models = vec![
            json!({
                "id": "hidden", "model": "hidden", "hidden": true,
                "supportedReasoningEfforts": [{"reasoningEffort": "low"}]
            }),
            json!({
                "id": "audio", "model": "audio", "hidden": false,
                "inputModalities": ["audio"],
                "supportedReasoningEfforts": [{"reasoningEffort": "low"}]
            }),
        ];
        assert!(select_model(&models, Some("hidden")).is_err());
        assert!(select_model(&models, Some("audio")).is_err());
    }

    #[test]
    fn requires_the_normal_service_tier_for_automatic_and_explicit_wake() {
        let models = vec![json!({
            "id": "gpt-5.6-luna",
            "model": "gpt-5.6-luna",
            "hidden": false,
            "inputModalities": ["text"],
            "supportedReasoningEfforts": [{"reasoningEffort": "low"}],
            "serviceTiers": [{"id": "priority"}]
        })];

        assert!(select_model(&models, None).is_err());
        assert!(select_model(&models, Some("gpt-5.6-luna")).is_err());
    }

    #[test]
    fn skips_active_or_exhausted_ordinary_windows() {
        assert!(window_is_active(&snapshot(10, 90, 200), 100));
        assert!(ordinary_quota_exhausted(&snapshot(100, 0, 200), 100));
        assert!(!ordinary_quota_exhausted(&snapshot(100, 0, 99), 100));
    }

    #[test]
    fn confirms_a_new_window_only_when_the_reset_marker_changes() {
        assert!(window_started(
            &snapshot(100, 0, 99),
            &snapshot(1, 99, 400),
            100
        ));
        assert!(!window_started(
            &snapshot(99, 1, 400),
            &snapshot(1, 99, 400),
            100
        ));
    }
}
