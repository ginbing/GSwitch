use std::time::{SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    accounts::{AppState, OperationGuard},
    app_server::{AppServer, TempCodexHome, REJECTED_REQUEST},
    chatgpt::{self, ChatGptClient, RequestFailure, RequestFailureKind},
    codex,
    identity::{derive_identity, document_kind},
    runtime,
    types::{
        AccountIdentity, AccountKind, PendingResetCredit, QuotaBucket, QuotaBucketKind,
        QuotaRefreshFailure, QuotaRefreshFailureCode, QuotaSnapshot, QuotaStatus, QuotaView,
        QuotaWindow, QuotaWindowKind, ResetCreditDetailView, ResetCreditFailure,
        ResetCreditFailureCode, ResetCreditOutcome, ResetCreditOutcomeKind, ResetCreditsView,
        StoredAccount, StoredResetCredit, StoredResetCredits,
    },
};

/// The externally running Codex process is the refresh-token owner for a
/// matching account. A Wake may still use a token snapshot in every state,
/// but only a definitely inactive account may use an isolated refresh.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ExternalCredentialState {
    NotRunning,
    Matching(Value),
    DifferentAccount,
    Unidentifiable,
}

const CACHE_FRESH_FOR_MS: i64 = 5 * 60 * 1000;

/// Only a stable, non-secret reason crosses the Rust/WebView boundary. These
/// messages originate in GSwitch or the sanitized App Server adapter, never
/// in a provider response body.
pub fn refresh_failure(error: &str) -> QuotaRefreshFailure {
    let code = if error.contains("Another GSwitch operation") {
        QuotaRefreshFailureCode::OperationBusy
    } else if error.contains("cannot safely identify its active account") {
        QuotaRefreshFailureCode::CodexAccountUnknown
    } else if error.contains("Refresh this account to retry its saved sign-in") {
        QuotaRefreshFailureCode::ManualRefreshNeeded
    } else if error.contains("rejected the read-only quota request") {
        QuotaRefreshFailureCode::Authentication
    } else if error.contains("rate-limited the quota request") {
        QuotaRefreshFailureCode::RateLimited
    } else if error.contains("Unable to reach ChatGPT quota service") {
        QuotaRefreshFailureCode::Network
    } else if error.contains("quota service returned an error") {
        QuotaRefreshFailureCode::Service
    } else if error.contains("invalid quota response")
        || error.contains("response is missing a result")
    {
        QuotaRefreshFailureCode::InvalidResponse
    } else if error.contains("identity") || error.contains("saved account needs to be added again")
    {
        QuotaRefreshFailureCode::IdentityMismatch
    } else {
        QuotaRefreshFailureCode::Unavailable
    };
    QuotaRefreshFailure { code }
}

/// Returns the last provider snapshot without initiating a provider request.
pub fn cached_quota(state: &AppState, account_id: &str) -> Result<QuotaView, String> {
    let account = state.account_by_id(account_id)?;
    Ok(cached_view(&account, now_unix_ms()))
}

/// Reads capacity from ChatGPT's read-only usage endpoint. Only a rejected,
/// inactive saved sign-in may use one isolated official refresh, after which
/// the same read is repeated; `account/read` alone is deliberately not used
/// as proof that a credential reaches the provider.
pub fn refresh_quota(state: &AppState, account_id: &str) -> Result<QuotaView, String> {
    let operation = state.acquire_operation()?;
    let account = state.account_by_id_under_operation(&operation, account_id)?;
    if account.kind == AccountKind::ApiKey {
        return Ok(not_applicable(&account));
    }
    let identity = verified_chatgpt_identity(&account)?;
    match external_credential_state_for_identity(&identity)? {
        ExternalCredentialState::Matching(credential) => {
            return refresh_active_read_only(state, &operation, &account, &identity, credential)
        }
        ExternalCredentialState::Unidentifiable => {
            return Err(
                "Codex is running and GSwitch cannot safely identify its active account"
                    .to_string(),
            )
        }
        ExternalCredentialState::NotRunning | ExternalCredentialState::DifferentAccount => {}
    }

    match refresh_read_only(state, &operation, &account, &identity, &account.credential) {
        Ok(view) => Ok(view),
        Err(ReadOnlyRefreshFailure::Provider(error)) if error.can_fallback_to_managed_refresh() => {
            let refreshed = refresh_saved_sign_in(state, &operation, &account, &identity)
                .map_err(ManagedRefreshFailure::message)?;
            refresh_read_only(state, &operation, &account, &identity, &refreshed)
                .map_err(ReadOnlyRefreshFailure::message)
        }
        Err(error) => Err(error.message()),
    }
}

/// Automatic refresh only reads a credential snapshot and provider quota.
/// Network latency cannot hold the credential-operation lock or start a token
/// refresh. A later short commit rechecks the saved credential generation.
pub fn refresh_quota_background(state: &AppState, account_id: &str) -> Result<QuotaView, String> {
    state.ensure_store_ready()?;
    let account = state.account_by_id(account_id)?;
    if account.kind == AccountKind::ApiKey {
        return Ok(not_applicable(&account));
    }
    let identity = verified_chatgpt_identity(&account)?;
    let (credential, matching_live) = match external_credential_state_for_identity(&identity)? {
        ExternalCredentialState::Matching(credential) => (credential, true),
        ExternalCredentialState::Unidentifiable => {
            return Err(
                "Codex is running and GSwitch cannot safely identify its active account"
                    .to_string(),
            );
        }
        ExternalCredentialState::NotRunning | ExternalCredentialState::DifferentAccount => {
            (account.credential.clone(), false)
        }
    };
    let saved_credits = account.reset_credits.as_ref();
    let normalized = match fetch_quota_projection(&identity, &credential, saved_credits) {
        Ok(normalized) => normalized,
        Err(ReadOnlyRefreshFailure::Provider(error))
            if matching_live && error.can_fallback_to_managed_refresh() =>
        {
            let latest = live_credential_for_identity(&identity)?.ok_or_else(|| {
                "Codex is running and GSwitch could not reread its active account credential"
                    .to_string()
            })?;
            if latest == credential {
                return Err(ReadOnlyRefreshFailure::Provider(error).message());
            }
            fetch_quota_projection(&identity, &latest, saved_credits)
                .map_err(ReadOnlyRefreshFailure::message)?
        }
        Err(ReadOnlyRefreshFailure::Provider(error)) if error.can_fallback_to_managed_refresh() => {
            return Err("Refresh this account to retry its saved sign-in".to_string());
        }
        Err(error) => return Err(error.message()),
    };

    let operation = state.acquire_quota_commit_operation()?;
    let current = state.account_by_id_under_operation(&operation, account_id)?;
    if current.kind != account.kind
        || current.identity != account.identity
        || current.credential_generation != account.credential_generation
    {
        return Err("The saved account changed while quota was refreshing".to_string());
    }
    if current
        .quota
        .as_ref()
        .is_some_and(|saved| saved.fetched_at_unix_ms >= normalized.snapshot.fetched_at_unix_ms)
    {
        return Ok(cached_view(&current, now_unix_ms()));
    }
    let snapshot = normalized.snapshot;
    state.update_quota_under_operation(
        &operation,
        account_id,
        snapshot.clone(),
        normalized.reset_credits,
    )?;
    Ok(view_from_snapshot(account_id, snapshot, now_unix_ms()))
}

fn refresh_active_read_only(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
    initial_credential: Value,
) -> Result<QuotaView, String> {
    match refresh_read_only(state, operation, account, identity, &initial_credential) {
        Ok(view) => Ok(view),
        Err(ReadOnlyRefreshFailure::Provider(error)) if error.can_fallback_to_managed_refresh() => {
            let latest = live_credential_for_identity(identity)?.ok_or_else(|| {
                "Codex is running and GSwitch could not reread its active account credential"
                    .to_string()
            })?;
            if latest == initial_credential {
                return Err(ReadOnlyRefreshFailure::Provider(error).message());
            }
            match refresh_read_only(state, operation, account, identity, &latest) {
                Ok(view) => Ok(view),
                Err(error) => Err(error.message()),
            }
        }
        Err(error) => Err(error.message()),
    }
}

fn refresh_read_only(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
    credential: &Value,
) -> Result<QuotaView, ReadOnlyRefreshFailure> {
    let snapshot = refresh_quota_snapshot(state, operation, account, identity, credential)?;
    Ok(view_from_snapshot(&account.id, snapshot, now_unix_ms()))
}

/// Refreshes only the account's quota projection from a supplied access-token
/// snapshot. It never updates credentials and is safe for an externally owned
/// active account.
pub(crate) fn refresh_quota_snapshot(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
    credential: &Value,
) -> Result<QuotaSnapshot, ReadOnlyRefreshFailure> {
    let normalized = fetch_quota_projection(identity, credential, account.reset_credits.as_ref())?;
    let snapshot = normalized.snapshot;
    state
        .update_quota_under_operation(
            operation,
            &account.id,
            snapshot.clone(),
            normalized.reset_credits,
        )
        .map_err(ReadOnlyRefreshFailure::Message)?;
    Ok(snapshot)
}

/// Credit details change only when a credit is granted, used, or expires.
/// The usage response's count reveals the first two, and saved lists are
/// filtered for expiry when read, so a matching count within this window lets
/// a refresh skip the separate detail request.
const CREDIT_DETAILS_FRESH_FOR_MS: i64 = 60 * 60 * 1000;

fn fetch_quota_projection(
    identity: &AccountIdentity,
    credential: &Value,
    saved: Option<&StoredResetCredits>,
) -> Result<NormalizedRateLimits, ReadOnlyRefreshFailure> {
    let client = ChatGptClient::new().map_err(ReadOnlyRefreshFailure::Message)?;
    let usage = client
        .usage(credential)
        .map_err(ReadOnlyRefreshFailure::Provider)?;
    chatgpt::validate_response_identity(credential, identity, &usage)
        .map_err(ReadOnlyRefreshFailure::Message)?;

    let now = now_unix_ms();
    if let Some(saved) = reusable_credit_details(saved, &usage, now) {
        return Ok(with_saved_credit_details(
            normalize_rate_limits_data(&usage, now),
            saved,
        ));
    }

    let details = client.reset_credit_details(credential);
    let merged = chatgpt::merge_reset_credit_details(&usage, details.as_ref().ok());
    let mut normalized = normalize_rate_limits_data(&merged, now);
    if let Some(reset_credits) = normalized.reset_credits.as_mut() {
        if details.is_err() {
            reset_credits.credits = None;
            normalized.snapshot.reset_credits = Some(reset_credits_view(reset_credits, now / 1000));
        } else if reset_credits.credits.is_some() {
            reset_credits.details_read_at_unix_ms = Some(now);
        }
    }
    Ok(normalized)
}

