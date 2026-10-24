import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "./api";
import { RefreshLanes } from "./refreshLanes";
import { quotaRefreshDue } from "./refreshPolicy";
import type { AccountView, QuotaRefreshFailureCode, QuotaView } from "./types";

interface Membership { revision: number; epoch: number; accepted: number }
interface Request { promise: Promise<QuotaView>; manual: boolean; started: boolean; epoch: number; accepted: number }
interface View { quotas: Readonly<Record<string, QuotaView>>; failures: Readonly<Record<string, QuotaRefreshFailureCode>> }
type Input = { accounts: AccountView[]; activeId?: string; enabled: boolean };
type Incoming = { id: string; epoch: number } & (
  | { kind: "cache" | "reset"; quota: QuotaView }
  | { kind: "refresh"; quota: QuotaView; request: Request }
  | { kind: "failure"; error: unknown; request: Request }
);
const failureCodes = new Set<QuotaRefreshFailureCode>([
  "operation_busy", "codex_account_unknown", "authentication", "rate_limited",
  "manual_refresh_needed", "network", "service", "invalid_response", "identity_mismatch", "unavailable",
]);
function failureCode(error: unknown): QuotaRefreshFailureCode {
  const code = typeof error === "object" && error !== null && "code" in error ? error.code : undefined;
  return typeof code === "string" && failureCodes.has(code as QuotaRefreshFailureCode) ? code as QuotaRefreshFailureCode : "unavailable";
}
const cancelled = () => ({ code: "unavailable" });

