use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

use serde_json::Value;
use uuid::Uuid;

use crate::{
    accounts::{AppState, OperationGuard},
    chatgpt::{ChatGptClient, RequestFailureKind},
    quota::{
        external_credential_state_for_identity, now_unix_ms, refresh_quota_snapshot,
        refresh_via_managed_profile_for_wake, verified_chatgpt_identity, ExternalCredentialState,
        ReadOnlyRefreshFailure,
    },
    types::{
        AccountIdentity, AccountKind, QuotaBucketKind, QuotaSnapshot, QuotaWindowKind,
        StoredAccount, WakeAccountResult, WakeOperationStatus, WakeOperationView, WakeRequestState,
        WakeResultKind, WakeStart,
    },
};

/// Current Codex exposes this lightweight visible text model on the standard
/// service tier. Wake deliberately sends one request only; it never probes or
/// falls back to another model after the provider may have received it.
const WAKE_MODEL: &str = "gpt-5.6-luna";

#[derive(Clone)]
struct WakeTarget {
    id: String,
    label: String,
}

struct WakeAccountOutcome {
    result: WakeResultKind,
    request_state: WakeRequestState,
    message: String,
}

impl WakeAccountOutcome {
    fn new(result: WakeResultKind, message: impl Into<String>) -> Self {
        Self {
            result,
            request_state: WakeRequestState::NotSent,
            message: message.into(),
        }
    }

