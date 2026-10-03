import { describe, expect, it } from "vitest";

import {
  ACTIVE_FOCUS_REFRESH_MS,
  ACTIVE_REFRESH_MS,
  OTHER_REFRESH_MS,
  quotaRefreshDue,
} from "./refreshPolicy";
import type { QuotaView } from "./types";

const now = 10_000_000_000;

function quota(fetchedAt: number, resetsAt?: number): QuotaView {
  return {
    account_id: "account",
    status: "fresh",
    snapshot: {
      fetched_at_unix_ms: fetchedAt,
      buckets: [{
        limit_id: "codex",
        kind: "codex",
        windows: [{ kind: "five_hour", remaining_percent: 0, used_percent: 100, resets_at: resetsAt }],
      }],
    },
  };
}

describe("automatic quota refresh policy", () => {
  it("reads an unknown account at once", () => {
    expect(quotaRefreshDue(undefined, { active: false, focused: false, now })).toBe(true);
    expect(quotaRefreshDue({ account_id: "a", status: "unknown" }, { active: false, focused: false, now })).toBe(true);
    expect(quotaRefreshDue({ account_id: "a", status: "not_applicable" }, { active: false, focused: false, now })).toBe(false);
  });

  it("keeps the current account within two minutes, or one minute after focus", () => {
    const options = { active: true, focused: false, now };
    expect(quotaRefreshDue(quota(now - ACTIVE_REFRESH_MS + 1), options)).toBe(false);
    expect(quotaRefreshDue(quota(now - ACTIVE_REFRESH_MS), options)).toBe(true);
    expect(quotaRefreshDue(quota(now - ACTIVE_FOCUS_REFRESH_MS), { ...options, focused: true })).toBe(true);
    expect(quotaRefreshDue(quota(now - ACTIVE_FOCUS_REFRESH_MS + 1), { ...options, focused: true })).toBe(false);
  });

  it("reads another account after thirty minutes or once one of its windows has reset", () => {
    const options = { active: false, focused: true, now };
    expect(quotaRefreshDue(quota(now - OTHER_REFRESH_MS + 1), options)).toBe(false);
    expect(quotaRefreshDue(quota(now - OTHER_REFRESH_MS), options)).toBe(true);
    expect(quotaRefreshDue(quota(now - 60_000, (now - 1_000) / 1000), options)).toBe(true);
    expect(quotaRefreshDue(quota(now - 60_000, (now + 1_000) / 1000), options)).toBe(false);
    expect(quotaRefreshDue(quota(now - 60_000, (now - 120_000) / 1000), options)).toBe(false);
  });

  it("counts from the last attempt so a failing account is not retried on every check", () => {
    const old = quota(now - 2 * OTHER_REFRESH_MS, (now - OTHER_REFRESH_MS) / 1000);
    const options = { active: false, focused: false, failure: "network" as const, now };
    expect(quotaRefreshDue(old, options)).toBe(true);
    expect(quotaRefreshDue(old, { ...options, lastAttemptAt: now - 60_000 })).toBe(false);
  });

  it("leaves failures only the user can fix to the user", () => {
    for (const failure of ["authentication", "manual_refresh_needed", "identity_mismatch"] as const) {
      expect(quotaRefreshDue(quota(0), { active: true, focused: true, failure, now })).toBe(false);
    }
  });

  it("treats a stale reading as unread until an attempt has been made", () => {
    const stale = { ...quota(now - 1_000), status: "stale" as const };
    expect(quotaRefreshDue(stale, { active: false, focused: false, now })).toBe(true);
    expect(quotaRefreshDue(stale, { active: false, focused: false, lastAttemptAt: now - 1_000, now })).toBe(false);
  });

  it("reads again when the saved time is in the future", () => {
    expect(quotaRefreshDue(quota(now + 60_000), { active: false, focused: false, now })).toBe(true);
  });
});
