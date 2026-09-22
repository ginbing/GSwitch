use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    ChatGpt,
    ApiKey,
}

/// A non-secret identifier used only to decide whether two saved profiles are
/// the same Codex account. The complete credential document stays Rust-owned.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AccountIdentity {
    ChatGpt {
        user_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_id: Option<String>,
    },
    ApiKey {
        fingerprint: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredAccount {
    pub id: String,
    pub label: String,
    pub kind: AccountKind,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_structure: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<AccountIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<QuotaSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<StoredResetCredits>,
    pub credential: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountView {
    pub id: String,
    pub label: String,
    pub kind: AccountKind,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    pub active: bool,
}

/// A display-only outcome for one bounded user-selected account-file batch.
/// Parsed credentials and failure details stay in Rust; the WebView receives
/// only saved profiles and aggregate counts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportResult {
    #[serde(default)]
    pub imported: Vec<AccountView>,
    pub duplicate_count: u32,
    pub unsupported_count: u32,
    pub failed_count: u32,
}

/// A source adapter exposed by the one-shot local migration assistant. The
/// enum is intentionally closed: discovery never becomes a generic plugin or
/// filesystem search surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MigrationSource {
    OfficialCodex,
    CockpitTools,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MigrationCandidateState {
    New,
    AlreadyPresent,
    Unsupported,
}

/// Sanitized preview data for a local migration candidate. Credential
/// documents, raw source records, keys, paths, and provider payloads remain
/// Rust-owned and never cross the Tauri boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationCandidateView {
    pub id: String,
    pub source: MigrationSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    pub state: MigrationCandidateState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationPreview {
    #[serde(default)]
    pub candidates: Vec<MigrationCandidateView>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PendingSwitch {
    pub target_id: String,
    pub target_identity: AccountIdentity,
    pub previous_active_id: Option<String>,
    pub previous_auth: Option<Value>,
    pub stage: PendingSwitchStage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingSwitchStage {
    Prepared,
    Verified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStoreMode {
    File,
    Keyring,
    Auto,
    Ephemeral,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub codex_home: String,
    pub auth_file_exists: bool,
    pub credential_store: CredentialStoreMode,
}

/// The health of GSwitch-owned account storage. This deliberately says
/// nothing about the live Codex credential file: a damaged GSwitch library
/// must never be mistaken for a damaged Codex installation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageStatus {
    Ready,
    RecoveryRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageView {
    pub status: StorageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A single read-only view for the initial workspace render. Credential
/// documents and raw storage errors remain on the Rust side.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSnapshot {
    pub storage: StorageView,
    #[serde(default)]
    pub accounts: Vec<AccountView>,
    /// A deliberately coarse recovery signal. The corresponding account,
    /// provider credit, and idempotency key must remain in Rust-owned storage.
    #[serde(default)]
    pub pending_reset_credit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<LiveAccountView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveAccountStatus {
    Ready,
    NotSignedIn,
    UnknownAccount,
    FileStoreRequired,
    RecoveryRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveAccountView {
    pub status: LiveAccountStatus,
    pub credential_store: CredentialStoreMode,
    pub account: Option<AccountView>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwitchOutcome {
    pub account: AccountView,
}

/// A provider-reported capacity snapshot. It is operational state, not usage
/// analytics: credentials and raw App Server payloads stay in Rust.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuotaSnapshot {
    pub fetched_at_unix_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinary_usage_allowed: Option<bool>,
    #[serde(default)]
    pub buckets: Vec<QuotaBucket>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<ResetCreditsView>,
}

/// A display-only reset-credit summary. It intentionally excludes opaque
/// provider credit IDs; Rust reselects the eligible credit immediately before
/// a user-confirmed redemption.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResetCreditsView {
    pub available_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nearest_expiry: Option<i64>,
    pub details_available: bool,
    pub can_redeem: bool,
    #[serde(default)]
    pub usable_credits: Vec<ResetCreditDetailView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResetCreditDetailView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

/// Rust-owned reset-credit data. The opaque ID is never part of a Tauri
/// command response or WebView state.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredResetCredits {
    pub available_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<Vec<StoredResetCredit>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredResetCredit {
    pub id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PendingResetCredit {
    pub account_id: String,
    pub credit_id: String,
    pub idempotency_key: String,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResetCreditOutcomeKind {
    Reset,
    AlreadyRedeemed,
    NothingToReset,
    NoCredit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResetCreditOutcome {
    pub account_id: String,
    pub outcome: ResetCreditOutcomeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<QuotaView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_warning: Option<String>,
}

/// A short-lived, in-memory Wake result. Wake is intentionally not a job
/// system: completed operations may disappear when the app restarts or when a
/// later operation replaces them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WakeResultKind {
    Started,
    AlreadyActive,
    NoOrdinaryCapacity,
    NeedsSignIn,
    SentNotConfirmed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WakeAccountResult {
    pub account_id: String,
    pub label: String,
    pub result: WakeResultKind,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WakeOperationStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WakeOperationView {
    pub id: String,
    pub status: WakeOperationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_account_id: Option<String>,
    #[serde(default)]
    pub results: Vec<WakeAccountResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WakeStart {
    pub operation_id: String,
}

/// How a signed update is delivered on this particular installed package.
/// This is intentionally independent from Codex account state: it only tells
/// the WebView whether Tauri can install and whether a relaunch is required.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateDelivery {
    InstallerExits,
    RelaunchRequired,
    ReleaseDownload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuotaBucket {
    pub limit_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_reached_type: Option<String>,
    pub kind: QuotaBucketKind,
    #[serde(default)]
    pub windows: Vec<QuotaWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuotaBucketKind {
    Codex,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuotaWindow {
    pub kind: QuotaWindowKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_duration_mins: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuotaWindowKind {
    FiveHour,
    Weekly,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuotaStatus {
    Fresh,
    Stale,
    Unknown,
    NotApplicable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuotaView {
    pub account_id: String,
    pub status: QuotaStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<QuotaSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthLoginStart {
    pub login_id: String,
    pub auth_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OAuthLoginStatus {
    Pending,
    Complete { account: AccountView },
    Cancelled,
    Failed { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_snapshot_exposes_reset_recovery_as_a_boolean_only() {
        let snapshot = AppSnapshot {
            storage: StorageView {
                status: StorageStatus::Ready,
                message: None,
            },
            accounts: Vec::new(),
            pending_reset_credit: true,
            runtime: None,
            live: None,
        };

        let serialized = serde_json::to_string(&snapshot).expect("serialize snapshot");
        assert!(serialized.contains("\"pending_reset_credit\":true"));
        assert!(!serialized.contains("credit_id"));
        assert!(!serialized.contains("idempotency_key"));
    }
}