    fn request_state(mut self, request_state: WakeRequestState) -> Self {
        self.request_state = request_state;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialOwnership {
    /// Codex owns refresh-token changes for this identity. GSwitch may only
    /// read a live access-token snapshot.
    External,
    /// No external Codex process owns this identity, so an authentication
    /// failure may safely use one isolated managed refresh.
    Inactive,
    /// A Codex process is present but its identity cannot be proved. Wake
    /// remains eligible using its saved token, but no isolated refresh starts.
    Uncertain,
}

struct WakeCredential {
    value: Value,
    ownership: CredentialOwnership,
}

/// Starts a user-triggered one-account Wake operation. The returned ID refers
/// only to in-memory progress for this GSwitch process.
pub fn start_one(state: AppState, account_id: String) -> Result<WakeStart, String> {
    state.ensure_store_ready()?;
    let account = state.account_by_id(&account_id)?;
    start(
        state,
        vec![WakeTarget {
            id: account.id,
            label: account.label,
        }],
    )
}

/// Starts a sequential Wake queue for every saved ChatGPT account. API-key
/// accounts do not participate because they have no subscription window.
pub fn start_all(state: AppState) -> Result<WakeStart, String> {
    state.ensure_store_ready()?;
    let targets = state
        .list()?
        .into_iter()
        .filter(|account| account.kind == AccountKind::ChatGpt)
        .map(|account| WakeTarget {
            id: account.id,
            label: account.label,
        })
        .collect::<Vec<_>>();
    start(state, targets)
}

/// Starts Wake for the current selection. The WebView supplies only saved
/// account IDs; Rust validates the snapshot and still owns the actual queue.
pub fn start_selected(state: AppState, selected_ids: Vec<String>) -> Result<WakeStart, String> {
    state.ensure_store_ready()?;
    let mut seen = std::collections::HashSet::new();
    let selected_ids: Vec<_> = selected_ids
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect();
    if selected_ids.is_empty() {
        return Err("Select at least one ChatGPT account to wake".to_string());
    }

    let accounts = state.list()?;
    let targets = selected_ids
        .iter()
        .map(|id| {
            accounts
                .iter()
                .find(|account| account.id == *id)
                .ok_or_else(|| "The selected account is no longer saved".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|account| account.kind == AccountKind::ChatGpt)
        .map(|account| WakeTarget {
            id: account.id.clone(),
            label: account.label.clone(),
        })
        .collect();
    start(state, targets)
}

pub fn operation(state: &AppState, operation_id: &str) -> Result<WakeOperationView, String> {
    state.wake_operation(operation_id)
}

pub fn cancel(state: &AppState, operation_id: &str) -> Result<(), String> {
    state.cancel_wake(operation_id)
}

fn start(state: AppState, targets: Vec<WakeTarget>) -> Result<WakeStart, String> {
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
        .spawn(move || run_queue(worker_state, worker_id, targets, cancelled))
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
    cancelled: Arc<AtomicBool>,
) {
    let result = state
        .acquire_operation()
        .and_then(|operation| run_targets(&state, &operation, &operation_id, &targets, &cancelled));

    if let Err(message) = result {
        let _ = fail_operation(&state, &operation_id, &targets, message);
    }
}

fn run_targets(
    state: &AppState,
    operation: &OperationGuard<'_>,
    operation_id: &str,
    targets: &[WakeTarget],
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
            Ok(account) => wake_account(state, operation, &account, cancelled),
            Err(error) => WakeAccountOutcome::new(WakeResultKind::Failed, error),
        };
        view.results.push(WakeAccountResult {
            account_id: target.id.clone(),
            label: target.label.clone(),
            result: outcome.result,
            request_state: outcome.request_state,
            message: outcome.message,
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
            request_state: WakeRequestState::NotSent,
            message: "Wake queue was cancelled before this account started".to_string(),
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
            request_state: WakeRequestState::NotSent,
            message: message.clone(),
        });
    }
    state.update_wake_operation(view)
}

fn wake_account(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    cancelled: &AtomicBool,
) -> WakeAccountOutcome {
    if account.kind != AccountKind::ChatGpt {
        return WakeAccountOutcome::new(
            WakeResultKind::Failed,
            "Wake is not available for API-key accounts",
        );
    }
    if cancelled.load(Ordering::SeqCst) {
        return WakeAccountOutcome::new(WakeResultKind::Cancelled, "Wake was cancelled");
    }

    let identity = match verified_chatgpt_identity(account) {
        Ok(identity) => identity,
        Err(error) => return WakeAccountOutcome::new(WakeResultKind::Failed, error),
    };
    let mut credential = wake_credential(account, &identity);
    let mut before = match preflight_quota(state, operation, account, &identity, &mut credential) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    if let Some(outcome) = quota_eligibility(&before) {
        return outcome;
    }
    if cancelled.load(Ordering::SeqCst) {
        return WakeAccountOutcome::new(WakeResultKind::Cancelled, "Wake was cancelled");
    }

    // Codex can rotate its access token while it owns this identity. Re-read
    // once immediately before the one Wake request, then repeat only the safe
    // quota preflight if the snapshot changed.
    if refresh_credential_ownership_before_wake(
        &mut credential,
        external_credential_state_for_identity(&identity),
    ) {
        before = match preflight_quota(state, operation, account, &identity, &mut credential) {
            Ok(snapshot) => snapshot,
            Err(outcome) => return outcome,
        };
        if let Some(outcome) = quota_eligibility(&before) {
            return outcome;
        }
    }
    if cancelled.load(Ordering::SeqCst) {
        return WakeAccountOutcome::new(WakeResultKind::Cancelled, "Wake was cancelled");
    }

    let client = match ChatGptClient::new() {
        Ok(client) => client,
        Err(error) => return WakeAccountOutcome::new(WakeResultKind::Failed, error),
    };
    match client.wake(&credential.value, WAKE_MODEL) {
        Ok(()) => {}
        Err(error) => {
            return match error.kind {
                // A send failure may occur after the provider received the
                // request. Do not retry it or start an auth refresh.
                RequestFailureKind::Transport | RequestFailureKind::InvalidJson => {
                    WakeAccountOutcome::new(
                        WakeResultKind::SentNotConfirmed,
                        "Wake may have reached ChatGPT, but delivery was not confirmed and was not retried",
                    ).request_state(WakeRequestState::MayHaveSent)
                }
                RequestFailureKind::Authentication => WakeAccountOutcome::new(
                    WakeResultKind::NeedsSignIn,
                    "ChatGPT rejected the Wake credential; sign in again before trying another Wake",
                ).request_state(WakeRequestState::Sent),
                RequestFailureKind::RateLimited => WakeAccountOutcome::new(
                    WakeResultKind::NoOrdinaryCapacity,
                    "ChatGPT reported no ordinary Codex capacity; Wake did not use Reserve or reset credits",
                ).request_state(WakeRequestState::Sent),
                RequestFailureKind::Http => WakeAccountOutcome::new(
                    WakeResultKind::RequestRejected,
                    "ChatGPT rejected the Wake request before it could start",
                ).request_state(WakeRequestState::Sent),
            };
        }
    }

    // The request has been sent. A subsequent quota problem is confirmation
    // only, never a reason to send another Wake request.
    let after =
        match refresh_quota_snapshot(state, operation, account, &identity, &credential.value) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                return WakeAccountOutcome::new(
                    WakeResultKind::SentNotConfirmed,
                    "Wake was sent, but the quota response did not confirm a new five-hour window",
                )
                .request_state(WakeRequestState::Sent)
            }
        };
    if window_started(&before, &after, now_unix_ms() / 1000) {
        WakeAccountOutcome::new(WakeResultKind::Started, "The five-hour window is active")
            .request_state(WakeRequestState::Sent)
    } else {
        WakeAccountOutcome::new(
            WakeResultKind::SentNotConfirmed,
            "Wake was sent, but the quota response did not confirm a new five-hour window",
        )
        .request_state(WakeRequestState::Sent)
    }
}