export function useAccountQuota({ accounts, activeId, enabled }: Input) {
  const [view, setView] = useState<View>({ quotas: {}, failures: {} });
  const held = useRef(view);
  const [now, setNow] = useState(() => Date.now());
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [changedAccounts, setChangedAccounts] = useState<Record<string, number | undefined>>({});
  const members = useRef(new Map<string, Membership>());
  const requests = useRef(new Map<string, Request>());
  const lanes = useRef(new RefreshLanes({ background: 1, manual: 3, signIn: 1 }));
  const lastAttempt = useRef(new Map<string, number>());
  const inputs = useRef({ accounts, activeId, enabled });
  const mounted = useRef(true);
  const epoch = useRef(0);
  const batch = useRef(false);
  useLayoutEffect(() => { inputs.current = { accounts, activeId, enabled }; }, [accounts, activeId, enabled]);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  // All cached, refreshed and reset projections enter here. The refs point at
  // the same transient view React renders; no second account/quota store exists.
  const receive = useCallback((incoming: Incoming) => {
    const member = members.current.get(incoming.id);
    if (!mounted.current || !member || member.epoch !== incoming.epoch) return;
    if ("request" in incoming && requests.current.get(incoming.id) !== incoming.request) return;
    const current = held.current;
    const previous = current.quotas[incoming.id];
    if (incoming.kind === "failure") {
      if (member.accepted !== incoming.request.accepted) return;
      const next: View = {
        quotas: { ...current.quotas, [incoming.id]: { account_id: incoming.id, status: previous?.snapshot ? "stale" : "unknown", snapshot: previous?.snapshot } },
        failures: { ...current.failures, [incoming.id]: failureCode(incoming.error) },
      };
      held.current = next; setView(next);
      return;
    }
    if (incoming.quota.account_id !== incoming.id) return;
    const timestamp = incoming.quota.snapshot?.fetched_at_unix_ms ?? 0;
    const previousTime = previous?.snapshot?.fetched_at_unix_ms ?? 0;
    if (timestamp < previousTime || (incoming.kind === "cache" && previous && timestamp === previousTime)) return;
    const failures = { ...current.failures };
    // An empty initial cache is not a newer successful read.
    if (incoming.kind !== "cache" || timestamp > previousTime) {
      delete failures[incoming.id];
      member.accepted++;
    }
    const next = { quotas: { ...current.quotas, [incoming.id]: incoming.quota }, failures };
    held.current = next; setView(next);
  }, []);

  const invalidate = useCallback((id: string) => {
    const member = members.current.get(id);
    if (!member) return;
    member.epoch = ++epoch.current;
    requests.current.delete(id);
    lastAttempt.current.delete(id);
  }, []);

  useLayoutEffect(() => {
    const retained = new Set(accounts.filter(account => account.kind === "chat_gpt").map(account => account.id));
    const changed = new Set<string>();
    for (const id of members.current.keys()) {
      if (!retained.has(id)) { members.current.delete(id); requests.current.delete(id); lastAttempt.current.delete(id); }
    }
    for (const account of accounts) {
      if (account.kind !== "chat_gpt") continue;
      const previous = members.current.get(account.id);
      if (!previous || previous.revision !== (account.revision ?? 0)) {
        if (previous) changed.add(account.id);
        members.current.set(account.id, { revision: account.revision ?? 0, epoch: ++epoch.current, accepted: 0 });
        requests.current.delete(account.id); lastAttempt.current.delete(account.id);
      }
    }
    if (!enabled) for (const member of members.current.values()) member.epoch = ++epoch.current;
    if (!enabled) requests.current.clear();
    const next: View = {
      quotas: Object.fromEntries(Object.entries(held.current.quotas).filter(([id]) => retained.has(id)).map(([id, quota]): [string, QuotaView] => [id, changed.has(id) ? { ...quota, status: quota.snapshot ? "stale" : "unknown" } : quota])),
      failures: Object.fromEntries(Object.entries(held.current.failures).filter(([id]) => retained.has(id) && !changed.has(id))),
    };
    held.current = next; setView(next);
  }, [accounts, enabled]);

  const refresh = useCallback((id: string, background = false): Promise<QuotaView> => {
    const member = members.current.get(id);
    if (!member || !inputs.current.enabled || !mounted.current) return Promise.reject(cancelled());
    const current = requests.current.get(id);
    if (current && (background || current.started || current.manual)) return current.promise;
    const request: Request = { manual: !background, started: false, epoch: member.epoch, accepted: member.accepted, promise: Promise.resolve() as never };
    const valid = () => mounted.current && inputs.current.enabled && members.current.get(id)?.epoch === request.epoch && requests.current.get(id) === request;
    const read = () => {
      if (!valid()) return Promise.reject(cancelled());
      request.started = true; lastAttempt.current.set(id, Date.now());
      return api.refreshAccountQuota(id, true);
    };
    // Register before an available lane starts its synchronous task body.
    requests.current.set(id, request);
    const attempt = background
      ? lanes.current.run("background", () => {
        const replacement = requests.current.get(id);
        return replacement && replacement !== request ? replacement.promise : read();
      })
      : lanes.current.run("manual", read).catch((error: unknown) => {
        if (failureCode(error) !== "manual_refresh_needed" || !valid()) throw error;
        return lanes.current.run("signIn", () => valid() ? api.refreshAccountQuota(id, false) : Promise.reject(cancelled()));
      });
    request.promise = attempt.then(
      quota => { receive({ kind: "refresh", id, epoch: request.epoch, request, quota }); return quota; },
      error => { receive({ kind: "failure", id, epoch: request.epoch, request, error }); throw error; },
    );
    const forget = () => {
      if (requests.current.get(id) === request) {
        if (request.started) lastAttempt.current.set(id, Date.now());
        requests.current.delete(id);
      }
    };
    void request.promise.then(forget, forget);
    return request.promise;
  }, [receive]);

  const check = useCallback((focused: boolean, candidates?: ReadonlySet<string>) => {
    if (!inputs.current.enabled || document.visibilityState === "hidden") return;
    const time = Date.now(); setNow(time);
    for (const account of inputs.current.accounts) {
      if (account.kind !== "chat_gpt") continue;
      if (candidates && !candidates.has(account.id)) continue;
      if (quotaRefreshDue(held.current.quotas[account.id], {
        active: account.active || account.id === inputs.current.activeId, focused,
        failure: held.current.failures[account.id], lastAttemptAt: lastAttempt.current.get(account.id), now: time,
      })) void refresh(account.id, true).catch(() => undefined);
    }
  }, [refresh]);

  useEffect(() => {
    if (!enabled) return;
    let closed = false;
    const cachedAccounts = new Set<string>();
    const cached = accounts.filter(account => account.kind === "chat_gpt").map(async account => {
      const token = members.current.get(account.id)?.epoch;
      const quota = await api.accountQuota(account.id);
      if (!closed && token !== undefined && token === members.current.get(account.id)?.epoch) {
        receive({ kind: "cache", id: account.id, epoch: token, quota });
        cachedAccounts.add(account.id);
      }
    });
    void Promise.allSettled(cached).then(() => { if (!closed) check(false, cachedAccounts); });
    return () => { closed = true; };
  }, [accounts, enabled, check, receive]);

  useEffect(() => {
    if (!enabled) return;
    const interval = window.setInterval(() => check(false), 30_000);
    const focus = () => check(true);
    const visibility = () => { if (document.visibilityState === "visible") check(true); };
    window.addEventListener("focus", focus);
    document.addEventListener("visibilitychange", visibility);
    return () => { window.clearInterval(interval); window.removeEventListener("focus", focus); document.removeEventListener("visibilitychange", visibility); };
  }, [enabled, check]);

  const refreshAfterChange = useCallback((account: Pick<AccountView, "id" | "revision"> | string) => {
    const id = typeof account === "string" ? account : account.id;
    const revision = typeof account === "string" ? undefined : account.revision ?? 0;
    setChangedAccounts(current => ({ ...current, [id]: revision }));
  }, []);

  useEffect(() => {
    if (!enabled) return;
    const ready = Object.entries(changedAccounts).filter(([id, revision]) => {
      const member = members.current.get(id);
      return member && (revision === undefined || member.revision === revision);
    });
    if (!ready.length) return;
    for (const [id] of ready) void refresh(id, true).catch(() => undefined);
    setChangedAccounts(current => Object.fromEntries(Object.entries(current).filter(([id]) => !ready.some(([readyId]) => readyId === id))));
  }, [accounts, enabled, changedAccounts, refresh]);

  const applyReset = useCallback((id: string, quota?: QuotaView) => {
    invalidate(id);
    const member = members.current.get(id);
    if (quota && member) receive({ kind: "reset", id, epoch: member.epoch, quota });
  }, [invalidate, receive]);

  const refreshAll = useCallback(async () => {
    if (batch.current) return { failures: 0 };
    const targets = inputs.current.accounts.filter(account => account.kind === "chat_gpt");
    if (!targets.length) return { failures: 0 };
    batch.current = true;
    setProgress({ done: 0, total: targets.length });
    try {
      const results = await Promise.allSettled(targets.map(account => refresh(account.id).finally(() => {
        if (mounted.current) setProgress(current => current && { ...current, done: current.done + 1 });
      })));
      return { failures: results.filter(result => result.status === "rejected").length };
    } finally { batch.current = false; if (mounted.current) setProgress(null); }
  }, [refresh]);

  return { quotas: view.quotas, failures: view.failures, now, progress, refresh, refreshAll, refreshAfterChange, invalidate, applyReset };
}
