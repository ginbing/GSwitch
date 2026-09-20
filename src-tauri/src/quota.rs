use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::{
    accounts::{AppState, OperationGuard},
    app_server::{AppServer, TempCodexHome},
    identity::{derive_identity, document_kind},
    runtime,
    types::{
        AccountIdentity, AccountKind, QuotaBucket, QuotaBucketKind, QuotaSnapshot, QuotaStatus,
        QuotaView, QuotaWindow, QuotaWindowKind, StoredAccount,
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
    runtime::ensure_no_external_codex(&[])?;
    let account = state.account_by_id_under_operation(&operation, account_id)?;
    if account.kind == AccountKind::ApiKey {
        return Ok(not_applicable(&account));
    }
    let identity = verified_chatgpt_identity(&account)?;

    let mut temporary = TempCodexHome::create(&state.isolated_profile_root()?)?;
    temporary.write_auth(&account.credential)?;
    let mut server = AppServer::start(&temporary.path)?;
    let result = server.rate_limits_read(1)?;
    let refreshed_credential = temporary.read_auth()?;

    if document_kind(&refreshed_credential)? != AccountKind::ChatGpt
        || derive_identity(&AccountKind::ChatGpt, &refreshed_credential)? != identity
    {
        return Err("Codex did not confirm the quota account identity".to_string());
    }

    let snapshot = normalize_rate_limits(&result, now_unix_ms());
    persist_refreshed_credential_and_quota(
        state,
        &operation,
        &account.id,
        &refreshed_credential,
        snapshot.clone(),
        &mut temporary,
    )?;
    Ok(view_from_snapshot(&account.id, snapshot, now_unix_ms()))
}

fn persist_refreshed_credential_and_quota(
    state: &AppState,
    operation: &OperationGuard<'_>,
    account_id: &str,
    credential: &Value,
    snapshot: QuotaSnapshot,
    temporary: &mut TempCodexHome,
) -> Result<(), String> {
    if state
        .update_credential_and_quota_under_operation(
            operation,
            account_id,
            credential.clone(),
            snapshot,
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

fn verified_chatgpt_identity(account: &StoredAccount) -> Result<AccountIdentity, String> {
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
pub(crate) fn normalize_rate_limits(result: &Value, fetched_at_unix_ms: i64) -> QuotaSnapshot {
    let buckets = rate_limit_sources(result)
        .into_iter()
        .map(|(fallback_id, bucket)| normalize_bucket(fallback_id, bucket))
        .collect();
    QuotaSnapshot {
        fetched_at_unix_ms,
        account_id: string_at(result.get("accountId")),
        ordinary_usage_allowed: result.get("ordinaryUsageAllowed").and_then(Value::as_bool),
        buckets,
    }
}

fn rate_limit_sources(result: &Value) -> Vec<(&str, &Value)> {
    if let Some(by_limit_id) = result.get("rateLimitsByLimitId").and_then(Value::as_object) {
        if !by_limit_id.is_empty() {
            let mut entries = by_limit_id
                .iter()
                .map(|(id, bucket)| (id.as_str(), bucket))
                .collect::<Vec<_>>();
            let has_codex_bucket = entries.iter().any(|(id, bucket)| {
                *id == "codex" || bucket.get("limitId").and_then(Value::as_str) == Some("codex")
            });
            if !has_codex_bucket {
                if let Some(legacy_codex) =
                    result.get("rateLimits").filter(|bucket| bucket.is_object())
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
        .filter(|bucket| bucket.is_object())
        .map(|bucket| vec![("codex", bucket)])
        .unwrap_or_default()
}

fn normalize_bucket(fallback_id: &str, bucket: &Value) -> QuotaBucket {
    let limit_id = string_at(bucket.get("limitId")).unwrap_or_else(|| fallback_id.to_string());
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
        limit_name: string_at(bucket.get("limitName")),
        plan_type: string_at(bucket.get("planType")),
        rate_limit_reached_type: string_at(bucket.get("rateLimitReachedType")),
        windows,
    }
}

fn normalize_window(window: &Value) -> QuotaWindow {
    let used_percent = percent_at(window.get("usedPercent"));
    let window_duration_mins = positive_i64_at(window.get("windowDurationMins"));
    QuotaWindow {
        kind: match window_duration_mins {
            Some(300) => QuotaWindowKind::FiveHour,
            Some(10_080) => QuotaWindowKind::Weekly,
            _ => QuotaWindowKind::Other,
        },
        used_percent,
        remaining_percent: used_percent.map(|used| 100 - used),
        window_duration_mins,
        resets_at: positive_i64_at(window.get("resetsAt")),
    }
}

fn string_at(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn percent_at(value: Option<&Value>) -> Option<u8> {
    value
        .and_then(Value::as_i64)
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| *value <= 100)
}

fn positive_i64_at(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64).filter(|value| *value > 0)
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
        };
        let view = view_from_snapshot("account", snapshot, CACHE_FRESH_FOR_MS + 2);
        assert_eq!(view.status, QuotaStatus::Stale);
    }
}
