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

export interface QuotaView {
  account_id: string;
  status: QuotaStatus;
  snapshot?: {
    fetched_at_unix_ms: number;
    account_id?: string;
    ordinary_usage_allowed?: boolean;
    buckets: QuotaBucket[];
  };
  message?: string;
}
