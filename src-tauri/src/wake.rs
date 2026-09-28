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
        external_credential_state_for_identity, refresh_via_managed_profile_for_wake,
        verified_chatgpt_identity, ExternalCredentialState,
    },
    types::{
        AccountIdentity, AccountKind, StoredAccount, WakeAccountResult, WakeOperationStatus,
        WakeOperationView, WakeRequestState, WakeResultKind, WakeStart,
    },
};

/// Use one short text turn on the standard tier. No model fallback is allowed
/// after a request may have reached the provider.
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
/// accounts use a different credential route and do not participate.
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
    // Codex can rotate an active token. Use one fresh read immediately before
    // the request without making Wake depend on quota availability.
    let _ = refresh_credential_ownership_before_wake(
        &mut credential,
        external_credential_state_for_identity(&identity),
    );
    if cancelled.load(Ordering::SeqCst) {
        return WakeAccountOutcome::new(WakeResultKind::Cancelled, "Wake was cancelled");
    }

    let client = match ChatGptClient::new() {
        Ok(client) => client,
        Err(error) => return WakeAccountOutcome::new(WakeResultKind::Failed, error),
    };
    let mut result = client.wake(&credential.value, WAKE_MODEL);
    // A definite authentication rejection cannot have completed a model turn.
    // Only a definitely inactive account may use one isolated official refresh;
    // an uncertain send or an externally owned token is never retried.
    if result.as_ref().is_err_and(|error| {
        error.kind == RequestFailureKind::Authentication
            && credential.ownership == CredentialOwnership::Inactive
    }) {
        let refreshed =
            match refresh_via_managed_profile_for_wake(state, operation, account, &identity) {
                Ok(refreshed) => refreshed,
                Err(error) => {
                    let request_state = result
                        .as_ref()
                        .err()
                        .map_or(WakeRequestState::NotSent, |failure| failure.request_state);
                    return WakeAccountOutcome::new(WakeResultKind::NeedsSignIn, error)
                        .request_state(request_state);
                }
            };
        credential.value = refreshed.credential;
        if cancelled.load(Ordering::SeqCst) {
            return WakeAccountOutcome::new(WakeResultKind::Cancelled, "Wake was cancelled");
        }
        result = client.wake(&credential.value, WAKE_MODEL);
    }
    match result {
        Ok(()) => WakeAccountOutcome::new(
            WakeResultKind::ReplyReceived,
            "Codex replied to the Wake request",
        )
        .request_state(WakeRequestState::Sent),
        Err(error) => wake_failure(error.kind, error.request_state),
    }
}

fn wake_failure(kind: RequestFailureKind, request_state: WakeRequestState) -> WakeAccountOutcome {
    let (result, message) = match kind {
        RequestFailureKind::Transport | RequestFailureKind::InvalidJson => (
            WakeResultKind::SentNotConfirmed,
            "A Wake request may have reached ChatGPT, but no complete model reply was confirmed",
        ),
        RequestFailureKind::Authentication => (
            WakeResultKind::NeedsSignIn,
            "ChatGPT rejected this account credential; sign in again",
        ),
        RequestFailureKind::RateLimited => (
            WakeResultKind::RateLimited,
            "ChatGPT rate-limited or declined capacity for this Wake request",
        ),
        RequestFailureKind::Http => (
            WakeResultKind::RequestRejected,
            "ChatGPT rejected the Wake request",
        ),
    };
    WakeAccountOutcome::new(result, message).request_state(request_state)
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
    fn request_failures_preserve_whether_a_request_was_sent() {
        let uncertain = wake_failure(RequestFailureKind::Transport, WakeRequestState::MayHaveSent);
        assert_eq!(uncertain.result, WakeResultKind::SentNotConfirmed);
        assert_eq!(uncertain.request_state, WakeRequestState::MayHaveSent);

        let rejected = wake_failure(RequestFailureKind::RateLimited, WakeRequestState::Sent);
        assert_eq!(rejected.result, WakeResultKind::RateLimited);
        assert_eq!(rejected.request_state, WakeRequestState::Sent);

        let local = wake_failure(
            RequestFailureKind::Authentication,
            WakeRequestState::NotSent,
        );
        assert_eq!(local.result, WakeResultKind::NeedsSignIn);
        assert_eq!(local.request_state, WakeRequestState::NotSent);
    }
}