fn wake_credential(account: &StoredAccount, identity: &AccountIdentity) -> WakeCredential {
    wake_credential_from_external_state(
        account.credential.clone(),
        external_credential_state_for_identity(identity),
    )
}

fn wake_credential_from_external_state(
    saved_credential: Value,
    external_state: Result<ExternalCredentialState, String>,
) -> WakeCredential {
    match external_state {
        Ok(ExternalCredentialState::Matching(value)) => WakeCredential {
            value,
            ownership: CredentialOwnership::External,
        },
        Ok(ExternalCredentialState::NotRunning | ExternalCredentialState::DifferentAccount) => {
            WakeCredential {
                value: saved_credential,
                ownership: CredentialOwnership::Inactive,
            }
        }
        Ok(ExternalCredentialState::Unidentifiable) | Err(_) => WakeCredential {
            value: saved_credential,
            ownership: CredentialOwnership::Uncertain,
        },
    }
}

fn refresh_credential_ownership_before_wake(
    credential: &mut WakeCredential,
    external_state: Result<ExternalCredentialState, String>,
) -> bool {
    match external_state {
        Ok(ExternalCredentialState::Matching(latest)) => {
            let changed =
                credential.ownership != CredentialOwnership::External || latest != credential.value;
            credential.value = latest;
            credential.ownership = CredentialOwnership::External;
            changed
        }
        Ok(ExternalCredentialState::Unidentifiable) | Err(_) => {
            credential.ownership = CredentialOwnership::Uncertain;
            false
        }
        Ok(ExternalCredentialState::NotRunning | ExternalCredentialState::DifferentAccount) => {
            false
        }
    }
}

fn preflight_quota(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
    credential: &mut WakeCredential,
) -> Result<QuotaSnapshot, WakeAccountOutcome> {
    match refresh_quota_snapshot(state, operation, account, identity, &credential.value) {
        Ok(snapshot) => Ok(snapshot),
        Err(error) if can_use_managed_refresh(credential.ownership, &error) => {
            let refreshed =
                refresh_via_managed_profile_for_wake(state, operation, account, identity)
                    .map_err(|error| WakeAccountOutcome::new(WakeResultKind::NeedsSignIn, error))?;
            credential.value = refreshed.credential;
            Ok(refreshed.snapshot)
        }
        Err(error) => Err(preflight_failure(error)),
    }
}

fn can_use_managed_refresh(ownership: CredentialOwnership, error: &ReadOnlyRefreshFailure) -> bool {
    ownership == CredentialOwnership::Inactive && error.can_fallback_to_managed_refresh()
}

fn preflight_failure(error: ReadOnlyRefreshFailure) -> WakeAccountOutcome {
    if error.can_fallback_to_managed_refresh() {
        WakeAccountOutcome::new(
            WakeResultKind::NeedsSignIn,
            "ChatGPT rejected the Wake credential; sign in again before trying another Wake",
        )
    } else {
        WakeAccountOutcome::new(WakeResultKind::QuotaUnavailable, error.message())
    }
}

