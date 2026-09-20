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
    pub identity: Option<AccountIdentity>,
    pub credential: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountView {
    pub id: String,
    pub label: String,
    pub kind: AccountKind,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub active: bool,
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
    Failed { message: String },
}