/// The saved list still describes the account when it was read recently and
/// its count matches the count in this usage response. A missing count never
/// matches.
fn reusable_credit_details<'a>(
    saved: Option<&'a StoredResetCredits>,
    usage: &Value,
    now_unix_ms: i64,
) -> Option<&'a StoredResetCredits> {
    let saved = saved?;
    let read_at = saved.details_read_at_unix_ms?;
    let usage_count = usage
        .get("rate_limit_reset_credits")
        .or_else(|| usage.get("rateLimitResetCredits"))
        .and_then(|summary| {
            summary
                .get("available_count")
                .or_else(|| summary.get("availableCount"))
        })
        .and_then(nonnegative_u64_at)?;
    (saved.credits.is_some()
        && saved.available_count == usage_count
        && read_at <= now_unix_ms
        && now_unix_ms - read_at < CREDIT_DETAILS_FRESH_FOR_MS)
        .then_some(saved)
}

fn with_saved_credit_details(
    mut normalized: NormalizedRateLimits,
    saved: &StoredResetCredits,
) -> NormalizedRateLimits {
    let now_seconds = normalized.snapshot.fetched_at_unix_ms / 1000;
    let reset_credits = normalized.reset_credits.get_or_insert(StoredResetCredits {
        available_count: saved.available_count,
        credits: None,
        details_read_at_unix_ms: None,
    });
    reset_credits.credits = saved.credits.clone();
    reset_credits.details_read_at_unix_ms = saved.details_read_at_unix_ms;
    normalized.snapshot.reset_credits = Some(reset_credits_view(reset_credits, now_seconds));
    normalized
}

pub(crate) enum ReadOnlyRefreshFailure {
    Provider(RequestFailure),
    Message(String),
}

impl ReadOnlyRefreshFailure {
    pub(crate) fn message(self) -> String {
        match self {
            Self::Message(message) => message,
            Self::Provider(error) => match error.kind {
                RequestFailureKind::Authentication => {
                    "ChatGPT rejected the read-only quota request".to_string()
                }
                RequestFailureKind::RateLimited => {
                    "ChatGPT rate-limited the quota request; try again later".to_string()
                }
                RequestFailureKind::Http => "ChatGPT quota service returned an error".to_string(),
                RequestFailureKind::Transport => {
                    "Unable to reach ChatGPT quota service".to_string()
                }
                RequestFailureKind::InvalidJson => {
                    "ChatGPT returned an invalid quota response".to_string()
                }
            },
        }
    }
}

/// Why one isolated official refresh of an inactive saved sign-in produced no
/// credential to retry with. None of these says ChatGPT rejected the sign-in.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ManagedRefreshFailure {
    /// Codex could not run the refresh, or its result could not be read.
    Unavailable(String),
    /// Codex returned a credential for another account; nothing was saved.
    IdentityChanged,
    /// Codex rotated the credential but GSwitch could not commit it.
    NotSaved { recovery_retained: bool },
}

impl ManagedRefreshFailure {
    pub(crate) fn message(self) -> String {
        match self {
            Self::Unavailable(message) => message,
            Self::IdentityChanged => {
                "Codex did not confirm the refreshed account identity".to_string()
            }
            Self::NotSaved {
                recovery_retained: true,
            } => "GSwitch could not save refreshed credentials. A protected recovery copy was retained."
                .to_string(),
            Self::NotSaved {
                recovery_retained: false,
            } => "GSwitch could not save refreshed credentials or write protected recovery. The temporary profile was removed."
                .to_string(),
        }
    }
}

/// Runs one official token refresh for a saved ChatGPT sign-in in an isolated
/// profile and returns the credential to retry with. Callers must first
/// establish that no external Codex process owns this identity.
///
/// Codex does not report whether the refresh was rejected, so this never
/// decides that the account needs sign-in. The caller repeats its own provider
/// request with the returned credential; an authentication failure there is
/// the confirmed rejection.
pub(crate) fn refresh_saved_sign_in(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
) -> Result<Value, ManagedRefreshFailure> {
    let unavailable = ManagedRefreshFailure::Unavailable;
    let temporary = TempCodexHome::create(&state.isolated_profile_root().map_err(unavailable)?)
        .map_err(unavailable)?;
    temporary
        .write_auth(&account.credential)
        .map_err(unavailable)?;
    let mut server = AppServer::start(&temporary.path).map_err(unavailable)?;
    let attempt = server.account_refresh(1);

    // Codex may have rotated the token chain even when its reply never
    // arrived, so keep a newer credential before reporting that failure.
    let refreshed = temporary.read_auth().map_err(unavailable)?;
    let credential = keep_refreshed_credential(state, operation, account, identity, refreshed)?;
    attempt.map_err(unavailable)?;
    Ok(credential)
}

/// Commits a credential Codex rotated for the same identity before anything
/// else can fail, so a consumed refresh token never stays saved.
pub(crate) fn keep_refreshed_credential(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
    refreshed: Value,
) -> Result<Value, ManagedRefreshFailure> {
    let same_identity = document_kind(&refreshed).is_ok_and(|kind| kind == AccountKind::ChatGpt)
        && derive_identity(&AccountKind::ChatGpt, &refreshed)
            .is_ok_and(|derived| &derived == identity);
    if !same_identity {
        return Err(ManagedRefreshFailure::IdentityChanged);
    }
    if refreshed == account.credential
        || state
            .update_refreshed_credential_under_operation(operation, &account.id, refreshed.clone())
            .is_ok()
    {
        return Ok(refreshed);
    }
    Err(ManagedRefreshFailure::NotSaved {
        recovery_retained: state
            .record_pending_credential(operation, &refreshed)
            .is_ok(),
    })
}

/// Redeems the reset credit the user picked through the official App Server.
/// The durable idempotency record is written before the provider call so an
/// interrupted request can only ever be retried with the same credit and key.
pub fn redeem_reset_credit(
    state: &AppState,
    account_id: &str,
    choice: ResetCreditChoice,
) -> Result<ResetCreditOutcome, ResetCreditFailure> {
    let operation = state
        .acquire_operation()
        .map_err(|error| failure_before_consume(&error))?;
    redeem_under_operation(state, &operation, account_id, Redemption::Pick(choice))
}

/// The non-secret identity of the credit a user picked. The WebView never
/// holds the provider's credit ID, so the pick is matched again after a fresh
/// provider read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResetCreditChoice {
    pub expires_at: Option<i64>,
    pub granted_at: Option<i64>,
}

/// Replays a pending request with its original credit and idempotency key.
/// The record is read and replayed under one operation lock, so recovery can
/// never fall through to selecting a new credit.
pub fn recover_pending_reset_credit(
    state: &AppState,
) -> Result<ResetCreditOutcome, ResetCreditFailure> {
    let operation = state
        .acquire_operation()
        .map_err(|error| failure_before_consume(&error))?;
    let account_id = state
        .pending_reset_credit_under_operation(&operation)
        .and_then(|pending| pending.ok_or_else(|| NO_PENDING_RESET.to_string()))
        .map_err(|error| failure_before_consume(&error))?
        .account_id;
    redeem_under_operation(state, &operation, &account_id, Redemption::Recovery)
}

/// Nothing can have been spent before the consume request, so a failure there
/// is "not started" unless it names a more specific cause.
fn failure_before_consume(error: &str) -> ResetCreditFailure {
    let code = if error.contains("Another GSwitch operation") {
        ResetCreditFailureCode::OperationBusy
    } else if error.contains("Quit Codex") {
        ResetCreditFailureCode::CodexOpen
    } else if error == RECOVER_PENDING_RESET_FIRST || error.contains("must be recovered first") {
        ResetCreditFailureCode::RecoveryRequired
    } else if error.contains("details are unavailable")
        || error.contains("did not return reset-credit information")
    {
        ResetCreditFailureCode::DetailsUnavailable
    } else if error == PICKED_CREDIT_UNAVAILABLE {
        ResetCreditFailureCode::CreditsChanged
    } else {
        ResetCreditFailureCode::NotStarted
    };
    ResetCreditFailure { code }
}

/// After the consume request was sent, only an explicit provider rejection is
/// distinguishable from an unknown outcome. Both keep the pending record.
fn failure_at_consume(error: &str) -> ResetCreditFailure {
    ResetCreditFailure {
        code: if error == REJECTED_REQUEST {
            ResetCreditFailureCode::ProviderRejected
        } else {
            ResetCreditFailureCode::ResultUnknown
        },
    }
}

const NO_PENDING_RESET: &str = "No reset-credit operation needs recovery";
const RECOVER_PENDING_RESET_FIRST: &str =
    "GSwitch must recover a previous reset-credit operation before starting another";
const PICKED_CREDIT_UNAVAILABLE: &str = "The selected reset credit is no longer available";
const PENDING_RESET_STILL_RETRYABLE: &str =
    "This reset can still be retried safely; retry it instead of discarding the record";

/// Clears a pending reset only when retrying can no longer help: its saved
/// account was removed, or the provider rejected a replay of a credit it no
/// longer lists as available. GSwitch cannot consume that credit later either
/// way, so discarding cannot lead to a second consumption.
pub fn discard_pending_reset_credit(state: &AppState) -> Result<(), String> {
    let operation = state.acquire_operation()?;
    let view = state
        .pending_reset_view()?
        .ok_or_else(|| NO_PENDING_RESET.to_string())?;
    if !view.discardable {
        return Err(PENDING_RESET_STILL_RETRYABLE.to_string());
    }
    state.clear_pending_reset_credit_under_operation(&operation)
}

/// Whether a redemption starts a new provider request for a picked credit or
/// may only replay the recorded one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Redemption {
    Pick(ResetCreditChoice),
    Recovery,
}

fn redeem_under_operation(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account_id: &str,
    redemption: Redemption,
) -> Result<ResetCreditOutcome, ResetCreditFailure> {
    let (account, identity, mut session) = (|| {
        runtime::ensure_no_external_codex(&[])?;
        let account = state.account_by_id_under_operation(operation, account_id)?;
        if account.kind == AccountKind::ApiKey {
            return Err("Reset credits are not available for API-key accounts".to_string());
        }
        let identity = verified_chatgpt_identity(&account)?;
        let session = AppServerResetSession::start(state, &account.credential)?;
        Ok((account, identity, session))
    })()
    .map_err(|error: String| failure_before_consume(&error))?;
    redeem_with_session(
        state,
        operation,
        &account,
        &identity,
        redemption,
        &mut session,
    )
}

/// The provider calls one reset-credit redemption makes. Production runs them
/// through an isolated official App Server; tests replay scripted results.
trait ResetCreditSession {
    fn read_rate_limits(&mut self) -> Result<Value, String>;
    fn consume(&mut self, idempotency_key: &str, credit_id: &str) -> Result<Value, String>;
    fn read_credential(&self) -> Result<Value, String>;
}

/// Fields drop in declaration order, so the App Server stops before its
/// isolated profile is removed.
struct AppServerResetSession {
    server: AppServer,
    temporary: TempCodexHome,
    next_request_id: i64,
}

impl AppServerResetSession {
    fn start(state: &AppState, credential: &Value) -> Result<Self, String> {
        let temporary = TempCodexHome::create(&state.isolated_profile_root()?)?;
        temporary.write_auth(credential)?;
        let server = AppServer::start(&temporary.path)?;
        Ok(Self {
            server,
            temporary,
            next_request_id: 1,
        })
    }
}

