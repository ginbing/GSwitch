import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAccountQuota } from "./useAccountQuota";
import type { AccountView, QuotaView } from "./types";

const mocks = vi.hoisted(() => ({ accountQuota: vi.fn(), refreshAccountQuota: vi.fn() }));
vi.mock("./api", () => ({ api: mocks }));
const accounts: AccountView[] = [{ id: "synthetic", kind: "chat_gpt", label: "Synthetic", active: false, revision: 1 }];
function quota(time = Date.now(), count = 1): QuotaView {
  return { account_id: "synthetic", status: "fresh", snapshot: { fetched_at_unix_ms: time, buckets: [], reset_credits: { available_count: count, details_available: true, can_redeem: false, usable_credits: [] } } };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.resetAllMocks();
  mocks.accountQuota.mockResolvedValue(quota());
  mocks.refreshAccountQuota.mockResolvedValue(quota());
});
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe("quota acceptance owner", () => {
  it.each([false, true])("a newer cache defeats an older point read or its error (failure=%s)", async failure => {
    const cache = deferred<QuotaView>();
    const refresh = deferred<QuotaView>();
    mocks.accountQuota.mockReturnValue(cache.promise);
    mocks.refreshAccountQuota.mockReturnValue(refresh.promise);
    const { result } = renderHook(() => useAccountQuota({ accounts, enabled: true }));
    let request!: Promise<unknown>;
    act(() => { request = result.current.refresh("synthetic").catch(() => undefined); });
    const newer = quota(Date.now() + 100, 9);
    await act(async () => { cache.resolve(newer); await cache.promise; });
    await act(async () => {
      if (failure) refresh.reject({ code: "authentication" });
      else refresh.resolve(quota(newer.snapshot!.fetched_at_unix_ms - 50, 2));
      await request;
    });
    expect(result.current.quotas.synthetic).toEqual(newer);
    expect(result.current.failures.synthetic).toBeUndefined();
  });

  it.each([false, true])("reset invalidates even a late result with a later clock (failure=%s)", async failure => {
    const pending = deferred<QuotaView>();
    mocks.refreshAccountQuota.mockReturnValue(pending.promise);
    const { result } = renderHook(() => useAccountQuota({ accounts, enabled: true }));
    await waitFor(() => expect(result.current.quotas.synthetic).toBeDefined());
    let request!: Promise<unknown>;
    act(() => { request = result.current.refresh("synthetic").catch(() => undefined); });
    const reset = quota(Date.now() + 100, 8);
    act(() => result.current.applyReset("synthetic", reset));
    await act(async () => {
      if (failure) pending.reject({ code: "service" });
      else pending.resolve(quota(reset.snapshot!.fetched_at_unix_ms + 1000, 0));
      await request;
    });
    expect(result.current.quotas.synthetic).toEqual(reset);
    expect(result.current.failures.synthetic).toBeUndefined();
  });

  it("removal and re-adding the same ID cannot admit a previous membership's response", async () => {
    const pending = deferred<QuotaView>();
    mocks.refreshAccountQuota.mockReturnValue(pending.promise);
    const { result, rerender } = renderHook(({ saved }) => useAccountQuota({ accounts: saved, enabled: true }), { initialProps: { saved: accounts } });
    await waitFor(() => expect(result.current.quotas.synthetic).toBeDefined());
    let request!: Promise<unknown>;
    act(() => { request = result.current.refresh("synthetic").catch(() => undefined); });
    rerender({ saved: [] });
    expect(result.current.quotas).toEqual({});
    const current = quota(Date.now(), 7);
    mocks.accountQuota.mockResolvedValue(current);
    rerender({ saved: accounts });
    await waitFor(() => expect(result.current.quotas.synthetic).toEqual(current));
    await act(async () => { pending.resolve(quota(Date.now() + 1000, 0)); await request; });
    expect(result.current.quotas.synthetic).toEqual(current);
    expect(result.current.failures).toEqual({});
  });

  it("a new credential revision clears old authentication failures and rejects its old cache", async () => {
    const oldCache = deferred<QuotaView>();
    mocks.accountQuota.mockReturnValueOnce(oldCache.promise);
    mocks.refreshAccountQuota.mockRejectedValueOnce({ code: "authentication" });
    const { result, rerender } = renderHook(({ saved }) => useAccountQuota({ accounts: saved, enabled: true }), { initialProps: { saved: accounts } });
    await act(async () => { await result.current.refresh("synthetic").catch(() => undefined); });
    expect(result.current.failures.synthetic).toBe("authentication");
    const current = quota(Date.now(), 7);
    mocks.accountQuota.mockResolvedValue(current);
    rerender({ saved: [{ ...accounts[0], revision: 2 }] });
    await waitFor(() => expect(result.current.quotas.synthetic).toEqual(current));
    await act(async () => { oldCache.resolve(quota(Date.now() + 1000, 0)); await oldCache.promise; });
    expect(result.current.quotas.synthetic).toEqual(current);
    expect(result.current.failures).toEqual({});
  });

  it("a removed queued account never starts its provider read", async () => {
    const first = deferred<QuotaView>();
    const second = { ...accounts[0], id: "second" };
    const saved = [...accounts, second];
    mocks.accountQuota.mockImplementation(async (id: string) => ({ account_id: id, status: "unknown" }));
    mocks.refreshAccountQuota.mockReturnValue(first.promise);
    const { rerender } = renderHook(({ saved }) => useAccountQuota({ accounts: saved, enabled: true }), { initialProps: { saved } });
    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledTimes(1));
    rerender({ saved: accounts });
    await act(async () => { first.resolve(quota()); await first.promise; });
    expect(mocks.refreshAccountQuota.mock.calls.map(call => call[0])).toEqual(["synthetic"]);
  });

  it("defers a post-login read until its new revision arrives, then coalesces it", async () => {
    const { result, rerender } = renderHook(({ saved }) => useAccountQuota({ accounts: saved, enabled: true }), { initialProps: { saved: accounts } });
    await waitFor(() => expect(result.current.quotas.synthetic).toBeDefined());
    act(() => result.current.refreshAfterChange({ id: "synthetic", revision: 2 }));
    expect(mocks.refreshAccountQuota).not.toHaveBeenCalled();
    rerender({ saved: [{ ...accounts[0], revision: 2 }] });
    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledTimes(1));
  });

  it("disabling and re-enabling cannot accept an old outstanding read", async () => {
    const pending = deferred<QuotaView>();
    mocks.refreshAccountQuota.mockReturnValue(pending.promise);
    const { result, rerender } = renderHook(({ enabled }) => useAccountQuota({ accounts, enabled }), { initialProps: { enabled: true } });
    await waitFor(() => expect(result.current.quotas.synthetic).toBeDefined());
    const current = result.current.quotas.synthetic;
    let request!: Promise<unknown>;
    act(() => { request = result.current.refresh("synthetic").catch(() => undefined); });
    rerender({ enabled: false }); rerender({ enabled: true });
    await act(async () => { pending.resolve(quota(Date.now() + 1000, 0)); await request; });
    expect(result.current.quotas.synthetic).toEqual(current);
  });
});
