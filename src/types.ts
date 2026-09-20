export type QuotaStatus = "fresh" | "stale" | "unknown" | "not_applicable";

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
  | "already_active"
  | "window_started"
  | "request_completed_unconfirmed"
  | "needs_model_selection"
  | "failed"
  | "skipped"
  | "cancelled";

export interface WakeAccountResult {
  account_id: string;
  label: string;
  result: WakeResultKind;
  message: string;
  available_models?: string[];
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