impl ResetCreditSession for AppServerResetSession {
    fn read_rate_limits(&mut self) -> Result<Value, String> {
        // A rate-limit read may retry once with the following request ID.
        let id = self.next_request_id;
        self.next_request_id += 2;
        self.server.rate_limits_read(id)
    }

    fn consume(&mut self, idempotency_key: &str, credit_id: &str) -> Result<Value, String> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.server
            .consume_reset_credit(id, idempotency_key, credit_id)
    }

    fn read_credential(&self) -> Result<Value, String> {
        self.temporary.read_auth()
    }
}

/// Everything decided before the consume request: the fresh provider state,
/// the durable pending record, and the expiry of the credit it names.
struct PreparedRedemption {
    normalized: NormalizedRateLimits,
    pending: PendingResetCredit,
    used_expires_at: Option<i64>,
}

fn redeem_with_session(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
    redemption: Redemption,
    session: &mut impl ResetCreditSession,
) -> Result<ResetCreditOutcome, ResetCreditFailure> {
    let PreparedRedemption {
        normalized,
        pending,
        used_expires_at,
    } = prepare_redemption(state, operation, account, identity, redemption, session)
        .map_err(|error| failure_before_consume(&error))?;

    let response = match session.consume(&pending.idempotency_key, &pending.credit_id) {
        Ok(response) => response,
        Err(error) => {
            if redemption == Redemption::Recovery
                && error == REJECTED_REQUEST
                && !recorded_credit_may_still_be_consumed(
                    normalized.reset_credits.as_ref(),
                    &pending.credit_id,
                    now_unix_ms() / 1000,
                )
            {
                let _ = state.allow_pending_reset_discard_under_operation(operation);
            }
            return Err(failure_at_consume(&error));
        }
    };
    let outcome = parse_reset_outcome(&response).map_err(|_| ResetCreditFailure {
        code: ResetCreditFailureCode::ResultUnknown,
    })?;
    let mut result = ResetCreditOutcome {
        account_id: account.id.clone(),
        outcome: outcome.clone(),
        quota: None,
        refresh_warning: None,
        used_expires_at,
    };
    complete_redemption(
        state,
        operation,
        account,
        identity,
        session,
        normalized,
        outcome,
        &mut result,
    );
    Ok(result)
}

fn prepare_redemption(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
    redemption: Redemption,
    session: &mut impl ResetCreditSession,
) -> Result<PreparedRedemption, String> {
    // This read both proves the provider is reachable and gives GSwitch the
    // complete current credential document before it attempts an irreversible
    // operation.
    let preflight = session.read_rate_limits()?;
    let preflight_credential = session.read_credential()?;
    ensure_reset_credential_identity(&preflight_credential, identity)?;
    let normalized = normalize_rate_limits_data(&preflight, now_unix_ms());
    persist_refreshed_credential_and_quota(
        state,
        operation,
        &account.id,
        &preflight_credential,
        normalized.snapshot.clone(),
        normalized.reset_credits.clone(),
    )?;

    // Only recovery replays a recorded request; a new pick never silently
    // consumes the earlier, possibly different credit instead.
    let now_seconds = now_unix_ms() / 1000;
    let (pending, used_expires_at) = match (
        state.pending_reset_credit_under_operation(operation)?,
        redemption,
    ) {
        (Some(pending), Redemption::Recovery) if pending.account_id == account.id => {
            let used_expires_at = normalized
                .reset_credits
                .as_ref()
                .and_then(|credits| credits.credits.as_ref())
                .and_then(|details| details.iter().find(|credit| credit.id == pending.credit_id))
                .and_then(|credit| credit.expires_at);
            (pending, used_expires_at)
        }
        (Some(pending), _) if pending.account_id != account.id => {
            return Err(
                "A reset-credit operation for another account must be recovered first".to_string(),
            )
        }
        (Some(_), Redemption::Pick(_)) => return Err(RECOVER_PENDING_RESET_FIRST.to_string()),
        (Some(_), Redemption::Recovery) | (None, Redemption::Recovery) => {
            return Err(NO_PENDING_RESET.to_string());
        }
        (None, Redemption::Pick(choice)) => {
            let credit =
                picked_available_credit(normalized.reset_credits.as_ref(), choice, now_seconds)?;
            let used_expires_at = credit.expires_at;
            let pending = PendingResetCredit {
                account_id: account.id.clone(),
                secret_ref: None,
                secret_generation: 0,
                credit_id: credit.id.clone(),
                idempotency_key: Uuid::new_v4().to_string(),
                created_at_unix_ms: now_unix_ms(),
                discard_allowed: false,
            };
            state.prepare_reset_credit_under_operation(operation, pending.clone())?;
            (pending, used_expires_at)
        }
    };
    Ok(PreparedRedemption {
        normalized,
        pending,
        used_expires_at,
    })
}

/// Saves what follows an authoritative provider result. A local problem here
/// becomes a warning on the confirmed result, never a failure.
#[allow(clippy::too_many_arguments)]
fn complete_redemption(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
    session: &mut impl ResetCreditSession,
    normalized: NormalizedRateLimits,
    outcome: ResetCreditOutcomeKind,
    result: &mut ResetCreditOutcome,
) {
    // An authoritative provider result must not be hidden just because the
    // follow-up persistence or refresh has a local problem. Keep the pending
    // idempotency record in those cases so recovery can safely reconcile it.
    let refreshed_credential = match session.read_credential() {
        Ok(credential) => credential,
        Err(_) => {
            result.refresh_warning = Some(
                "The reset result was confirmed, but GSwitch could not read refreshed credentials. Retry recovery before another reset."
                    .to_string(),
            );
            return;
        }
    };
    if ensure_reset_credential_identity(&refreshed_credential, identity).is_err() {
        let recovery_retained = state
            .record_pending_credential_for_identity(
                operation,
                &refreshed_credential,
                Some(identity),
            )
            .is_ok();
        result.refresh_warning = Some(
            if recovery_retained {
                "The reset result was confirmed, but GSwitch could not confirm refreshed credentials. A protected recovery copy was retained."
            } else {
                "The reset result was confirmed, but GSwitch could not confirm refreshed credentials or write protected recovery. The temporary profile was removed."
            }
            .to_string(),
        );
        return;
    }

    // A confirmed capacity-changing result makes the preflight cache stale
    // until the post-action provider read succeeds.
    let mut snapshot_after_result = normalized.snapshot;
    if matches!(
        outcome,
        ResetCreditOutcomeKind::Reset | ResetCreditOutcomeKind::AlreadyRedeemed
    ) {
        snapshot_after_result.fetched_at_unix_ms = 0;
    }
    if let Err(error) = persist_refreshed_credential_and_quota(
        state,
        operation,
        &account.id,
        &refreshed_credential,
        snapshot_after_result,
        normalized.reset_credits,
    ) {
        result.refresh_warning = Some(format!(
            "The reset result was confirmed, but {error} Retry recovery before another reset."
        ));
        return;
    }
    if state
        .clear_pending_reset_credit_under_operation(operation)
        .is_err()
    {
        result.refresh_warning = Some(
            "The reset result was confirmed, but GSwitch could not finalize its local transaction. Retry recovery before another reset."
                .to_string(),
        );
        return;
    }

    let (quota, warning) =
        refresh_after_confirmed_reset(state, operation, account, identity, session);
    result.quota = quota;
    result.refresh_warning = warning;
}

fn refresh_after_confirmed_reset(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
    session: &mut impl ResetCreditSession,
) -> (Option<QuotaView>, Option<String>) {
    let provider_state = match session.read_rate_limits() {
        Ok(value) => value,
        Err(_) => {
            return (
                None,
                Some(
                    "The reset result was confirmed, but the refreshed quota is not available yet"
                        .to_string(),
                ),
            );
        }
    };
    let credential = match session.read_credential() {
        Ok(credential) => credential,
        Err(_) => {
            return (
                None,
                Some(
                    "The reset result was confirmed, but GSwitch could not read refreshed credentials"
                        .to_string(),
                ),
            );
        }
    };
    if ensure_reset_credential_identity(&credential, identity).is_err() {
        let recovery_retained = state
            .record_pending_credential_for_identity(operation, &credential, Some(identity))
            .is_ok();
        return (
            None,
            Some(
                if recovery_retained {
                    "The reset result was confirmed, but GSwitch could not confirm refreshed credentials. A protected recovery copy was retained."
                } else {
                    "The reset result was confirmed, but GSwitch could not confirm refreshed credentials or write protected recovery. The temporary profile was removed."
                }
                .to_string(),
            ),
        );
    }

    let normalized = normalize_rate_limits_data(&provider_state, now_unix_ms());
    let snapshot = normalized.snapshot;
    match persist_refreshed_credential_and_quota(
        state,
        operation,
        &account.id,
        &credential,
        snapshot.clone(),
        normalized.reset_credits,
    ) {
        Ok(()) => (
            Some(view_from_snapshot(&account.id, snapshot, now_unix_ms())),
            None,
        ),
        Err(error) => (
            None,
            Some(format!("The reset result was confirmed, but {error}")),
        ),
    }
}

fn ensure_reset_credential_identity(
    credential: &Value,
    identity: &AccountIdentity,
) -> Result<(), String> {
    if document_kind(credential)? != AccountKind::ChatGpt
        || derive_identity(&AccountKind::ChatGpt, credential)? != *identity
    {
        return Err("Codex did not confirm the reset-credit account identity".to_string());
    }
    Ok(())
}

pub(crate) fn persist_refreshed_credential_and_quota(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account_id: &str,
    credential: &Value,
    snapshot: QuotaSnapshot,
    reset_credits: Option<StoredResetCredits>,
) -> Result<(), String> {
    if state
        .update_credential_and_quota_under_operation(
            operation,
            account_id,
            credential.clone(),
            snapshot,
            reset_credits,
        )
        .is_ok()
    {
        return Ok(());
    }

    if state
        .record_pending_credential(operation, credential)
        .is_ok()
    {
        return Err(
            "GSwitch could not save refreshed credentials. A protected recovery copy was retained."
                .to_string(),
        );
    }
    Err("GSwitch could not save refreshed credentials or write protected recovery. The temporary profile was removed.".to_string())
}

pub(crate) fn verified_chatgpt_identity(
    account: &StoredAccount,
) -> Result<AccountIdentity, String> {
    let identity = account.identity.clone().ok_or_else(|| {
        "The saved account needs to be added again before its quota can be read".to_string()
    })?;
    if account.kind != AccountKind::ChatGpt
        || document_kind(&account.credential)? != AccountKind::ChatGpt
        || derive_identity(&AccountKind::ChatGpt, &account.credential)? != identity
    {
        return Err(
            "The saved account credentials do not match their recorded identity".to_string(),
        );
    }
    Ok(identity)
}

