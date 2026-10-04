import type { QuotaRefreshFailureCode, QuotaView } from "./types";

// The account Codex is using is the only one whose quota keeps moving.
export const ACTIVE_REFRESH_MS = 2 * 60_000;
export const ACTIVE_FOCUS_REFRESH_MS = 60_000;
// Other accounts change only when used elsewhere or when a window resets.
export const OTHER_REFRESH_MS = 30 * 60_000;

// Automatic reads cannot fix these; they wait for the user's own action.
const WAITS_FOR_USER = new Set<QuotaRefreshFailureCode>([
  "authentication",
  "manual_refresh_needed",
  "identity_mismatch",
]);

export interface RefreshDueOptions {
  active: boolean;
  // The window just regained focus.
  focused: boolean;
  failure?: QuotaRefreshFailureCode;
  // When GSwitch last asked for this account's quota, successful or not.
  lastAttemptAt?: number;
  now: number;
}

// Decides whether an automatic read is worth making: the quota is unknown, it
// is old for its role, or a quota window has reset since the last read.
export function quotaRefreshDue(quota: QuotaView | undefined, options: RefreshDueOptions) {
  if (quota?.status === "not_applicable") {
    return false;
  }
  if (options.failure && WAITS_FOR_USER.has(options.failure)) {
    return false;
  }
  // A stale reading (marked after a reset, or kept after a failed read) counts
  // as unread; the last attempt still spaces out retries.
  const readAt = quota?.status === "stale" ? 0 : quota?.snapshot?.fetched_at_unix_ms ?? 0;
  const since = Math.max(readAt, options.lastAttemptAt ?? 0);
  if (since === 0 || since > options.now) {
    return true;
  }
  const limit = options.active
    ? options.focused ? ACTIVE_FOCUS_REFRESH_MS : ACTIVE_REFRESH_MS
    : OTHER_REFRESH_MS;
  if (options.now - since >= limit) {
    return true;
  }
  return (quota?.snapshot?.buckets ?? []).some((bucket) =>
    bucket.windows.some((window) => {
      const resetsAt = window.resets_at === undefined ? undefined : window.resets_at * 1000;
      return resetsAt !== undefined && resetsAt > since && resetsAt <= options.now;
    }),
  );
}
