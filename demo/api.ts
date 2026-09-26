import type { AccountView, AppSnapshot, QuotaView } from "../src/types";

// This file is loaded only by `vite --mode demo`. It contains display data,
// never imports Tauri, and rejects every account mutation.
if ("__TAURI_INTERNALS__" in window) {
  throw new Error("The README demo cannot run inside Tauri.");
}

const fixedNow = new Date(2026, 8, 26, 10, 0).getTime();
Date.now = () => fixedNow;
const at = (day: number, hour: number, minute = 0) => Math.floor(new Date(2026, 8, day, hour, minute).getTime() / 1000);

const accounts: AccountView[] = [
  { id: "demo-1", label: "Studio", kind: "chat_gpt", email: "alex@example.com", workspace_name: "Studio", plan_type: "team", active: true },
  { id: "demo-2", label: "Studio", kind: "chat_gpt", email: "riley@example.org", workspace_name: "Studio", plan_type: "team", active: false },
  { id: "demo-3", label: "Research", kind: "chat_gpt", email: "sam@example.net", workspace_name: "Research", plan_type: "team", active: false },
  { id: "demo-4", label: "Personal", kind: "chat_gpt", email: "taylor@example.com", workspace_name: "Personal", plan_type: "plus", active: false },
  { id: "demo-5", label: "Research", kind: "chat_gpt", email: "morgan@example.org", workspace_name: "Research", plan_type: "team", active: false },
  { id: "demo-6", label: "Personal", kind: "chat_gpt", email: "casey@example.net", workspace_name: "Personal", plan_type: "free", active: false },
];

const quotaValues = [
  [72, 38], [44, 81], [100, 16], [23, 61], [89, 7], [68],
];
const quotas = Object.fromEntries(accounts.map((account, index) => {
  const values = quotaValues[index]!;
  const windows: NonNullable<QuotaView["snapshot"]>["buckets"][number]["windows"] = index === 5
    ? [{ kind: "other", window_duration_mins: 43_200, remaining_percent: values[0], used_percent: 100 - values[0]!, resets_at: at(30, 16, 30) }]
    : [
      { kind: "five_hour", window_duration_mins: 300, remaining_percent: values[0], used_percent: 100 - values[0]!, resets_at: at(26, 12, 30) },
      { kind: "weekly", window_duration_mins: 10_080, remaining_percent: values[1], used_percent: 100 - values[1]!, resets_at: at(29, 9, 45) },
    ];
  const quota: QuotaView = {
    account_id: account.id,
    status: "fresh",
    snapshot: {
      fetched_at_unix_ms: fixedNow,
      buckets: [{ limit_id: "codex", kind: "codex", windows }],
      reset_credits: { available_count: index === 5 ? 0 : 2, nearest_expiry: at(30, 16, 30), details_available: true, can_redeem: index !== 5, usable_credits: [] },
    },
  };
  return [account.id, quota];
})) as Record<string, QuotaView>;

const snapshot: AppSnapshot = {
  storage: { status: "ready" },
  accounts,
  pending_reset_credit: false,
  live: { status: "ready", credential_store: "file", account: accounts[0] },
};

const displayOnly = {
  appSnapshot: async () => snapshot,
  accountQuota: async (id: string) => quotas[id] || { account_id: id, status: "unknown" },
  codexCliInfo: async () => ({ version: "0.158.0", latest_version: "0.158.0", supports_update: true, update_status: "current" as const }),
};

export const api = new Proxy(displayOnly, {
  get(target, property, receiver) {
    if (Reflect.has(target, property)) return Reflect.get(target, property, receiver);
    throw new Error(`The README demo forbids operation ${String(property)}.`);
  },
}) as typeof import("../src/api").api;
export type UpdateDelivery = import("../src/api").UpdateDelivery;