/// When Codex is running, identify the active file-backed account immediately
/// before the provider request. A matching account gets a live token snapshot;
/// a different account remains eligible for a read-only saved snapshot.
pub(crate) fn live_credential_for_identity(
    identity: &AccountIdentity,
) -> Result<Option<Value>, String> {
    match external_credential_state_for_identity(identity)? {
        ExternalCredentialState::Matching(credential) => Ok(Some(credential)),
        ExternalCredentialState::NotRunning | ExternalCredentialState::DifferentAccount => Ok(None),
        ExternalCredentialState::Unidentifiable => Err(
            "Codex is running and GSwitch cannot safely identify its active account".to_string(),
        ),
    }
}

pub(crate) fn external_credential_state_for_identity(
    identity: &AccountIdentity,
) -> Result<ExternalCredentialState, String> {
    if !runtime::external_codex_running(&[])? {
        return Ok(ExternalCredentialState::NotRunning);
    }

    let codex_home = codex::codex_home()?;
    if codex::credential_store_mode(&codex_home)? != crate::types::CredentialStoreMode::File {
        return Ok(ExternalCredentialState::Unidentifiable);
    }
    let Some(live) = codex::read_optional_auth_document(&codex_home)? else {
        return Ok(ExternalCredentialState::Unidentifiable);
    };
    let live_kind = match document_kind(&live) {
        Ok(kind) => kind,
        Err(_) => return Ok(ExternalCredentialState::Unidentifiable),
    };
    let live_identity = match derive_identity(&live_kind, &live) {
        Ok(identity) => identity,
        Err(_) => return Ok(ExternalCredentialState::Unidentifiable),
    };
    if live_kind == AccountKind::ChatGpt && &live_identity == identity {
        return Ok(ExternalCredentialState::Matching(live));
    }
    Ok(ExternalCredentialState::DifferentAccount)
}

#[cfg(test)]
fn quota_refresh_conflicts_with_live_identity(
    target_kind: &AccountKind,
    target_identity: &AccountIdentity,
    live_kind: &AccountKind,
    live_identity: &AccountIdentity,
) -> bool {
    target_kind == &AccountKind::ChatGpt
        && live_kind == &AccountKind::ChatGpt
        && target_identity == live_identity
}

fn cached_view(account: &StoredAccount, now: i64) -> QuotaView {
    if account.kind == AccountKind::ApiKey {
        return not_applicable(account);
    }
    match account.quota.clone() {
        Some(snapshot) => view_from_snapshot(
            &account.id,
            with_current_credit_view(snapshot, account.reset_credits.as_ref(), now / 1000),
            now,
        ),
        None => QuotaView {
            account_id: account.id.clone(),
            status: QuotaStatus::Unknown,
            snapshot: None,
            message: Some("Quota has not been refreshed for this account".to_string()),
        },
    }
}

/// A saved credit list was filtered when it was read; recompute it so a credit
/// that has expired since then is no longer offered.
fn with_current_credit_view(
    mut snapshot: QuotaSnapshot,
    stored: Option<&StoredResetCredits>,
    now_seconds: i64,
) -> QuotaSnapshot {
    if let Some(stored) = stored {
        snapshot.reset_credits = Some(reset_credits_view(stored, now_seconds));
    }
    snapshot
}

fn not_applicable(account: &StoredAccount) -> QuotaView {
    QuotaView {
        account_id: account.id.clone(),
        status: QuotaStatus::NotApplicable,
        snapshot: None,
        message: Some("API-key accounts do not have ChatGPT subscription quota".to_string()),
    }
}

fn view_from_snapshot(account_id: &str, snapshot: QuotaSnapshot, now: i64) -> QuotaView {
    if snapshot.buckets.is_empty() {
        return QuotaView {
            account_id: account_id.to_string(),
            status: QuotaStatus::Unknown,
            snapshot: Some(snapshot),
            message: Some("Codex did not return subscription quota buckets".to_string()),
        };
    }
    let fresh = snapshot.fetched_at_unix_ms <= now
        && now - snapshot.fetched_at_unix_ms <= CACHE_FRESH_FOR_MS;
    QuotaView {
        account_id: account_id.to_string(),
        status: if fresh {
            QuotaStatus::Fresh
        } else {
            QuotaStatus::Stale
        },
        snapshot: Some(snapshot),
        message: None,
    }
}

/// Normalize the backwards-compatible single bucket and the optional
/// multi-bucket response without interpreting absent fields as zero or
/// unlimited capacity.
#[cfg(test)]
pub(crate) fn normalize_rate_limits(result: &Value, fetched_at_unix_ms: i64) -> QuotaSnapshot {
    normalize_rate_limits_data(result, fetched_at_unix_ms).snapshot
}

pub(crate) struct NormalizedRateLimits {
    pub snapshot: QuotaSnapshot,
    pub reset_credits: Option<StoredResetCredits>,
}

pub(crate) fn normalize_rate_limits_data(
    result: &Value,
    fetched_at_unix_ms: i64,
) -> NormalizedRateLimits {
    if result.get("rate_limit").is_some() {
        return normalize_chatgpt_rate_limits_data(result, fetched_at_unix_ms);
    }
    let buckets = rate_limit_sources(result)
        .into_iter()
        .map(|(fallback_id, bucket)| normalize_bucket(fallback_id, bucket))
        .collect();
    let reset_credits = normalize_stored_reset_credits(result);
    let reset_credits_view = reset_credits
        .as_ref()
        .map(|credits| reset_credits_view(credits, fetched_at_unix_ms / 1000));
    NormalizedRateLimits {
        snapshot: QuotaSnapshot {
            fetched_at_unix_ms,
            account_id: string_at(result.get("accountId")),
            ordinary_usage_allowed: result.get("ordinaryUsageAllowed").and_then(Value::as_bool),
            buckets,
            reset_credits: reset_credits_view,
        },
        reset_credits,
    }
}

fn normalize_chatgpt_rate_limits_data(
    result: &Value,
    fetched_at_unix_ms: i64,
) -> NormalizedRateLimits {
    let mut buckets = Vec::new();
    if let Some(rate_limit) = result.get("rate_limit").filter(|value| value.is_object()) {
        buckets.push(normalize_chatgpt_bucket(
            "codex",
            rate_limit,
            string_at(result.get("plan_type")),
            None,
        ));
    }
    if let Some(additional) = result
        .get("additional_rate_limits")
        .and_then(Value::as_array)
    {
        for entry in additional.iter().filter(|value| value.is_object()) {
            let rate_limit = entry
                .get("rate_limit")
                .filter(|value| value.is_object())
                .unwrap_or(entry);
            let fallback_id = string_at(entry.get("metered_feature"))
                .or_else(|| string_at(entry.get("limit_id")))
                .unwrap_or_else(|| "additional".to_string());
            buckets.push(normalize_chatgpt_bucket(
                &fallback_id,
                rate_limit,
                string_at(entry.get("plan_type")).or_else(|| string_at(result.get("plan_type"))),
                string_at(entry.get("limit_name")),
            ));
        }
    }
    let reset_credits = normalize_stored_reset_credits(result);
    let reset_credits_view = reset_credits
        .as_ref()
        .map(|credits| reset_credits_view(credits, fetched_at_unix_ms / 1000));
    let ordinary_usage_allowed = result
        .get("ordinary_usage_allowed")
        .and_then(Value::as_bool)
        .or_else(|| {
            result
                .get("rate_limit")
                .and_then(|value| value.get("allowed"))
                .and_then(Value::as_bool)
        });
    NormalizedRateLimits {
        snapshot: QuotaSnapshot {
            fetched_at_unix_ms,
            account_id: string_at(result.get("account_id")),
            ordinary_usage_allowed,
            buckets,
            reset_credits: reset_credits_view,
        },
        reset_credits,
    }
}

fn normalize_chatgpt_bucket(
    fallback_id: &str,
    bucket: &Value,
    plan_type: Option<String>,
    limit_name: Option<String>,
) -> QuotaBucket {
    let limit_id = string_at(bucket.get("limit_id"))
        .or_else(|| string_at(bucket.get("limitId")))
        .unwrap_or_else(|| fallback_id.to_string());
    let windows = [
        bucket.get("primary_window"),
        bucket.get("secondary_window"),
        bucket.get("primary"),
        bucket.get("secondary"),
    ]
    .into_iter()
    .flatten()
    .filter(|window| window.is_object())
    .map(normalize_window)
    .collect();
    QuotaBucket {
        kind: if limit_id == "codex" {
            QuotaBucketKind::Codex
        } else {
            QuotaBucketKind::Other
        },
        limit_id,
        limit_name: limit_name.or_else(|| string_at(bucket.get("limit_name"))),
        plan_type: plan_type.or_else(|| string_at(bucket.get("plan_type"))),
        rate_limit_reached_type: string_at(bucket.get("rate_limit_reached_type")),
        windows,
    }
}

fn normalize_stored_reset_credits(result: &Value) -> Option<StoredResetCredits> {
    let summary = result
        .get("rateLimitResetCredits")
        .or_else(|| result.get("rate_limit_reset_credits"))?
        .as_object()?;
    let available_count = summary
        .get("availableCount")
        .or_else(|| summary.get("available_count"))
        .and_then(nonnegative_u64_at)
        .unwrap_or(0);
    let credits = match summary.get("credits") {
        Some(Value::Array(values)) => Some(
            values
                .iter()
                .filter_map(|value| {
                    let id = string_at(value.get("id"))?;
                    let status = string_at(value.get("status"))?;
                    Some(StoredResetCredit {
                        id,
                        status,
                        expires_at: value
                            .get("expiresAt")
                            .or_else(|| value.get("expires_at"))
                            .and_then(unix_seconds_at),
                        reset_type: string_at(
                            value
                                .get("resetType")
                                .or_else(|| value.get("reset_type"))
                                .or_else(|| value.get("type")),
                        ),
                        granted_at: value
                            .get("grantedAt")
                            .or_else(|| value.get("granted_at"))
                            .and_then(unix_seconds_at),
                    })
                })
                .collect(),
        ),
        _ => None,
    };
    Some(StoredResetCredits {
        available_count,
        credits,
        details_read_at_unix_ms: None,
    })
}

