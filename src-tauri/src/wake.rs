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
    chatgpt::{ChatGptClient, WakeFailure, WakeFailureKind},
    quota::{
        external_credential_state_for_identity, refresh_saved_sign_in, verified_chatgpt_identity,
        ExternalCredentialState, ManagedRefreshFailure,
    },
    types::{
        AccountIdentity, AccountKind, StoredAccount, WakeAccountResult, WakeOperationStatus,
        WakeOperationView, WakeRequestState, WakeResultKind, WakeStart,
    },
};

/// Each explicit Wake sends one short text turn. A rejected default model may
/// be retried with the older model only after another user action.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WakeModel {
    Default,
    Alternate,
}

impl WakeModel {
    fn id(self) -> &'static str {
        match self {
            Self::Default => "gpt-6-luna",
            Self::Alternate => "gpt-5.6-luna",
        }
    }

    fn reasoning_effort(self) -> &'static str {
        "low"
    }
}

#[derive(Clone)]
struct WakeTarget {
    id: String,
    label: String,
}

struct WakeAccountOutcome {
    result: WakeResultKind,
    request_state: WakeRequestState,
    http_status: Option<u16>,
    message: String,
    /// The saved credential generation that ChatGPT rejected. Only that
    /// generation is marked as needing sign-in.
    rejected_generation: Option<u64>,
}

