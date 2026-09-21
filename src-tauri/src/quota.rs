use std::time::{SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    accounts::{AppState, OperationGuard},
    app_server::{AppServer, TempCodexHome},
    chatgpt::{self, ChatGptClient, RequestFailure, RequestFailureKind},
    codex,
    identity::{derive_identity, document_kind},
    runtime,
    types::{
        AccountIdentity, AccountKind, PendingResetCredit, QuotaBucket, QuotaBucketKind,
        QuotaSnapshot, QuotaStatus, QuotaView, QuotaWindow, QuotaWindowKind, ResetCreditDetailView,
        ResetCreditOutcome, ResetCreditOutcomeKind, ResetCreditsView, StoredAccount,
        StoredResetCredit, StoredResetCredits,
    },
};

const CACHE_FRESH_FOR_MS: i64 = 5 * 60 * 1000;

/// Returns the last provider snapshot without initiating a provider request.
pub fn cached_quota(state: &AppState, account_id: &str) -> Result<QuotaView, String> {
    let account = state.account_by_id(account_id)?;
    Ok(cached_view(&account, now_unix_ms()))
}

/// Reads capacity through an isolated official Codex App Server profile. A
/// successful call proves the saved ChatGPT credential can reach this official
/// account endpoint; `account/read` alone is deliberately not used as proof.
pub fn refresh_quota(state: &AppState, account_id: &str) -> Result<QuotaView, String> {
    let operation = state.acquire_operation()?;
    let account = state.account_by_id_under_operation(&operation, account_id)?;
    if account.kind == AccountKind::ApiKey {
        return Ok(not_applicable(&account));
    }
    let identity = verified_chatgpt_identity(&account)?;
    let live_credential = live_credential_for_identity(&identity)?;
    if let Some(credential) = live_credential {
        return refresh_active_read_only(state, &operation, &account, &identity, credential);
    }

    match refresh_read_only(state, &operation, &account, &identity, &account.credential) {
        Ok(view) => Ok(view),
        Err(ReadOnlyRefreshFailure::Provider(error)) if error.can_fallback_to_managed_refresh() => {
            refresh_via_managed_profile(state, &operation, &account, &identity)
        }
        Err(error) => Err(error.message()),
    }
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
    let client = ChatGptClient::new().map_err(ReadOnlyRefreshFailure::Message)?;
    let response = client
        .quota(credential)
        .map_err(ReadOnlyRefreshFailure::Provider)?;
    chatgpt::validate_response_identity(credential, identity, &response.usage)
        .map_err(ReadOnlyRefreshFailure::Message)?;

    let merged = chatgpt::merge_reset_credit_details(
        &response.usage,
        response.reset_credit_details.as_ref(),
    );
    let mut normalized = normalize_rate_limits_data(&merged, now_unix_ms());
    if response.detail_failure.is_some() {
        if let Some(reset_credits) = normalized.reset_credits.as_mut() {
            reset_credits.credits = None;
            normalized.snapshot.reset_credits = Some(reset_credits_view(
                reset_credits,
                normalized.snapshot.fetched_at_unix_ms / 1000,
            ));
        }
    }
    let snapshot = normalized.snapshot;
    state
        .update_quota_under_operation(
            operation,
            &account.id,
            snapshot.clone(),
            normalized.reset_credits,
        )
        .map_err(ReadOnlyRefreshFailure::Message)?;
    Ok(view_from_snapshot(&account.id, snapshot, now_unix_ms()))
}

enum ReadOnlyRefreshFailure {
    Provider(RequestFailure),
    Message(String),
}