fn quota_eligibility(snapshot: &QuotaSnapshot) -> Option<WakeAccountOutcome> {
    let now_seconds = now_unix_ms() / 1000;
    if five_hour(snapshot).is_none() {
        return Some(WakeAccountOutcome::new(
            WakeResultKind::NoFiveHourWindow,
            "No five-hour quota window was reported; no Wake request was sent",
        ));
    }
    if window_is_active(snapshot, now_seconds) {
        return Some(WakeAccountOutcome::new(
            WakeResultKind::AlreadyActive,
            "The five-hour window is already active",
        ));
    }
    if ordinary_quota_exhausted(snapshot, now_seconds) {
        let exhausted_window = snapshot
            .buckets
            .iter()
            .find(|bucket| bucket.kind == QuotaBucketKind::Codex)
            .and_then(|bucket| {
                bucket
                    .windows
                    .iter()
                    .find(|window| {
                        window.kind == QuotaWindowKind::FiveHour
                            && window.remaining_percent == Some(0)
                            && window.resets_at.is_some_and(|reset| reset > now_seconds)
                    })
                    .or_else(|| {
                        bucket.windows.iter().find(|window| {
                            window.kind == QuotaWindowKind::Weekly
                                && window.remaining_percent == Some(0)
                                && window.resets_at.is_some_and(|reset| reset > now_seconds)
                        })
                    })
            });
        let result = match exhausted_window.map(|window| &window.kind) {
            Some(QuotaWindowKind::FiveHour) => WakeResultKind::FiveHourExhausted,
            Some(QuotaWindowKind::Weekly) => WakeResultKind::WeeklyExhausted,
            _ => WakeResultKind::NoOrdinaryCapacity,
        };
        return Some(WakeAccountOutcome::new(
            result,
            "Codex quota is exhausted; Wake will not use Reserve or reset credits",
        ));
    }
    None
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
    use serde_json::json;

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
    fn keeps_externally_owned_and_inactive_refresh_paths_distinct() {
        let saved = json!({"tokens": {"access_token": "saved"}});
        let live = json!({"tokens": {"access_token": "live"}});

        let active = wake_credential_from_external_state(
            saved.clone(),
            Ok(ExternalCredentialState::Matching(live.clone())),
        );
        assert_eq!(active.value, live);
        assert_eq!(active.ownership, CredentialOwnership::External);

        let inactive = wake_credential_from_external_state(
            saved.clone(),
            Ok(ExternalCredentialState::DifferentAccount),
        );
        assert_eq!(inactive.value, saved);
        assert_eq!(inactive.ownership, CredentialOwnership::Inactive);

        let uncertain = wake_credential_from_external_state(
            json!({"tokens": {"access_token": "saved"}}),
            Ok(ExternalCredentialState::Unidentifiable),
        );
        assert_eq!(uncertain.ownership, CredentialOwnership::Uncertain);
    }

    #[test]
    fn rereads_a_changed_or_newly_active_token_before_wake() {
        let mut credential = WakeCredential {
            value: json!({"tokens": {"access_token": "before"}}),
            ownership: CredentialOwnership::External,
        };
        assert!(refresh_credential_ownership_before_wake(
            &mut credential,
            Ok(ExternalCredentialState::Matching(
                json!({"tokens": {"access_token": "after"}}),
            )),
        ));
        assert_eq!(credential.value["tokens"]["access_token"], "after");
        let unchanged = credential.value.clone();
        assert!(!refresh_credential_ownership_before_wake(
            &mut credential,
            Ok(ExternalCredentialState::Matching(unchanged)),
        ));

        credential.ownership = CredentialOwnership::Inactive;
        let newly_active = credential.value.clone();
        assert!(refresh_credential_ownership_before_wake(
            &mut credential,
            Ok(ExternalCredentialState::Matching(newly_active)),
        ));
        assert_eq!(credential.ownership, CredentialOwnership::External);
    }

    #[test]
    fn only_a_definitely_inactive_auth_failure_can_refresh() {
        let auth_failure = ReadOnlyRefreshFailure::Provider(crate::chatgpt::RequestFailure {
            kind: RequestFailureKind::Authentication,
            status: Some(401),
        });
        assert!(can_use_managed_refresh(
            CredentialOwnership::Inactive,
            &auth_failure
        ));
        assert!(!can_use_managed_refresh(
            CredentialOwnership::External,
            &auth_failure
        ));
        assert!(!can_use_managed_refresh(
            CredentialOwnership::Uncertain,
            &auth_failure
        ));
    }

    #[test]
    fn reports_active_or_exhausted_ordinary_windows_without_sending_wake() {
        let active = quota_eligibility(&snapshot(10, 90, i64::MAX)).expect("active result");
        assert_eq!(active.result, WakeResultKind::AlreadyActive);
        assert_eq!(active.request_state, WakeRequestState::NotSent);
        let exhausted = quota_eligibility(&snapshot(100, 0, i64::MAX)).expect("capacity result");
        assert_eq!(exhausted.result, WakeResultKind::FiveHourExhausted);
        let mut other_plan = snapshot(10, 90, i64::MAX);
        other_plan.buckets[0].windows[0].kind = QuotaWindowKind::Other;
        other_plan.buckets[0].windows[0].window_duration_mins = Some(43_200);
        let unavailable = quota_eligibility(&other_plan).expect("no five-hour window result");
        assert_eq!(unavailable.result, WakeResultKind::NoFiveHourWindow);
        assert_eq!(unavailable.request_state, WakeRequestState::NotSent);
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