impl WakeAccountOutcome {
    fn new(result: WakeResultKind, message: impl Into<String>) -> Self {
        Self {
            result,
            request_state: WakeRequestState::NotSent,
            http_status: None,
            message: message.into(),
            rejected_generation: None,
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

/// The Codex-runtime and provider calls Wake makes. Production uses the local
/// Codex runtime and ChatGPT; tests replay scripted results.
trait WakeTransport {
    fn live_state(&mut self, identity: &AccountIdentity)
        -> Result<ExternalCredentialState, String>;
    fn send(&mut self, credential: &Value, model: WakeModel) -> Result<(), WakeFailure>;
    fn refresh(
        &mut self,
        state: &AppState,
        operation: &OperationGuard<'_>,
        account: &StoredAccount,
        identity: &AccountIdentity,
    ) -> Result<Value, ManagedRefreshFailure>;
}

struct ChatGptWake(ChatGptClient);

impl WakeTransport for ChatGptWake {
    fn live_state(
        &mut self,
        identity: &AccountIdentity,
    ) -> Result<ExternalCredentialState, String> {
        external_credential_state_for_identity(identity)
    }

    fn send(&mut self, credential: &Value, model: WakeModel) -> Result<(), WakeFailure> {
        self.0
            .wake(credential, model.id(), model.reasoning_effort())
    }

    fn refresh(
        &mut self,
        state: &AppState,
        operation: &OperationGuard<'_>,
        account: &StoredAccount,
        identity: &AccountIdentity,
    ) -> Result<Value, ManagedRefreshFailure> {
        refresh_saved_sign_in(state, operation, account, identity)
    }
}

/// Starts a user-triggered one-account Wake operation. The returned ID refers
/// only to in-memory progress for this GSwitch process.
pub fn start_one(
    state: AppState,
    account_id: String,
    alternate_model: bool,
) -> Result<WakeStart, String> {
    state.ensure_store_ready()?;
    let account = state.account_by_id(&account_id)?;
    start(
        state,
        vec![WakeTarget {
            id: account.id,
            label: account.label,
        }],
        if alternate_model {
            WakeModel::Alternate
        } else {
            WakeModel::Default
        },
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
    start(state, targets, WakeModel::Default)
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
    start(state, targets, WakeModel::Default)
}

pub fn operation(state: &AppState, operation_id: &str) -> Result<WakeOperationView, String> {
    state.wake_operation(operation_id)
}

pub fn cancel(state: &AppState, operation_id: &str) -> Result<(), String> {
    state.cancel_wake(operation_id)
}

fn start(state: AppState, targets: Vec<WakeTarget>, model: WakeModel) -> Result<WakeStart, String> {
    if targets.is_empty() {
        return Err("There are no ChatGPT accounts to wake".to_string());
    }

    let operation_id = Uuid::new_v4().to_string();
    let cancelled = Arc::new(AtomicBool::new(false));
    state.insert_wake_operation(
        WakeOperationView {
            id: operation_id.clone(),
            status: WakeOperationStatus::Running,
            alternate_model: model == WakeModel::Alternate,
            current_account_id: None,
            results: Vec::new(),
        },
        cancelled.clone(),
    )?;

    let worker_state = state.clone();
    let worker_id = operation_id.clone();
    if thread::Builder::new()
        .name("gswitch-wake".to_string())
        .spawn(move || run_queue(worker_state, worker_id, targets, cancelled, model))
        .is_err()
    {
        let _ = state.update_wake_operation(WakeOperationView {
            id: operation_id.clone(),
            status: WakeOperationStatus::Failed,
            alternate_model: model == WakeModel::Alternate,
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
    model: WakeModel,
) {
    let result = state
        .ensure_store_ready()
        .and_then(|()| ChatGptClient::new())
        .and_then(|client| {
            run_targets(
                &state,
                &operation_id,
                &targets,
                &cancelled,
                model,
                &mut ChatGptWake(client),
            )
        });

    if let Err(message) = result {
        let _ = fail_operation(&state, &operation_id, &targets, message);
    }
}

/// Wake requests use a credential snapshot and hold no GSwitch lock, so quota
/// refresh, switching, and other GSwitch work continue while they are in
/// flight. Only steps that change a saved account wait for the lock.
fn run_targets(
    state: &AppState,
    operation_id: &str,
    targets: &[WakeTarget],
    cancelled: &AtomicBool,
    model: WakeModel,
    transport: &mut impl WakeTransport,
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
        let outcome = match state.account_by_id(&target.id) {
            Ok(account) => wake_account(state, &account, cancelled, model, transport),
            Err(error) => WakeAccountOutcome::new(WakeResultKind::Failed, error),
        };
        if let Some(generation) = outcome.rejected_generation {
            // A sign-in or refresh saved while the request was in flight is a
            // newer generation and stays usable.
            if let Ok(operation) = state.acquire_wake_operation() {
                let _ = state.mark_sign_in_required(&operation, &target.id, generation);
            }
        }
        view.results.push(WakeAccountResult {
            account_id: target.id.clone(),
            label: target.label.clone(),
            result: outcome.result,
            request_state: outcome.request_state,
            http_status: outcome.http_status,
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
            http_status: None,
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
            http_status: None,
            message: message.clone(),
        });
    }
    state.update_wake_operation(view)
}

fn wake_account(
    state: &AppState,
    account: &StoredAccount,
    cancelled: &AtomicBool,
    model: WakeModel,
    transport: &mut impl WakeTransport,
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
    let mut credential = wake_credential_from_external_state(
        account.credential.clone(),
        transport.live_state(&identity),
    );
    // Codex can rotate an active token. Use one fresh read immediately before
    // the request without making Wake depend on quota availability.
    let _ =
        refresh_credential_ownership_before_wake(&mut credential, transport.live_state(&identity));
    if cancelled.load(Ordering::SeqCst) {
        return WakeAccountOutcome::new(WakeResultKind::Cancelled, "Wake was cancelled");
    }

    let mut sent_generation = account.credential_generation;
    let mut result = transport.send(&credential.value, model);
    // A definite authentication rejection cannot have completed a model turn.
    // Only a definitely inactive account may use one isolated official refresh;
    // an uncertain send or an externally owned token is never retried. The
    // repeated request, not the refresh, decides whether sign-in is needed.
    if result.as_ref().is_err_and(|error| {
        error.kind == WakeFailureKind::Authentication
            && credential.ownership == CredentialOwnership::Inactive
    }) {
        match refresh_inactive_sign_in(state, &account.id, transport) {
            Ok(Some((refreshed, generation))) => {
                if cancelled.load(Ordering::SeqCst) {
                    return WakeAccountOutcome::new(
                        WakeResultKind::Cancelled,
                        "Wake was cancelled",
                    );
                }
                sent_generation = generation;
                result = transport.send(&refreshed, model);
            }
            // Codex now owns this identity or cannot be identified, so the
            // first rejection stands without a refresh.
            Ok(None) => {}
            Err(outcome) => {
                let request_state = result
                    .as_ref()
                    .err()
                    .map_or(WakeRequestState::NotSent, |failure| failure.request_state);
                return outcome.request_state(request_state);
            }
        }
    }
    match result {
        Ok(()) => WakeAccountOutcome::new(
            WakeResultKind::ReplyReceived,
            "Codex replied to the Wake request",
        )
        .request_state(WakeRequestState::Sent),
        Err(error) => {
            let mut outcome = wake_failure(error);
            if outcome.result == WakeResultKind::NeedsSignIn {
                outcome.rejected_generation = Some(sent_generation);
            }
            outcome
        }
    }
}

/// Runs the one isolated refresh an inactive account may use after an
/// authentication failure. It waits for the credential-operation lock, then
/// rereads the saved account and its live ownership, because a switch,
/// sign-in, or Codex launch may have changed either while the request was in
/// flight. Returns the refreshed credential and its saved generation, or None
/// when a running Codex now owns or hides the identity.
fn refresh_inactive_sign_in(
    state: &AppState,
    account_id: &str,
    transport: &mut impl WakeTransport,
) -> Result<Option<(Value, u64)>, WakeAccountOutcome> {
    let failed = |message: String| WakeAccountOutcome::new(WakeResultKind::Failed, message);
    let operation = state.acquire_wake_operation().map_err(failed)?;
    let account = state
        .account_by_id_under_operation(&operation, account_id)
        .map_err(failed)?;
    let identity = verified_chatgpt_identity(&account).map_err(failed)?;
    if !matches!(
        transport.live_state(&identity),
        Ok(ExternalCredentialState::NotRunning | ExternalCredentialState::DifferentAccount)
    ) {
        return Ok(None);
    }
    match transport.refresh(state, &operation, &account, &identity) {
        Ok(refreshed) => {
            let generation = state
                .account_by_id_under_operation(&operation, account_id)
                .map_err(failed)?
                .credential_generation;
            Ok(Some((refreshed, generation)))
        }
        Err(failure) => {
            let mut outcome =
                WakeAccountOutcome::new(managed_refresh_result(&failure), failure.message());
            if outcome.result == WakeResultKind::NeedsSignIn {
                outcome.rejected_generation = Some(account.credential_generation);
            }
            Err(outcome)
        }
    }
}

fn managed_refresh_result(failure: &ManagedRefreshFailure) -> WakeResultKind {
    match failure {
        ManagedRefreshFailure::IdentityChanged
        | ManagedRefreshFailure::NotSaved {
            recovery_retained: false,
        } => WakeResultKind::NeedsSignIn,
        ManagedRefreshFailure::Unavailable(_)
        | ManagedRefreshFailure::NotSaved {
            recovery_retained: true,
        } => WakeResultKind::Failed,
    }
}

fn wake_failure(failure: WakeFailure) -> WakeAccountOutcome {
    let (result, message) = match failure.kind {
        WakeFailureKind::Transport | WakeFailureKind::InvalidResponse => (
            WakeResultKind::SentNotConfirmed,
            "A Wake request may have reached ChatGPT, but no complete model reply was confirmed",
        ),
        WakeFailureKind::Authentication => (
            WakeResultKind::NeedsSignIn,
            "ChatGPT rejected this account credential; sign in again",
        ),
        WakeFailureKind::RateLimited => (
            WakeResultKind::RateLimited,
            "ChatGPT rate-limited or declined capacity for this Wake request",
        ),
        WakeFailureKind::ModelUnavailable => (
            WakeResultKind::ModelUnavailable,
            "The selected model is unavailable for this account",
        ),
        WakeFailureKind::InvalidRequest => (
            WakeResultKind::InvalidRequest,
            "Codex did not accept the Wake request parameters",
        ),
        WakeFailureKind::ServiceUnavailable => (
            WakeResultKind::ServiceUnavailable,
            "The Codex service could not complete the Wake request",
        ),
        WakeFailureKind::Rejected => (
            WakeResultKind::RequestRejected,
            "ChatGPT rejected the Wake request",
        ),
    };
    let mut outcome = WakeAccountOutcome::new(result, message).request_state(failure.request_state);
    outcome.http_status = failure.status;
    outcome
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
    fn an_unavailable_refresh_is_not_reported_as_a_rejected_sign_in() {
        assert_eq!(
            managed_refresh_result(&ManagedRefreshFailure::Unavailable(
                "Unable to start Codex App Server. Check that Codex is installed.".into(),
            )),
            WakeResultKind::Failed
        );
        assert_eq!(
            managed_refresh_result(&ManagedRefreshFailure::NotSaved {
                recovery_retained: true,
            }),
            WakeResultKind::Failed
        );
        assert_eq!(
            managed_refresh_result(&ManagedRefreshFailure::IdentityChanged),
            WakeResultKind::NeedsSignIn
        );
        assert_eq!(
            managed_refresh_result(&ManagedRefreshFailure::NotSaved {
                recovery_retained: false,
            }),
            WakeResultKind::NeedsSignIn
        );
    }

    #[test]
    fn request_failures_preserve_whether_a_request_was_sent() {
        let uncertain = wake_failure(WakeFailure {
            kind: WakeFailureKind::Transport,
            request_state: WakeRequestState::MayHaveSent,
            status: None,
        });
        assert_eq!(uncertain.result, WakeResultKind::SentNotConfirmed);
        assert_eq!(uncertain.request_state, WakeRequestState::MayHaveSent);

        let rejected = wake_failure(WakeFailure {
            kind: WakeFailureKind::RateLimited,
            request_state: WakeRequestState::Sent,
            status: Some(429),
        });
        assert_eq!(rejected.result, WakeResultKind::RateLimited);
        assert_eq!(rejected.request_state, WakeRequestState::Sent);
        assert_eq!(rejected.http_status, Some(429));

        let local = wake_failure(WakeFailure {
            kind: WakeFailureKind::Authentication,
            request_state: WakeRequestState::NotSent,
            status: None,
        });
        assert_eq!(local.result, WakeResultKind::NeedsSignIn);
        assert_eq!(local.request_state, WakeRequestState::NotSent);
    }

    type ScriptedSend<'a> = Box<dyn FnMut(&Value) -> Result<(), WakeFailure> + 'a>;

    /// Replays the Codex runtime and ChatGPT for one account. `live` answers
    /// each ownership check in turn and repeats its last answer.
    struct ScriptedWake<'a> {
        live: Vec<ExternalCredentialState>,
        send: ScriptedSend<'a>,
        refreshed: Option<Value>,
        refreshes: usize,
    }

    impl WakeTransport for ScriptedWake<'_> {
        fn live_state(
            &mut self,
            _identity: &AccountIdentity,
        ) -> Result<ExternalCredentialState, String> {
            Ok(if self.live.len() > 1 {
                self.live.remove(0)
            } else {
                self.live[0].clone()
            })
        }

        fn send(&mut self, credential: &Value, _model: WakeModel) -> Result<(), WakeFailure> {
            (self.send)(credential)
        }

        fn refresh(
            &mut self,
            state: &AppState,
            operation: &OperationGuard<'_>,
            account: &StoredAccount,
            identity: &AccountIdentity,
        ) -> Result<Value, ManagedRefreshFailure> {
            self.refreshes += 1;
            let refreshed = self.refreshed.clone().expect("unexpected refresh");
            crate::quota::keep_refreshed_credential(state, operation, account, identity, refreshed)
        }
    }

    fn rejected() -> Result<(), WakeFailure> {
        Err(WakeFailure {
            kind: WakeFailureKind::Authentication,
            request_state: WakeRequestState::Sent,
            status: Some(401),
        })
    }

    fn credential(access_token: &str) -> Value {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "https://api.openai.com/auth": {
                    "chatgpt_user_id": "user",
                    "chatgpt_account_id": "workspace",
                }
            }))
            .expect("claims"),
        );
        json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": access_token,
                "refresh_token": "refresh-token",
                "id_token": format!("header.{claims}.signature"),
            },
        })
    }

    fn save(state: &AppState, access_token: &str) -> String {
        let operation = state.acquire_operation().expect("operation");
        state
            .upsert_under_operation(
                &operation,
                crate::accounts::AccountDraft {
                    label: None,
                    default_label: "user@example.com".into(),
                    kind: AccountKind::ChatGpt,
                    email: Some("user@example.com".into()),
                    plan_type: Some("plus".into()),
                    workspace_name: Some("workspace".into()),
                    account_structure: Some("workspace".into()),
                    identity: AccountIdentity::ChatGpt {
                        user_id: "user".into(),
                        workspace_id: Some("workspace".into()),
                    },
                    credential: credential(access_token),
                },
            )
            .expect("save account")
            .id
    }

    fn temp_state(name: &str) -> (AppState, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("gswitch-wake-{name}-{}", Uuid::new_v4()));
        let state = AppState::new(root.join("accounts.json")).expect("state");
        (state, root)
    }

    /// Runs one Wake operation for the account and returns its result.
    fn wake(
        state: &AppState,
        account_id: &str,
        transport: &mut ScriptedWake<'_>,
    ) -> WakeResultKind {
        let id = Uuid::new_v4().to_string();
        let cancelled = Arc::new(AtomicBool::new(false));
        state
            .insert_wake_operation(
                WakeOperationView {
                    id: id.clone(),
                    status: WakeOperationStatus::Running,
                    alternate_model: false,
                    current_account_id: None,
                    results: Vec::new(),
                },
                cancelled.clone(),
            )
            .expect("operation");
        let target = WakeTarget {
            id: account_id.into(),
            label: "user@example.com".into(),
        };
        run_targets(
            state,
            &id,
            &[target],
            &cancelled,
            WakeModel::Default,
            transport,
        )
        .expect("run Wake");
        state
            .wake_operation(&id)
            .expect("result")
            .results
            .remove(0)
            .result
    }

    #[test]
    fn wake_requests_run_while_other_work_holds_the_credential_lock() {
        let (state, root) = temp_state("lock-free");
        let account_id = save(&state, "saved");

        // A quota commit or switch already holds the lock: Wake still runs.
        let other_work = state.acquire_operation().expect("other work");
        let mut transport = ScriptedWake {
            live: vec![ExternalCredentialState::NotRunning],
            send: Box::new(|_| Ok(())),
            refreshed: None,
            refreshes: 0,
        };
        assert_eq!(
            wake(&state, &account_id, &mut transport),
            WakeResultKind::ReplyReceived
        );
        drop(other_work);

        // While a request is in flight, other work can take the lock.
        let mut transport = ScriptedWake {
            live: vec![ExternalCredentialState::NotRunning],
            send: Box::new(|_| {
                assert!(state.acquire_operation().is_ok());
                Ok(())
            }),
            refreshed: None,
            refreshes: 0,
        };
        assert_eq!(
            wake(&state, &account_id, &mut transport),
            WakeResultKind::ReplyReceived
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_rejection_marks_sign_in_only_for_the_credential_that_was_sent() {
        let (state, root) = temp_state("rejected-generation");
        let account_id = save(&state, "saved");
        let mut transport = ScriptedWake {
            live: vec![ExternalCredentialState::Unidentifiable],
            send: Box::new(|_| {
                // GSwitch saves a token Codex rotated while the request is out.
                let operation = state.acquire_operation().expect("operation");
                let generation = state
                    .account_by_id(&account_id)
                    .expect("account")
                    .credential_generation;
                state
                    .update_credential_under_operation(
                        &operation,
                        &account_id,
                        credential("rotated"),
                        generation,
                    )
                    .expect("save rotated token");
                rejected()
            }),
            refreshed: None,
            refreshes: 0,
        };
        assert_eq!(
            wake(&state, &account_id, &mut transport),
            WakeResultKind::NeedsSignIn
        );
        assert!(
            !state
                .account_by_id(&account_id)
                .expect("account")
                .sign_in_required
        );
        let _ = std::fs::remove_dir_all(root);

        let (state, root) = temp_state("rejected-unchanged");
        let account_id = save(&state, "saved");
        let mut transport = ScriptedWake {
            live: vec![ExternalCredentialState::Unidentifiable],
            send: Box::new(|_| rejected()),
            refreshed: None,
            refreshes: 0,
        };
        assert_eq!(
            wake(&state, &account_id, &mut transport),
            WakeResultKind::NeedsSignIn
        );
        assert!(
            state
                .account_by_id(&account_id)
                .expect("account")
                .sign_in_required
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_isolated_refresh_rechecks_live_ownership_under_the_lock() {
        let (state, root) = temp_state("refresh-recheck");
        let account_id = save(&state, "saved");

        // Codex starts with this identity while the first request is out.
        let mut sends = 0;
        let mut transport = ScriptedWake {
            live: vec![
                ExternalCredentialState::NotRunning,
                ExternalCredentialState::NotRunning,
                ExternalCredentialState::Matching(credential("live")),
            ],
            send: Box::new(|_| {
                sends += 1;
                rejected()
            }),
            refreshed: Some(credential("refreshed")),
            refreshes: 0,
        };
        assert_eq!(
            wake(&state, &account_id, &mut transport),
            WakeResultKind::NeedsSignIn
        );
        assert_eq!(transport.refreshes, 0);
        drop(transport);
        assert_eq!(sends, 1);

        // A still-inactive account refreshes once and resends with the
        // credential the refresh saved.
        let mut sent = Vec::new();
        let mut transport = ScriptedWake {
            live: vec![ExternalCredentialState::NotRunning],
            send: Box::new(|credential| {
                sent.push(credential["tokens"]["access_token"].clone());
                if sent.len() == 1 {
                    rejected()
                } else {
                    Ok(())
                }
            }),
            refreshed: Some(credential("refreshed")),
            refreshes: 0,
        };
        assert_eq!(
            wake(&state, &account_id, &mut transport),
            WakeResultKind::ReplyReceived
        );
        assert_eq!(transport.refreshes, 1);
        drop(transport);
        assert_eq!(sent, vec![json!("saved"), json!("refreshed")]);
        assert_eq!(
            state
                .account_by_id(&account_id)
                .expect("account")
                .credential,
            credential("refreshed")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