fn reset_credits_view(credits: &StoredResetCredits, now_seconds: i64) -> ResetCreditsView {
    let mut usable_credits = credits
        .credits
        .as_ref()
        .map(|items| {
            items
                .iter()
                .filter(|credit| is_redeemable_credit(credit, now_seconds))
                .map(|credit| ResetCreditDetailView {
                    expires_at: credit.expires_at,
                    granted_at: credit.granted_at,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    usable_credits.sort_by_key(|credit| credit.expires_at.unwrap_or(i64::MAX));
    let nearest_expiry = usable_credits
        .iter()
        .filter_map(|credit| credit.expires_at)
        .min();
    ResetCreditsView {
        available_count: credits.available_count,
        nearest_expiry,
        details_available: credits.credits.is_some(),
        can_redeem: !usable_credits.is_empty(),
        usable_credits,
    }
}

/// Matches the user's pick against the fresh provider list. Expiry must match;
/// grant time must match when both sides report it. Credits that match on
/// both are interchangeable, so the first is used.
fn picked_available_credit(
    credits: Option<&StoredResetCredits>,
    choice: ResetCreditChoice,
    now_seconds: i64,
) -> Result<&StoredResetCredit, String> {
    let credits =
        credits.ok_or_else(|| "Codex did not return reset-credit information".to_string())?;
    let details = credits.credits.as_ref().ok_or_else(|| {
        "Reset-credit details are unavailable, so GSwitch cannot identify the selected credit"
            .to_string()
    })?;
    details
        .iter()
        .filter(|credit| is_redeemable_credit(credit, now_seconds))
        .find(|credit| {
            credit.expires_at == choice.expires_at
                && match (credit.granted_at, choice.granted_at) {
                    (Some(granted_at), Some(picked)) => granted_at == picked,
                    _ => true,
                }
        })
        .ok_or_else(|| PICKED_CREDIT_UNAVAILABLE.to_string())
}

/// Unknown details count as "may still be consumed", so they never permit a
/// discard.
fn recorded_credit_may_still_be_consumed(
    credits: Option<&StoredResetCredits>,
    credit_id: &str,
    now_seconds: i64,
) -> bool {
    match credits.and_then(|credits| credits.credits.as_ref()) {
        Some(details) => details
            .iter()
            .any(|credit| credit.id == credit_id && is_redeemable_credit(credit, now_seconds)),
        None => true,
    }
}

fn is_redeemable_credit(credit: &StoredResetCredit, now_seconds: i64) -> bool {
    credit.status == "available"
        && credit
            .expires_at
            .is_none_or(|expires_at| expires_at > now_seconds)
        && credit
            .reset_type
            .as_deref()
            .is_none_or(is_codex_rate_limit_reset)
}

/// Codex's App Server reports `codexRateLimits`; the usage endpoint uses a
/// snake-case spelling. Any other kind, including the protocol's `unknown`,
/// may reset something else and is never listed or redeemed.
fn is_codex_rate_limit_reset(reset_type: &str) -> bool {
    reset_type
        .chars()
        .filter(|character| *character != '_')
        .flat_map(char::to_lowercase)
        .eq("codexratelimits".chars())
}

fn parse_reset_outcome(value: &Value) -> Result<ResetCreditOutcomeKind, String> {
    match value.get("outcome").and_then(Value::as_str) {
        Some("reset") => Ok(ResetCreditOutcomeKind::Reset),
        Some("alreadyRedeemed") => Ok(ResetCreditOutcomeKind::AlreadyRedeemed),
        Some("nothingToReset") => Ok(ResetCreditOutcomeKind::NothingToReset),
        Some("noCredit") => Ok(ResetCreditOutcomeKind::NoCredit),
        _ => Err("Codex returned an unknown reset-credit result".to_string()),
    }
}

fn rate_limit_sources(result: &Value) -> Vec<(&str, &Value)> {
    let by_limit_id = result
        .get("rateLimitsByLimitId")
        .or_else(|| result.get("rate_limits_by_limit_id"));
    if let Some(by_limit_id) = by_limit_id.and_then(Value::as_object) {
        if !by_limit_id.is_empty() {
            let mut entries = by_limit_id
                .iter()
                .map(|(id, bucket)| (id.as_str(), bucket))
                .collect::<Vec<_>>();
            let has_codex_bucket = entries.iter().any(|(id, bucket)| {
                *id == "codex" || bucket.get("limitId").and_then(Value::as_str) == Some("codex")
            });
            if !has_codex_bucket {
                if let Some(legacy_codex) = result
                    .get("rateLimits")
                    .or_else(|| result.get("rate_limits"))
                    .filter(|bucket| bucket.is_object())
                {
                    entries.push(("codex", legacy_codex));
                }
            }
            entries.sort_by_key(|(id, _)| (*id != "codex", *id));
            return entries;
        }
    }
    result
        .get("rateLimits")
        .or_else(|| result.get("rate_limits"))
        .filter(|bucket| bucket.is_object())
        .map(|bucket| vec![("codex", bucket)])
        .unwrap_or_default()
}

fn normalize_bucket(fallback_id: &str, bucket: &Value) -> QuotaBucket {
    let limit_id = string_at(bucket.get("limitId"))
        .or_else(|| string_at(bucket.get("limit_id")))
        .unwrap_or_else(|| fallback_id.to_string());
    let windows = [bucket.get("primary"), bucket.get("secondary")]
        .into_iter()
        .flatten()
        .filter(|window| window.is_object())
        .map(normalize_window)
        .collect();
    QuotaBucket {
        kind: if limit_id == "codex" {
            QuotaBucketKind::Codex
        } else {
            QuotaBucketKind::Other
        },
        limit_id,
        limit_name: string_at(bucket.get("limitName"))
            .or_else(|| string_at(bucket.get("limit_name"))),
        plan_type: string_at(bucket.get("planType")).or_else(|| string_at(bucket.get("plan_type"))),
        rate_limit_reached_type: string_at(bucket.get("rateLimitReachedType"))
            .or_else(|| string_at(bucket.get("rate_limit_reached_type"))),
        windows,
    }
}

fn normalize_window(window: &Value) -> QuotaWindow {
    let used_percent = percent_at(
        window
            .get("usedPercent")
            .or_else(|| window.get("used_percent")),
    );
    let window_duration_mins = positive_i64_at(
        window
            .get("windowDurationMins")
            .or_else(|| window.get("window_duration_mins")),
    )
    .or_else(|| {
        positive_i64_at(
            window
                .get("limitWindowSeconds")
                .or_else(|| window.get("limit_window_seconds")),
        )
        .map(|seconds| seconds / 60)
        .filter(|minutes| *minutes > 0)
    });
    QuotaWindow {
        kind: match window_duration_mins {
            Some(300) => QuotaWindowKind::FiveHour,
            Some(10_080) => QuotaWindowKind::Weekly,
            _ => QuotaWindowKind::Other,
        },
        used_percent,
        remaining_percent: used_percent.map(|used| 100 - used),
        window_duration_mins,
        resets_at: positive_i64_at(
            window
                .get("resetsAt")
                .or_else(|| window.get("resets_at"))
                .or_else(|| window.get("reset_at")),
        ),
    }
}

fn string_at(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn percent_at(value: Option<&Value>) -> Option<u8> {
    value.and_then(Value::as_f64).and_then(|value| {
        (0.0..=100.0)
            .contains(&value)
            .then_some(value.round() as u8)
    })
}

fn positive_i64_at(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64).filter(|value| *value > 0)
}

fn nonnegative_u64_at(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
}

fn unix_seconds_at(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| {
            value
                .as_str()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.timestamp())
        })
}

pub(crate) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn normalizes_codex_and_other_buckets_by_duration() {
        let snapshot = normalize_rate_limits(
            &json!({
                "accountId": "account-1",
                "ordinaryUsageAllowed": true,
                "rateLimitsByLimitId": {
                    "image": {
                        "limitId": "image",
                        "primary": {"usedPercent": 88, "windowDurationMins": 60, "resetsAt": 500}
                    },
                    "codex": {
                        "limitId": "codex",
                        "planType": "pro",
                        "primary": {"usedPercent": 25, "windowDurationMins": 300, "resetsAt": 400},
                        "secondary": {"usedPercent": 18, "windowDurationMins": 10080, "resetsAt": 900}
                    }
                }
            }),
            100,
        );

        assert_eq!(snapshot.account_id.as_deref(), Some("account-1"));
        assert_eq!(snapshot.buckets.len(), 2);
        assert_eq!(snapshot.buckets[0].kind, QuotaBucketKind::Codex);
        assert_eq!(
            snapshot.buckets[0].windows[0].kind,
            QuotaWindowKind::FiveHour
        );
        assert_eq!(snapshot.buckets[0].windows[0].remaining_percent, Some(75));
        assert_eq!(snapshot.buckets[0].windows[1].kind, QuotaWindowKind::Weekly);
        assert_eq!(snapshot.buckets[1].kind, QuotaBucketKind::Other);
    }

    #[test]
    fn normalizes_current_chatgpt_usage_shape_and_additional_limits() {
        let snapshot = normalize_rate_limits(
            &json!({
                "plan_type": "plus",
                "user_id": "user",
                "account_id": "workspace",
                "rate_limit": {
                    "allowed": true,
                    "primary_window": {
                        "used_percent": 25.5,
                        "limit_window_seconds": 18000,
                        "reset_at": 900
                    },
                    "secondary_window": {
                        "used_percent": 80,
                        "limit_window_seconds": 604800,
                        "reset_at": 1000
                    }
                },
                "additional_rate_limits": [{
                    "limit_name": "Images",
                    "metered_feature": "images",
                    "rate_limit": {
                        "primary_window": {"used_percent": 10, "limit_window_seconds": 300}
                    }
                }]
            }),
            100,
        );

        assert_eq!(snapshot.account_id.as_deref(), Some("workspace"));
        assert_eq!(snapshot.ordinary_usage_allowed, Some(true));
        assert_eq!(snapshot.buckets.len(), 2);
        assert_eq!(snapshot.buckets[0].kind, QuotaBucketKind::Codex);
        assert_eq!(
            snapshot.buckets[0].windows[0].kind,
            QuotaWindowKind::FiveHour
        );
        assert_eq!(snapshot.buckets[0].windows[0].used_percent, Some(26));
        assert_eq!(snapshot.buckets[0].windows[1].kind, QuotaWindowKind::Weekly);
        assert_eq!(snapshot.buckets[1].limit_name.as_deref(), Some("Images"));
        assert_eq!(snapshot.buckets[1].kind, QuotaBucketKind::Other);
    }

    #[test]
    fn current_chatgpt_reset_credit_expiry_is_normalized_without_exposing_ids() {
        let normalized = normalize_rate_limits_data(
            &json!({
                "rate_limit": {},
                "rate_limit_reset_credits": {
                    "available_count": 1,
                    "credits": [{
                        "id": "opaque-provider-credit-id",
                        "status": "available",
                        "expires_at": "1970-01-01T00:15:00Z"
                    }]
                }
            }),
            100_000,
        );
        let view = normalized.snapshot.reset_credits.expect("credit view");
        assert_eq!(view.available_count, 1);
        assert_eq!(view.nearest_expiry, Some(900));
        assert!(view.details_available);
        assert!(!serde_json::to_string(&view)
            .expect("serialize view")
            .contains("opaque-provider-credit-id"));
    }

    #[test]
    fn falls_back_to_the_legacy_single_bucket() {
        let snapshot = normalize_rate_limits(
            &json!({
                "rateLimits": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 4, "windowDurationMins": 300}
                },
                "rateLimitsByLimitId": null
            }),
            100,
        );

        assert_eq!(snapshot.buckets.len(), 1);
        assert_eq!(snapshot.buckets[0].limit_id, "codex");
    }

    #[test]
    fn retains_a_legacy_codex_bucket_when_multi_bucket_data_omits_it() {
        let snapshot = normalize_rate_limits(
            &json!({
                "rateLimits": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 4, "windowDurationMins": 300}
                },
                "rateLimitsByLimitId": {
                    "image": {"limitId": "image", "primary": {"usedPercent": 5, "windowDurationMins": 60}}
                }
            }),
            100,
        );

        assert_eq!(snapshot.buckets.len(), 2);
        assert_eq!(snapshot.buckets[0].limit_id, "codex");
        assert_eq!(snapshot.buckets[1].limit_id, "image");
    }

    #[test]
    fn retains_the_free_five_week_window_and_its_reset_time() {
        let snapshot = normalize_rate_limits(
            &json!({
                "plan_type": "free",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 0,
                        "limit_window_seconds": 3_024_000,
                        "reset_at": 1_800_000_000
                    }
                }
            }),
            100,
        );

        let window = &snapshot.buckets[0].windows[0];
        assert_eq!(window.kind, QuotaWindowKind::Other);
        assert_eq!(window.window_duration_mins, Some(50_400));
        assert_eq!(window.remaining_percent, Some(100));
        assert_eq!(window.resets_at, Some(1_800_000_000));
    }
    #[test]
    fn keeps_missing_or_malformed_values_unknown() {
        let snapshot = normalize_rate_limits(
            &json!({
                "rateLimits": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 101, "windowDurationMins": -1, "resetsAt": 0}
                }
            }),
            100,
        );

        let window = &snapshot.buckets[0].windows[0];
        assert_eq!(window.used_percent, None);
        assert_eq!(window.remaining_percent, None);
        assert_eq!(window.window_duration_mins, None);
        assert_eq!(window.resets_at, None);
    }

    #[test]
    fn preserves_an_exhausted_window_as_zero_remaining() {
        let snapshot = normalize_rate_limits(
            &json!({
                "ordinaryUsageAllowed": false,
                "rateLimits": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 100, "windowDurationMins": 300}
                }
            }),
            100,
        );

        assert_eq!(snapshot.ordinary_usage_allowed, Some(false));
        assert_eq!(snapshot.buckets[0].windows[0].remaining_percent, Some(0));
    }

    #[test]
    fn labels_an_old_snapshot_stale_without_inventing_new_values() {
        let snapshot = QuotaSnapshot {
            fetched_at_unix_ms: 1,
            account_id: None,
            ordinary_usage_allowed: None,
            buckets: vec![QuotaBucket {
                limit_id: "codex".into(),
                limit_name: None,
                plan_type: None,
                rate_limit_reached_type: None,
                kind: QuotaBucketKind::Codex,
                windows: vec![],
            }],
            reset_credits: None,
        };
        let view = view_from_snapshot("account", snapshot, CACHE_FRESH_FOR_MS + 2);
        assert_eq!(view.status, QuotaStatus::Stale);
    }

    #[test]
    fn blocks_only_the_running_codex_account_from_an_isolated_refresh() {
        let target = AccountIdentity::ChatGpt {
            user_id: "user".to_string(),
            workspace_id: Some("workspace".to_string()),
        };
        let other = AccountIdentity::ChatGpt {
            user_id: "other-user".to_string(),
            workspace_id: Some("workspace".to_string()),
        };

        assert!(quota_refresh_conflicts_with_live_identity(
            &AccountKind::ChatGpt,
            &target,
            &AccountKind::ChatGpt,
            &target,
        ));
        assert!(!quota_refresh_conflicts_with_live_identity(
            &AccountKind::ChatGpt,
            &target,
            &AccountKind::ChatGpt,
            &other,
        ));
    }

    #[test]
    fn reset_credit_count_is_authoritative_and_ids_do_not_enter_the_view() {
        let normalized = normalize_rate_limits_data(
            &json!({
                "rateLimits": {"limitId": "codex"},
                "rateLimitResetCredits": {
                    "availableCount": 4,
                    "credits": [{
                        "id": "opaque-provider-credit-id",
                        "status": "available",
                        "expiresAt": 900
                    }]
                }
            }),
            100_000,
        );
        let view = normalized
            .snapshot
            .reset_credits
            .expect("reset-credit view");

        assert_eq!(view.available_count, 4);
        assert_eq!(view.nearest_expiry, Some(900));
        assert!(view.details_available);
        assert!(view.can_redeem);
        assert_eq!(view.usable_credits.len(), 1);
        let serialized = serde_json::to_string(&view).expect("serialize view");
        assert!(!serialized.contains("opaque-provider-credit-id"));
    }

    #[test]
    fn reset_credit_without_details_cannot_be_redeemed() {
        let normalized = normalize_rate_limits_data(
            &json!({
                "rateLimits": {"limitId": "codex"},
                "rateLimitResetCredits": {"availableCount": 2, "credits": null}
            }),
            100_000,
        );
        let view = normalized
            .snapshot
            .reset_credits
            .expect("reset-credit view");

        assert_eq!(view.available_count, 2);
        assert!(!view.details_available);
        assert!(!view.can_redeem);
        assert!(view.usable_credits.is_empty());
        assert!(picked_available_credit(
            normalized.reset_credits.as_ref(),
            ResetCreditChoice {
                expires_at: None,
                granted_at: None,
            },
            100
        )
        .is_err());
    }

    #[test]
    fn listed_credits_are_unexpired_sorted_and_only_those_can_be_picked() {
        let credits = StoredResetCredits {
            available_count: 4,
            credits: Some(vec![
                StoredResetCredit {
                    id: "expired".into(),
                    status: "available".into(),
                    expires_at: Some(99),
                    reset_type: None,
                    granted_at: None,
                },
                StoredResetCredit {
                    id: "later".into(),
                    status: "available".into(),
                    expires_at: Some(300),
                    reset_type: None,
                    granted_at: None,
                },
                StoredResetCredit {
                    id: "without-expiry".into(),
                    status: "available".into(),
                    expires_at: None,
                    reset_type: None,
                    granted_at: None,
                },
                StoredResetCredit {
                    id: "first".into(),
                    status: "available".into(),
                    expires_at: Some(200),
                    reset_type: None,
                    granted_at: None,
                },
            ]),
            details_read_at_unix_ms: None,
        };

        let pick = |expires_at| ResetCreditChoice {
            expires_at,
            granted_at: None,
        };
        assert!(picked_available_credit(Some(&credits), pick(Some(99)), 100).is_err());
        assert_eq!(
            picked_available_credit(Some(&credits), pick(Some(300)), 100)
                .expect("picked credit")
                .id,
            "later"
        );
        let view = reset_credits_view(&credits, 100);
        assert_eq!(view.nearest_expiry, Some(200));
        assert_eq!(view.usable_credits[0].expires_at, Some(200));
        assert_eq!(view.usable_credits[2].expires_at, None);
    }

    #[test]
    fn a_saved_credit_list_drops_credits_that_expired_since_it_was_read() {
        let stored = StoredResetCredits {
            available_count: 2,
            credits: Some(vec![
                StoredResetCredit {
                    id: "soon".into(),
                    status: "available".into(),
                    expires_at: Some(200),
                    reset_type: None,
                    granted_at: Some(10),
                },
                StoredResetCredit {
                    id: "later".into(),
                    status: "available".into(),
                    expires_at: Some(300),
                    reset_type: None,
                    granted_at: Some(20),
                },
            ]),
            details_read_at_unix_ms: None,
        };
        let snapshot = QuotaSnapshot {
            fetched_at_unix_ms: 100_000,
            account_id: None,
            ordinary_usage_allowed: None,
            buckets: vec![],
            reset_credits: Some(reset_credits_view(&stored, 100)),
        };

        let current = with_current_credit_view(snapshot, Some(&stored), 250)
            .reset_credits
            .expect("credit view");

        assert_eq!(
            current.usable_credits,
            vec![ResetCreditDetailView {
                expires_at: Some(300),
                granted_at: Some(20),
            }]
        );
        assert_eq!(current.nearest_expiry, Some(300));
    }

    fn saved_credits(count: u64, read_at: Option<i64>) -> StoredResetCredits {
        StoredResetCredits {
            available_count: count,
            credits: Some(vec![StoredResetCredit {
                id: "saved".into(),
                status: "available".into(),
                expires_at: Some(4_000_000_000),
                reset_type: None,
                granted_at: None,
            }]),
            details_read_at_unix_ms: read_at,
        }
    }

    fn usage_with_count(count: Option<u64>) -> Value {
        match count {
            Some(count) => json!({
                "rate_limit": {},
                "rate_limit_reset_credits": {"available_count": count}
            }),
            None => json!({"rate_limit": {}}),
        }
    }

    #[test]
    fn saved_credit_details_are_reused_only_while_the_count_matches_and_they_are_fresh() {
        let now = 10 * CREDIT_DETAILS_FRESH_FOR_MS;
        let fresh = saved_credits(2, Some(now - 1_000));

        assert!(reusable_credit_details(Some(&fresh), &usage_with_count(Some(2)), now).is_some());
        assert!(reusable_credit_details(Some(&fresh), &usage_with_count(Some(1)), now).is_none());
        assert!(reusable_credit_details(Some(&fresh), &usage_with_count(None), now).is_none());
        assert!(reusable_credit_details(None, &usage_with_count(Some(2)), now).is_none());

        let stale = saved_credits(2, Some(now - CREDIT_DETAILS_FRESH_FOR_MS));
        assert!(reusable_credit_details(Some(&stale), &usage_with_count(Some(2)), now).is_none());
        let unknown_age = saved_credits(2, None);
        assert!(
            reusable_credit_details(Some(&unknown_age), &usage_with_count(Some(2)), now).is_none()
        );
        let from_the_future = saved_credits(2, Some(now + 1));
        assert!(
            reusable_credit_details(Some(&from_the_future), &usage_with_count(Some(2)), now)
                .is_none()
        );
        let without_details = StoredResetCredits {
            credits: None,
            ..saved_credits(2, Some(now - 1_000))
        };
        assert!(
            reusable_credit_details(Some(&without_details), &usage_with_count(Some(2)), now)
                .is_none()
        );
    }

    #[test]
    fn reused_credit_details_fill_the_usage_only_projection() {
        let saved = saved_credits(1, Some(5_000));
        let normalized = with_saved_credit_details(
            normalize_rate_limits_data(&usage_with_count(Some(1)), 100_000),
            &saved,
        );

        let stored = normalized.reset_credits.expect("stored credits");
        assert_eq!(stored.details_read_at_unix_ms, Some(5_000));
        assert_eq!(stored.credits.expect("details").len(), 1);
        let view = normalized.snapshot.reset_credits.expect("credit view");
        assert!(view.details_available);
        assert!(view.can_redeem);
        assert_eq!(view.usable_credits.len(), 1);
    }

    #[test]
    fn only_codex_rate_limit_resets_are_listed() {
        let normalized = normalize_rate_limits_data(
            &json!({
                "rate_limit": {},
                "rate_limit_reset_credits": {
                    "available_count": 4,
                    "credits": [
                        {"id": "a", "status": "available", "expires_at": 300, "reset_type": "codex_rate_limits"},
                        {"id": "b", "status": "available", "expires_at": 200, "reset_type": "unknown"},
                        {"id": "c", "status": "available", "expires_at": 400, "type": "image_generation"},
                        {"id": "d", "status": "available", "expires_at": 500}
                    ]
                }
            }),
            100_000,
        );
        let view = normalized.snapshot.reset_credits.expect("credit view");

        assert_eq!(view.available_count, 4);
        assert_eq!(view.nearest_expiry, Some(300));
        assert_eq!(
            view.usable_credits
                .iter()
                .map(|credit| credit.expires_at)
                .collect::<Vec<_>>(),
            vec![Some(300), Some(500)]
        );
        assert!(is_codex_rate_limit_reset("codexRateLimits"));
        assert!(is_codex_rate_limit_reset("codex_rate_limits"));
        assert!(!is_codex_rate_limit_reset("unknown"));
    }

    #[test]
    fn reset_outcomes_keep_idempotent_success_distinct_from_no_consumption() {
        assert_eq!(
            parse_reset_outcome(&json!({"outcome": "alreadyRedeemed"})).expect("outcome"),
            ResetCreditOutcomeKind::AlreadyRedeemed
        );
        assert_eq!(
            parse_reset_outcome(&json!({"outcome": "noCredit"})).expect("outcome"),
            ResetCreditOutcomeKind::NoCredit
        );
        assert!(parse_reset_outcome(&json!({"outcome": "unexpected"})).is_err());
    }

    mod redeem_flow {
        use std::{cell::RefCell, collections::VecDeque, fs, path::PathBuf};

        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        use serde_json::json;

        use super::super::*;
        use crate::accounts::AccountDraft;

        const FUTURE: i64 = 4_000_000_000;

        fn credential(user: &str) -> Value {
            let claims = json!({
                "https://api.openai.com/auth": {
                    "chatgpt_user_id": user,
                    "chatgpt_account_id": "workspace"
                }
            });
            let id_token = format!(
                "header.{}.signature",
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims"))
            );
            json!({"tokens": {"id_token": id_token, "access_token": format!("{user}-token")}})
        }

        fn state_with_accounts(users: &[&str]) -> (AppState, Vec<StoredAccount>, PathBuf) {
            let root = std::env::temp_dir().join(format!("gswitch-reset-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).expect("test directory");
            let state = AppState::new(root.join("accounts.json")).expect("state");
            let operation = state.acquire_operation().expect("operation");
            let accounts = users
                .iter()
                .map(|user| {
                    state
                        .upsert_under_operation(
                            &operation,
                            AccountDraft {
                                label: None,
                                default_label: format!("{user}@example.com"),
                                kind: AccountKind::ChatGpt,
                                email: Some(format!("{user}@example.com")),
                                plan_type: None,
                                workspace_name: None,
                                account_structure: Some("workspace".into()),
                                identity: AccountIdentity::ChatGpt {
                                    user_id: (*user).into(),
                                    workspace_id: Some("workspace".into()),
                                },
                                credential: credential(user),
                            },
                        )
                        .expect("save account")
                })
                .collect::<Vec<_>>();
            let accounts = accounts
                .iter()
                .map(|saved| {
                    state
                        .account_by_id_under_operation(&operation, &saved.id)
                        .expect("saved account")
                })
                .collect();
            drop(operation);
            (state, accounts, root)
        }

        fn rate_limits(credits: &[(&str, i64)]) -> Value {
            json!({
                "rateLimits": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 100, "windowDurationMins": 300}
                },
                "rateLimitResetCredits": {
                    "availableCount": credits.len(),
                    "credits": credits
                        .iter()
                        .map(|(id, expires_at)| json!({
                            "id": id,
                            "status": "available",
                            "expiresAt": expires_at
                        }))
                        .collect::<Vec<_>>()
                }
            })
        }

        struct ScriptedSession<'a> {
            state: &'a AppState,
            reads: VecDeque<Result<Value, String>>,
            consume_results: VecDeque<Result<Value, String>>,
            credential: Value,
            credential_failures_after_consume: RefCell<bool>,
            consumed: Vec<(String, String, bool)>,
        }

        impl<'a> ScriptedSession<'a> {
            fn new(state: &'a AppState, user: &str) -> Self {
                Self {
                    state,
                    reads: VecDeque::new(),
                    consume_results: VecDeque::new(),
                    credential: credential(user),
                    credential_failures_after_consume: RefCell::new(false),
                    consumed: Vec::new(),
                }
            }

            fn read(mut self, result: Result<Value, String>) -> Self {
                self.reads.push_back(result);
                self
            }

            fn consume_returns(mut self, result: Result<Value, String>) -> Self {
                self.consume_results.push_back(result);
                self
            }
        }

        impl ResetCreditSession for ScriptedSession<'_> {
            fn read_rate_limits(&mut self) -> Result<Value, String> {
                self.reads
                    .pop_front()
                    .unwrap_or_else(|| Err("no scripted rate-limit read".into()))
            }

            fn consume(&mut self, idempotency_key: &str, credit_id: &str) -> Result<Value, String> {
                let pending_was_durable = self
                    .state
                    .pending_reset_view()
                    .expect("pending reset state")
                    .is_some();
                self.consumed.push((
                    idempotency_key.to_string(),
                    credit_id.to_string(),
                    pending_was_durable,
                ));
                self.consume_results
                    .pop_front()
                    .unwrap_or_else(|| Err("no scripted consume result".into()))
            }

            fn read_credential(&self) -> Result<Value, String> {
                if *self.credential_failures_after_consume.borrow() && !self.consumed.is_empty() {
                    return Err("credential file is unreadable".into());
                }
                Ok(self.credential.clone())
            }
        }

        fn pick(expires_at: i64) -> ResetCreditChoice {
            ResetCreditChoice {
                expires_at: Some(expires_at),
                granted_at: None,
            }
        }

        fn redeem(
            state: &AppState,
            account: &StoredAccount,
            choice: ResetCreditChoice,
            session: &mut ScriptedSession<'_>,
        ) -> Result<ResetCreditOutcome, ResetCreditFailure> {
            let operation = state.acquire_operation().expect("operation");
            let identity = verified_chatgpt_identity(account).expect("identity");
            redeem_with_session(
                state,
                &operation,
                account,
                &identity,
                Redemption::Pick(choice),
                session,
            )
        }

        fn recover(
            state: &AppState,
            account: &StoredAccount,
            session: &mut ScriptedSession<'_>,
        ) -> Result<ResetCreditOutcome, ResetCreditFailure> {
            let operation = state.acquire_operation().expect("operation");
            let identity = verified_chatgpt_identity(account).expect("identity");
            redeem_with_session(
                state,
                &operation,
                account,
                &identity,
                Redemption::Recovery,
                session,
            )
        }

        fn pending(state: &AppState) -> Option<PendingResetCredit> {
            let operation = state.acquire_operation().expect("operation");
            state
                .pending_reset_credit_under_operation(&operation)
                .expect("pending reset state")
        }

        #[test]
        fn the_pending_record_is_durable_before_consume_and_cleared_after_a_reset() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut session = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                .consume_returns(Ok(json!({"outcome": "reset"})))
                .read(Ok(rate_limits(&[])));

            let result = redeem(&state, &accounts[0], pick(FUTURE), &mut session).expect("redeem");

            assert_eq!(result.outcome, ResetCreditOutcomeKind::Reset);
            assert!(result.refresh_warning.is_none());
            assert!(result.quota.is_some());
            assert_eq!(session.consumed.len(), 1);
            assert_eq!(session.consumed[0].1, "credit-a");
            assert!(
                session.consumed[0].2,
                "pending record must exist before consume"
            );
            assert!(pending(&state).is_none());
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_failed_consume_keeps_the_pending_record() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut session = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                .consume_returns(Err("Codex App Server did not respond".into()));

            assert!(redeem(&state, &accounts[0], pick(FUTURE), &mut session).is_err());

            let recorded = pending(&state).expect("pending record");
            assert_eq!(recorded.account_id, accounts[0].id);
            assert_eq!(recorded.credit_id, "credit-a");
            assert_eq!(recorded.idempotency_key, session.consumed[0].0);
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_new_pick_is_refused_while_a_record_is_pending() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut first = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                .consume_returns(Err("Codex App Server did not respond".into()));
            assert!(redeem(&state, &accounts[0], pick(FUTURE), &mut first).is_err());

            // Only recovery may replay the recorded request.
            let mut again = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[
                    ("credit-b", FUTURE - 10),
                    ("credit-a", FUTURE),
                ])))
                .consume_returns(Ok(json!({"outcome": "reset"})));
            let error =
                redeem(&state, &accounts[0], pick(FUTURE - 10), &mut again).expect_err("pending");

            assert_eq!(error.code, ResetCreditFailureCode::RecoveryRequired);
            assert!(again.consumed.is_empty());
            assert_eq!(pending(&state).expect("pending").credit_id, "credit-a");
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn another_accounts_pending_record_blocks_redemption() {
            let (state, accounts, root) = state_with_accounts(&["first", "second"]);
            let mut first = ScriptedSession::new(&state, "first")
                .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                .consume_returns(Err("Codex App Server did not respond".into()));
            assert!(redeem(&state, &accounts[0], pick(FUTURE), &mut first).is_err());

            let mut second = ScriptedSession::new(&state, "second")
                .read(Ok(rate_limits(&[("credit-z", FUTURE)])))
                .consume_returns(Ok(json!({"outcome": "reset"})));
            let error =
                redeem(&state, &accounts[1], pick(FUTURE), &mut second).expect_err("blocked");

            assert_eq!(error.code, ResetCreditFailureCode::RecoveryRequired);
            assert!(second.consumed.is_empty());
            assert_eq!(
                pending(&state).expect("pending record").account_id,
                accounts[0].id
            );
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn every_authoritative_outcome_clears_the_pending_record() {
            for (outcome, expected) in [
                ("reset", ResetCreditOutcomeKind::Reset),
                ("alreadyRedeemed", ResetCreditOutcomeKind::AlreadyRedeemed),
                ("nothingToReset", ResetCreditOutcomeKind::NothingToReset),
                ("noCredit", ResetCreditOutcomeKind::NoCredit),
            ] {
                let (state, accounts, root) = state_with_accounts(&["user"]);
                let mut session = ScriptedSession::new(&state, "user")
                    .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                    .consume_returns(Ok(json!({"outcome": outcome})))
                    .read(Ok(rate_limits(&[("credit-a", FUTURE)])));

                let result =
                    redeem(&state, &accounts[0], pick(FUTURE), &mut session).expect("redeem");

                assert_eq!(result.outcome, expected);
                assert!(pending(&state).is_none(), "{outcome} must clear the record");
                let _ = fs::remove_dir_all(root);
            }
        }

        #[test]
        fn an_unknown_outcome_keeps_the_pending_record() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut session = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                .consume_returns(Ok(json!({"outcome": "somethingNew"})));

            assert!(redeem(&state, &accounts[0], pick(FUTURE), &mut session).is_err());
            assert!(pending(&state).is_some());
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn an_unreadable_credential_after_consume_keeps_the_confirmed_result_recoverable() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut session = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                .consume_returns(Ok(json!({"outcome": "reset"})));
            *session.credential_failures_after_consume.borrow_mut() = true;

            let result = redeem(&state, &accounts[0], pick(FUTURE), &mut session).expect("redeem");

            assert_eq!(result.outcome, ResetCreditOutcomeKind::Reset);
            assert!(result.refresh_warning.is_some());
            assert!(pending(&state).is_some());
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_credit_of_another_reset_kind_is_never_redeemed() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut credits = rate_limits(&[("other-kind", FUTURE - 10), ("codex", FUTURE)]);
            credits["rateLimitResetCredits"]["credits"][0]["resetType"] = json!("unknown");
            credits["rateLimitResetCredits"]["credits"][1]["resetType"] = json!("codexRateLimits");
            let mut session = ScriptedSession::new(&state, "user")
                .read(Ok(credits))
                .consume_returns(Ok(json!({"outcome": "reset"})))
                .read(Ok(rate_limits(&[])));

            let error = redeem(&state, &accounts[0], pick(FUTURE - 10), &mut session)
                .expect_err("other kind");
            assert_eq!(error.code, ResetCreditFailureCode::CreditsChanged);
            assert!(session.consumed.is_empty());

            let mut credits = rate_limits(&[("other-kind", FUTURE - 10), ("codex", FUTURE)]);
            credits["rateLimitResetCredits"]["credits"][0]["resetType"] = json!("unknown");
            credits["rateLimitResetCredits"]["credits"][1]["resetType"] = json!("codexRateLimits");
            let mut session = ScriptedSession::new(&state, "user")
                .read(Ok(credits))
                .consume_returns(Ok(json!({"outcome": "reset"})))
                .read(Ok(rate_limits(&[])));
            redeem(&state, &accounts[0], pick(FUTURE), &mut session).expect("redeem");

            assert_eq!(session.consumed.len(), 1);
            assert_eq!(session.consumed[0].1, "codex");
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn the_picked_credit_is_redeemed_rather_than_the_earliest() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut session = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[
                    ("earliest", FUTURE - 10),
                    ("picked", FUTURE),
                ])))
                .consume_returns(Ok(json!({"outcome": "reset"})))
                .read(Ok(rate_limits(&[("earliest", FUTURE - 10)])));

            let result = redeem(&state, &accounts[0], pick(FUTURE), &mut session).expect("redeem");

            assert_eq!(session.consumed.len(), 1);
            assert_eq!(session.consumed[0].1, "picked");
            assert_eq!(result.used_expires_at, Some(FUTURE));
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_pick_that_is_no_longer_available_consumes_nothing() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut session = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("remaining", FUTURE)])))
                .consume_returns(Ok(json!({"outcome": "reset"})));

            let error =
                redeem(&state, &accounts[0], pick(FUTURE - 10), &mut session).expect_err("gone");

            assert_eq!(error.code, ResetCreditFailureCode::CreditsChanged);
            assert!(session.consumed.is_empty());
            assert!(pending(&state).is_none());
            let operation = state.acquire_operation().expect("operation");
            let saved = state
                .account_by_id_under_operation(&operation, &accounts[0].id)
                .expect("account");
            assert_eq!(
                saved
                    .reset_credits
                    .and_then(|credits| credits.credits)
                    .map(|credits| credits.len()),
                Some(1),
                "the fresh list is saved so the dialog can show it"
            );
            drop(operation);
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn grant_time_tells_apart_credits_that_expire_together() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut credits = rate_limits(&[("granted-first", FUTURE), ("granted-later", FUTURE)]);
            credits["rateLimitResetCredits"]["credits"][0]["grantedAt"] = json!(100);
            credits["rateLimitResetCredits"]["credits"][1]["grantedAt"] = json!(200);
            let mut session = ScriptedSession::new(&state, "user")
                .read(Ok(credits))
                .consume_returns(Ok(json!({"outcome": "reset"})))
                .read(Ok(rate_limits(&[])));

            let choice = ResetCreditChoice {
                expires_at: Some(FUTURE),
                granted_at: Some(200),
            };
            redeem(&state, &accounts[0], choice, &mut session).expect("redeem");

            assert_eq!(session.consumed[0].1, "granted-later");
            let _ = fs::remove_dir_all(root);
        }

        fn leave_a_pending_record(state: &AppState, account: &StoredAccount) {
            let mut first = ScriptedSession::new(state, "user")
                .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                .consume_returns(Err("Codex App Server did not respond".into()));
            assert!(redeem(state, account, pick(FUTURE), &mut first).is_err());
        }

        fn discardable(state: &AppState) -> bool {
            state
                .pending_reset_view()
                .expect("pending reset state")
                .expect("pending record")
                .discardable
        }

        #[test]
        fn a_rejected_replay_of_a_credit_no_longer_listed_can_be_discarded() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            leave_a_pending_record(&state, &accounts[0]);
            assert!(!discardable(&state));
            assert_eq!(
                discard_pending_reset_credit(&state).expect_err("retryable"),
                PENDING_RESET_STILL_RETRYABLE
            );

            let mut replay = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("credit-b", FUTURE)])))
                .consume_returns(Err(REJECTED_REQUEST.into()));
            assert!(recover(&state, &accounts[0], &mut replay).is_err());

            assert!(discardable(&state));
            discard_pending_reset_credit(&state).expect("discard");
            assert!(pending(&state).is_none());
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_rejected_replay_of_a_still_listed_credit_stays_retryable() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            leave_a_pending_record(&state, &accounts[0]);

            let mut replay = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                .consume_returns(Err(REJECTED_REQUEST.into()));
            assert!(recover(&state, &accounts[0], &mut replay).is_err());

            assert!(!discardable(&state));
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn an_unknown_replay_outcome_never_allows_a_discard() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            leave_a_pending_record(&state, &accounts[0]);

            let mut replay = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("credit-b", FUTURE)])))
                .consume_returns(Err("Codex App Server did not respond".into()));
            assert!(recover(&state, &accounts[0], &mut replay).is_err());

            assert!(!discardable(&state));
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_record_whose_account_is_gone_can_be_discarded() {
            let (state, accounts, root) = state_with_accounts(&["user", "other"]);
            leave_a_pending_record(&state, &accounts[0]);
            {
                let operation = state.acquire_operation().expect("operation");
                state
                    .remove_under_operation(&operation, &accounts[0].id)
                    .expect("remove without the switching guard");
            }

            assert!(discardable(&state));
            discard_pending_reset_credit(&state).expect("discard");
            assert!(pending(&state).is_none());
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn consume_failures_say_whether_the_provider_answered() {
            for (error, expected) in [
                (REJECTED_REQUEST, ResetCreditFailureCode::ProviderRejected),
                (
                    "Codex App Server did not respond",
                    ResetCreditFailureCode::ResultUnknown,
                ),
            ] {
                let (state, accounts, root) = state_with_accounts(&["user"]);
                let mut session = ScriptedSession::new(&state, "user")
                    .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                    .consume_returns(Err(error.into()));

                let failure =
                    redeem(&state, &accounts[0], pick(FUTURE), &mut session).expect_err("failure");

                assert_eq!(failure.code, expected);
                assert!(pending(&state).is_some(), "{error} must keep the record");
                let _ = fs::remove_dir_all(root);
            }
        }

        #[test]
        fn a_failure_before_consume_is_reported_as_not_started() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut session =
                ScriptedSession::new(&state, "user").read(Err("Unable to reach ChatGPT".into()));

            let failure =
                redeem(&state, &accounts[0], pick(FUTURE), &mut session).expect_err("failure");

            assert_eq!(failure.code, ResetCreditFailureCode::NotStarted);
            assert!(session.consumed.is_empty());
            assert!(pending(&state).is_none());
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_busy_operation_lock_is_reported_before_any_provider_work() {
            let root = std::env::temp_dir().join(format!("gswitch-reset-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).expect("test directory");
            let state = AppState::new(root.join("accounts.json")).expect("state");
            let operation = state.acquire_operation().expect("operation");

            let failure = redeem_reset_credit(&state, "account", pick(FUTURE)).expect_err("busy");

            assert_eq!(failure.code, ResetCreditFailureCode::OperationBusy);
            drop(operation);
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn recovery_without_a_record_never_consumes_a_credit() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut session = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                .consume_returns(Ok(json!({"outcome": "reset"})));

            let error = recover(&state, &accounts[0], &mut session).expect_err("no record");

            assert_eq!(error.code, ResetCreditFailureCode::NotStarted);
            assert!(session.consumed.is_empty());
            assert!(pending(&state).is_none());
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn recovery_replays_only_the_recorded_credit() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut first = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                .consume_returns(Err("Codex App Server did not respond".into()));
            assert!(redeem(&state, &accounts[0], pick(FUTURE), &mut first).is_err());

            let mut recovery = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[
                    ("credit-b", FUTURE - 10),
                    ("credit-a", FUTURE),
                ])))
                .consume_returns(Ok(json!({"outcome": "reset"})))
                .read(Ok(rate_limits(&[("credit-b", FUTURE - 10)])));
            let result = recover(&state, &accounts[0], &mut recovery).expect("recovery");

            assert_eq!(result.outcome, ResetCreditOutcomeKind::Reset);
            assert_eq!(recovery.consumed.len(), 1);
            assert_eq!(recovery.consumed[0].0, first.consumed[0].0);
            assert_eq!(recovery.consumed[0].1, "credit-a");
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn recovery_entry_point_reports_a_missing_record_before_any_provider_work() {
            // An empty store keeps credential fixtures out of the real entry
            // point, which would otherwise reach App Server code.
            let root = std::env::temp_dir().join(format!("gswitch-reset-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).expect("test directory");
            let state = AppState::new(root.join("accounts.json")).expect("state");
            assert_eq!(
                recover_pending_reset_credit(&state)
                    .expect_err("no record")
                    .code,
                ResetCreditFailureCode::NotStarted
            );
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_failed_follow_up_read_keeps_the_confirmed_result() {
            let (state, accounts, root) = state_with_accounts(&["user"]);
            let mut session = ScriptedSession::new(&state, "user")
                .read(Ok(rate_limits(&[("credit-a", FUTURE)])))
                .consume_returns(Ok(json!({"outcome": "reset"})))
                .read(Err("Unable to reach ChatGPT".into()));

            let result = redeem(&state, &accounts[0], pick(FUTURE), &mut session).expect("redeem");

            assert_eq!(result.outcome, ResetCreditOutcomeKind::Reset);
            assert!(result.quota.is_none());
            assert!(result.refresh_warning.is_some());
            assert!(pending(&state).is_none());
            let _ = fs::remove_dir_all(root);
        }
    }
}
