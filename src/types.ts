export type AccountKind = "chat_gpt" | "api_key";

export interface AccountView {
  id: string;
  label: string;
  kind: AccountKind;
  email?: string;
  plan_type?: string;
  workspace_name?: string;
  active: boolean;
}

export type CredentialStoreMode =
  | "file"
  | "keyring"
  | "auto"
  | "ephemeral"
  | "unknown";

export interface RuntimeInfo {
  codex_home: string;
  auth_file_exists: boolean;
  credential_store: CredentialStoreMode;
}

export interface CodexCliInfo {
  version?: string;
  supports_update: boolean;
}

export type CodexCliUpdateFailure =
  | "not_installed"
  | "unsupported"
  | "busy"
  | "codex_open"
  | "update_failed"
  | "verification_failed";

export type StorageStatus = "ready" | "recovery_required";

export interface StorageView {
  status: StorageStatus;
  message?: string;
}

export interface AppSnapshot {
  storage: StorageView;
  accounts: AccountView[];
  pending_reset_credit: boolean;
  runtime?: RuntimeInfo;
  live?: LiveAccountView;
}

export type LiveAccountStatus =
  | "ready"
  | "not_signed_in"
  | "unknown_account"
  | "file_store_required"
  | "recovery_required";

export interface LiveAccountView {
  status: LiveAccountStatus;
  credential_store: CredentialStoreMode;
  account?: AccountView;
  message?: string;
}

export interface SwitchOutcome {
  account: AccountView;
}

export type SwitchFailureCode =
  | "operation_busy"
  | "codex_open"
  | "account_needs_sign_in"
  | "file_store_required"
  | "credentials_changed"
  | "recovery_required"
  | "local_verification_failed"
  | "codex_app_server_unavailable"
  | "codex_config_unavailable"
  | "codex_config_cleanup_failed"
  | "current_credential_unreadable"
  | "current_account_not_saved"
  | "target_check_unavailable"
  | "target_workspace_mismatch"
  | "post_write_verification_failed"
  | "verification_failed";

export interface SwitchFailure {
  code: SwitchFailureCode;
}

export interface OAuthLoginStart {
  login_id: string;
  auth_url: string;
}

export type OAuthLoginStatus =
  | { status: "pending" }
  | { status: "complete"; account: AccountView }
  | { status: "cancelled" }
  | { status: "failed"; code: OAuthFailureCode };

export type OAuthFailureCode =
  | "not_completed" | "timed_out" | "identity_mismatch"
  | "verification_failed" | "save_failed" | "unavailable";

export interface ImportResult {
  imported: AccountView[];
  duplicate_count: number;
  unsupported_count: number;
  failed_count: number;
}

export interface ExportResult {
  exported_count: number;
  cancelled: boolean;
}

export type MigrationSource = "official_codex" | "cockpit_tools";
export type MigrationCandidateState = "new" | "already_present" | "unsupported";

export interface MigrationCandidate {
  id: string;
  source: MigrationSource;
  email?: string;
  workspace_name?: string;
  plan_type?: string;
  state: MigrationCandidateState;
}

export interface MigrationPreview {
  candidates: MigrationCandidate[];
}

export type QuotaStatus = "fresh" | "stale" | "unknown" | "not_applicable";

export type QuotaRefreshFailureCode =
  | "operation_busy" | "codex_account_unknown" | "authentication"
  | "rate_limited" | "network" | "service" | "invalid_response"
  | "identity_mismatch" | "unavailable";

export interface QuotaRefreshFailure {
  code: QuotaRefreshFailureCode;
}

export interface QuotaWindow {
  kind: "five_hour" | "weekly" | "other";
  used_percent?: number;
  remaining_percent?: number;
  window_duration_mins?: number;
  resets_at?: number;
}

export interface QuotaBucket {
  limit_id: string;
  limit_name?: string;
  plan_type?: string;
  rate_limit_reached_type?: string;
  kind: "codex" | "other";
  windows: QuotaWindow[];
}

export interface ResetCreditDetail {
  expires_at?: number;
}

export interface ResetCreditsView {
  available_count: number;
  nearest_expiry?: number;
  details_available: boolean;
  can_redeem: boolean;
  // Provider credit IDs deliberately never cross the Rust/WebView boundary.
  usable_credits: ResetCreditDetail[];
}

export interface QuotaView {
  account_id: string;
  status: QuotaStatus;
  snapshot?: {
    fetched_at_unix_ms: number;
    account_id?: string;
    ordinary_usage_allowed?: boolean;
    buckets: QuotaBucket[];
    reset_credits?: ResetCreditsView;
  };
  message?: string;
}

export type ResetCreditOutcomeKind =
  | "reset"
  | "already_redeemed"
  | "nothing_to_reset"
  | "no_credit";

export interface ResetCreditOutcome {
  account_id: string;
  outcome: ResetCreditOutcomeKind;
  quota?: QuotaView;
  refresh_warning?: string;
}

export type WakeResultKind =
  | "started"
  | "already_active"
  | "five_hour_exhausted"
  | "weekly_exhausted"
  | "no_ordinary_capacity"
  | "needs_sign_in"
  | "sent_not_confirmed"
  | "quota_unavailable"
  | "request_rejected"
  | "failed"
  | "cancelled";

export interface WakeAccountResult {
  account_id: string;
  label: string;
  result: WakeResultKind;
}

export interface WakeOperationView {
  id: string;
  status: "running" | "completed" | "cancelled" | "failed";
  current_account_id?: string;
  results: WakeAccountResult[];
}

export interface WakeStart {
  operation_id: string;
}