impl ReadOnlyRefreshFailure {
    fn message(self) -> String {
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

fn refresh_via_managed_profile(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
) -> Result<QuotaView, String> {
    let mut temporary = TempCodexHome::create(&state.isolated_profile_root()?)?;
    temporary.write_auth(&account.credential)?;
    let mut server = AppServer::start(&temporary.path)?;
    let result = server.rate_limits_read(1)?;
    let refreshed_credential = temporary.read_auth()?;

    if document_kind(&refreshed_credential)? != AccountKind::ChatGpt
        || derive_identity(&AccountKind::ChatGpt, &refreshed_credential)? != *identity
    {
        return Err("Codex did not confirm the quota account identity".to_string());
    }

    let normalized = normalize_rate_limits_data(&result, now_unix_ms());
    let snapshot = normalized.snapshot;
    persist_refreshed_credential_and_quota(
        state,
        &operation,
        &account.id,
        &refreshed_credential,
        snapshot.clone(),
        normalized.reset_credits,
        &mut temporary,
    )?;
    Ok(view_from_snapshot(&account.id, snapshot, now_unix_ms()))
}

/// Redeems one user-confirmed reset credit through the official App Server.
/// The durable idempotency record is written before the provider call so an
/// interrupted request can only ever be retried with the same credit and key.
pub fn redeem_earliest_reset_credit(
    state: &AppState,
    account_id: &str,
) -> Result<ResetCreditOutcome, String> {
    let operation = state.acquire_operation()?;
    runtime::ensure_no_external_codex(&[])?;
    let account = state.account_by_id_under_operation(&operation, account_id)?;
    if account.kind == AccountKind::ApiKey {
        return Err("Reset credits are not available for API-key accounts".to_string());
    }
    let identity = verified_chatgpt_identity(&account)?;

    let mut temporary = TempCodexHome::create(&state.isolated_profile_root()?)?;
    temporary.write_auth(&account.credential)?;
    let mut server = AppServer::start(&temporary.path)?;

    // This read both proves the provider is reachable and gives GSwitch the
    // complete current credential document before it attempts an irreversible
    // operation.
    let preflight = server.rate_limits_read(1)?;
    let preflight_credential = temporary.read_auth()?;
    ensure_reset_credential_identity(&preflight_credential, &identity)?;
    let normalized = normalize_rate_limits_data(&preflight, now_unix_ms());
    persist_refreshed_credential_and_quota(
        state,
        &operation,
        &account.id,
        &preflight_credential,
        normalized.snapshot.clone(),
        normalized.reset_credits.clone(),
        &mut temporary,
    )?;

    let pending = match state.pending_reset_credit_under_operation(&operation)? {
        Some(pending) if pending.account_id == account.id => pending,
        Some(_) => {
            return Err(
                "A reset-credit operation for another account must be recovered first".to_string(),
            )
        }
        None => {
            let credit_id =
                earliest_available_credit(normalized.reset_credits.as_ref(), now_unix_ms() / 1000)?
                    .id
                    .clone();
            let pending = PendingResetCredit {
                account_id: account.id.clone(),
                credit_id,
                idempotency_key: Uuid::new_v4().to_string(),
                created_at_unix_ms: now_unix_ms(),
            };
            state.prepare_reset_credit_under_operation(&operation, pending.clone())?;
            pending
        }
    };

    let response = server.consume_reset_credit(3, &pending.idempotency_key, &pending.credit_id)?;
    let outcome = parse_reset_outcome(&response)?;
    let mut result = ResetCreditOutcome {
        account_id: account.id.clone(),
        outcome: outcome.clone(),
        quota: None,
        refresh_warning: None,
    };

    // An authoritative provider result must not be hidden just because the
    // follow-up persistence or refresh has a local problem. Keep the pending
    // idempotency record in those cases so recovery can safely reconcile it.
    let refreshed_credential = match temporary.read_auth() {
        Ok(credential) => credential,
        Err(_) => {
            result.refresh_warning = Some(
                "The reset result was confirmed, but GSwitch could not read refreshed credentials. Retry recovery before another reset."
                    .to_string(),
            );
            return Ok(result);
        }
    };
    if ensure_reset_credential_identity(&refreshed_credential, &identity).is_err() {
        temporary.retain_for_recovery();
        result.refresh_warning = Some(
            "The reset result was confirmed, but GSwitch could not confirm refreshed credentials. A protected recovery copy was retained."
                .to_string(),
        );
        return Ok(result);
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
        &operation,
        &account.id,
        &refreshed_credential,
        snapshot_after_result,
        normalized.reset_credits,
        &mut temporary,
    ) {
        result.refresh_warning = Some(format!(
            "The reset result was confirmed, but {error} Retry recovery before another reset."
        ));
        return Ok(result);
    }
    if state
        .clear_pending_reset_credit_under_operation(&operation)
        .is_err()
    {
        result.refresh_warning = Some(
            "The reset result was confirmed, but GSwitch could not finalize its local transaction. Retry recovery before another reset."
                .to_string(),
        );
        return Ok(result);
    }

    let (quota, warning) = refresh_after_confirmed_reset(
        state,
        &operation,
        &account,
        &identity,
        &mut server,
        &mut temporary,
    );
    result.quota = quota;
    result.refresh_warning = warning;
    Ok(result)
}

/// Replays a pending request with its original credit and idempotency key.
/// This never selects a new credit during recovery.
pub fn recover_pending_reset_credit(state: &AppState) -> Result<ResetCreditOutcome, String> {
    let account_id = {
        let operation = state.acquire_operation()?;
        state
            .pending_reset_credit_under_operation(&operation)?
            .ok_or_else(|| "No reset-credit operation needs recovery".to_string())?
            .account_id
    };
    redeem_earliest_reset_credit(state, &account_id)
}

fn refresh_after_confirmed_reset(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account: &StoredAccount,
    identity: &AccountIdentity,
    server: &mut AppServer,
    temporary: &mut TempCodexHome,
) -> (Option<QuotaView>, Option<String>) {
    let provider_state = match server.rate_limits_read(4) {
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
    let credential = match temporary.read_auth() {
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
        temporary.retain_for_recovery();
        return (
            None,
            Some(
                "The reset result was confirmed, but GSwitch could not confirm refreshed credentials. A protected recovery copy was retained."
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
        temporary,
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
    temporary: &mut TempCodexHome,
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
        .is_err()
    {
        // Keep the isolated profile only as a last-resort protected recovery
        // copy. It remains outside the WebView and is not a backup system.
        temporary.retain_for_recovery();
    }
    Err(
        "GSwitch could not save refreshed credentials. A protected recovery copy was retained."
            .to_string(),
    )
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
    if !runtime::external_codex_running(&[])? {
        return Ok(None);
    }

    let codex_home = codex::codex_home()?;
    if codex::credential_store_mode(&codex_home)? != crate::types::CredentialStoreMode::File {
        return Err(
            "Codex is running and GSwitch cannot safely identify its active account".to_string(),
        );
    }
    let live = codex::read_optional_auth_document(&codex_home)?.ok_or_else(|| {
        "Codex is running and GSwitch cannot safely identify its active account".to_string()
    })?;
    let live_kind = document_kind(&live).map_err(|_| {
        "Codex is running and GSwitch cannot safely identify its active account".to_string()
    })?;
    let live_identity = derive_identity(&live_kind, &live).map_err(|_| {
        "Codex is running and GSwitch cannot safely identify its active account".to_string()
    })?;
    if live_kind == AccountKind::ChatGpt && &live_identity == identity {
        return Ok(Some(live));
    }
    Ok(None)
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
        Some(snapshot) => view_from_snapshot(&account.id, snapshot, now),
        None => QuotaView {
            account_id: account.id.clone(),
            status: QuotaStatus::Unknown,
            snapshot: None,
            message: Some("Quota has not been refreshed for this account".to_string()),
        },
    }
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
                    })
                })
                .collect(),
        ),
        _ => None,
    };
    Some(StoredResetCredits {
        available_count,
        credits,
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

fn earliest_available_credit(
    credits: Option<&StoredResetCredits>,
    now_seconds: i64,
) -> Result<&StoredResetCredit, String> {
    let credits =
        credits.ok_or_else(|| "Codex did not return reset-credit information".to_string())?;
    let details = credits.credits.as_ref().ok_or_else(|| {
        "Reset-credit details are unavailable, so GSwitch cannot safely choose the earliest credit"
            .to_string()
    })?;
    details
        .iter()
        .filter(|credit| is_redeemable_credit(credit, now_seconds))
        .min_by_key(|credit| credit.expires_at.unwrap_or(i64::MAX))
        .ok_or_else(|| "No available reset credit can be redeemed".to_string())
}

fn is_redeemable_credit(credit: &StoredResetCredit, now_seconds: i64) -> bool {
    credit.status == "available"
        && credit
            .expires_at
            .is_none_or(|expires_at| expires_at > now_seconds)
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
        assert!(earliest_available_credit(normalized.reset_credits.as_ref(), 100).is_err());
    }

    #[test]
    fn reset_credit_selection_uses_the_earliest_unexpired_available_credit() {
        let credits = StoredResetCredits {
            available_count: 4,
            credits: Some(vec![
                StoredResetCredit {
                    id: "expired".into(),
                    status: "available".into(),
                    expires_at: Some(99),
                },
                StoredResetCredit {
                    id: "later".into(),
                    status: "available".into(),
                    expires_at: Some(300),
                },
                StoredResetCredit {
                    id: "without-expiry".into(),
                    status: "available".into(),
                    expires_at: None,
                },
                StoredResetCredit {
                    id: "first".into(),
                    status: "available".into(),
                    expires_at: Some(200),
                },
            ]),
        };

        assert_eq!(
            earliest_available_credit(Some(&credits), 100)
                .expect("eligible credit")
                .id,
            "first"
        );
        let view = reset_credits_view(&credits, 100);
        assert_eq!(view.nearest_expiry, Some(200));
        assert_eq!(view.usable_credits[0].expires_at, Some(200));
        assert_eq!(view.usable_credits[2].expires_at, None);
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
}
