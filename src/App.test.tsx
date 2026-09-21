import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import App from "./App";
import type { AccountView, QuotaView } from "./types";

const mocks = vi.hoisted(() => ({
  runtimeInfo: vi.fn(),
  appSnapshot: vi.fn(),
  listAccounts: vi.fn(),
  resetDamagedAccountStore: vi.fn(),
  liveAccount: vi.fn(),
  startOAuth: vi.fn(),
  oauthStatus: vi.fn(),
  cancelOAuth: vi.fn(),
  openOAuth: vi.fn(),
  importAuthJson: vi.fn(),
  importAuthFile: vi.fn(),
  importApiKey: vi.fn(),
  saveCurrentAccount: vi.fn(),
  enableAccountSwitching: vi.fn(),
  switchAccount: vi.fn(),
  recoverPendingSwitch: vi.fn(),
  removeSavedAccount: vi.fn(),
  accountQuota: vi.fn(),
  refreshAccountQuota: vi.fn(),
  redeemEarliestResetCredit: vi.fn(),
  recoverPendingResetCredit: vi.fn(),
  startWake: vi.fn(),
  startWakeAll: vi.fn(),
  wakeOperation: vi.fn(),
  cancelWake: vi.fn(),
}));

vi.mock("./api", () => ({
  api: mocks,
}));

const chatAccount: AccountView = {
  id: "account-1",
  label: "Personal",
  kind: "chat_gpt",
  email: "person@example.com",
  plan_type: "Plus",
  active: false,
};

const staleQuota: QuotaView = {
  account_id: "account-1",
  status: "stale",
  snapshot: {
    fetched_at_unix_ms: 1735689600000,
    buckets: [
      {
        limit_id: "codex",
        kind: "codex",
        windows: [
          {
            kind: "five_hour",
            remaining_percent: 30,
            used_percent: 70,
            window_duration_mins: 300,
            resets_at: 1893456000,
          },
          {
            kind: "weekly",
            remaining_percent: 85,
            used_percent: 15,
            window_duration_mins: 10080,
            resets_at: 1894060800,
          },
        ],
      },
    ],
    reset_credits: {
      available_count: 2,
      nearest_expiry: 1893456000,
      details_available: true,
      can_redeem: true,
      usable_credits: [{ expires_at: 1893456000 }, { expires_at: 1894060800 }],
    },
  },
};

function prepareDefaults() {
  mocks.runtimeInfo.mockResolvedValue({
    codex_home: "C:\\Codex",
    auth_file_exists: false,
    credential_store: "file",
  });
  mocks.listAccounts.mockResolvedValue([]);
  mocks.liveAccount.mockResolvedValue({
    status: "not_signed_in",
    credential_store: "file",
  });
  mocks.appSnapshot.mockImplementation(async () => ({
    storage: { status: "ready" },
    accounts: await mocks.listAccounts(),
    pending_reset_credit: false,
    runtime: await mocks.runtimeInfo(),
    live: await mocks.liveAccount(),
  }));
  mocks.resetDamagedAccountStore.mockResolvedValue(undefined);
  mocks.accountQuota.mockResolvedValue({
    account_id: "unused",
    status: "unknown",
  });
  mocks.startOAuth.mockResolvedValue({
    login_id: "login-1",
    auth_url: "https://auth.openai.com/example",
  });
  mocks.oauthStatus.mockResolvedValue({ status: "pending" });
  mocks.cancelOAuth.mockResolvedValue(undefined);
  mocks.openOAuth.mockResolvedValue(undefined);
  mocks.importAuthJson.mockResolvedValue(chatAccount);
  mocks.importAuthFile.mockResolvedValue({ imported: [chatAccount], skipped_count: 0 });
  mocks.importApiKey.mockResolvedValue({
    id: "api-1",
    label: "Key",
    kind: "api_key",
    active: false,
  });
  mocks.saveCurrentAccount.mockResolvedValue(chatAccount);
  mocks.enableAccountSwitching.mockResolvedValue(true);
  mocks.switchAccount.mockResolvedValue({ account: chatAccount });
  mocks.recoverPendingSwitch.mockResolvedValue(undefined);
  mocks.removeSavedAccount.mockResolvedValue(undefined);
  mocks.refreshAccountQuota.mockResolvedValue(staleQuota);
  mocks.redeemEarliestResetCredit.mockResolvedValue({
    account_id: "account-1",
    outcome: "reset",
  });
  mocks.recoverPendingResetCredit.mockResolvedValue({
    account_id: "account-1",
    outcome: "nothing_to_reset",
  });
  mocks.startWake.mockResolvedValue({ operation_id: "wake-1" });
  mocks.startWakeAll.mockResolvedValue({ operation_id: "wake-all" });
  mocks.wakeOperation.mockResolvedValue({
    id: "wake-1",
    status: "completed",
    results: [],
  });
  mocks.cancelWake.mockResolvedValue(undefined);
}

describe("GSwitch account workspace", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    prepareDefaults();
  });

  it("guides a first-time user to an explicit import or manual add", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "0 saved accounts" })).toBeInTheDocument();
    expect(screen.getByText(/never reads another app/i)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Add manually" }));
    expect(await screen.findByRole("dialog", { name: "Add a Codex account" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Paste auth JSON/ })).toBeInTheDocument();
    expect(screen.getByText(/does not access Cockpit Tools/i)).toBeInTheDocument();
  });

  it("contains a damaged account library until the user explicitly resets only GSwitch storage", async () => {
    mocks.appSnapshot.mockResolvedValue({
      storage: {
        status: "recovery_required",
        message: "GSwitch could not safely read its saved account library. Codex credentials were not changed.",
      },
      accounts: [],
      pending_reset_credit: false,
    });
    render(<App />);

    expect(await screen.findByText("Saved accounts are protected until recovery")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add account" })).toBeDisabled();
    expect(screen.queryByText("Add your first Codex account")).not.toBeInTheDocument();

    await userEvent.click(screen.getAllByRole("button", { name: "Review recovery" })[0]);
    expect(await screen.findByRole("dialog", { name: "Recover GSwitch account storage" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(mocks.resetDamagedAccountStore).not.toHaveBeenCalled();
    expect(screen.getByText("Reset only GSwitch's saved account library?")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Reset GSwitch storage" }));
    await waitFor(() => expect(mocks.resetDamagedAccountStore).toHaveBeenCalledOnce());
  });

  it("clears pasted auth JSON before the import request completes", async () => {
    let resolveImport: ((account: AccountView) => void) | undefined;
    mocks.importAuthJson.mockImplementation(
      () =>
        new Promise<AccountView>((resolve) => {
          resolveImport = resolve;
        }),
    );
    render(<App />);
    await screen.findByRole("heading", { name: "0 saved accounts" });

    await userEvent.click(screen.getByRole("button", { name: /^Add account$/ }));
    const dialog = await screen.findByRole("dialog", { name: "Add a Codex account" });
    await userEvent.click(within(dialog).getByRole("button", { name: /Paste auth JSON/ }));
    const input = screen.getByLabelText("auth.json");
    fireEvent.change(input, { target: { value: '{"tokens":{"access_token":"secret"}}' } });
    await userEvent.click(within(dialog).getByRole("button", { name: /^Add account$/ }));

    expect(input).toHaveValue("");
    expect(mocks.importAuthJson).toHaveBeenCalledWith(
      '{"tokens":{"access_token":"secret"}}',
      undefined,
    );

    resolveImport?.(chatAccount);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("renders stale quota and requires a second explicit reset-credit confirmation", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    render(<App />);

    expect(await screen.findByText("Personal")).toBeInTheDocument();
    expect(screen.getByText("Stale")).toBeInTheDocument();
    expect(screen.getByText("30%")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Details" }));
    expect(await screen.findByRole("dialog", { name: /Reset credits/i })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Use reset credit" }));
    expect(mocks.redeemEarliestResetCredit).not.toHaveBeenCalled();
    expect(screen.getByText("Use the earliest eligible reset credit?")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Use earliest credit" }));
    await waitFor(() =>
      expect(mocks.redeemEarliestResetCredit).toHaveBeenCalledWith("account-1"),
    );
  });

  it("requires a deliberate recovery of the original pending reset request", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    mocks.appSnapshot.mockImplementation(async () => ({
      storage: { status: "ready" },
      accounts: await mocks.listAccounts(),
      pending_reset_credit: true,
      runtime: await mocks.runtimeInfo(),
      live: await mocks.liveAccount(),
    }));
    render(<App />);

    expect(await screen.findByText("A reset-credit request needs recovery")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Review reset recovery" }));
    expect(await screen.findByRole("dialog", { name: "Recover reset credit" })).toBeInTheDocument();
    expect(mocks.recoverPendingResetCredit).not.toHaveBeenCalled();
    expect(mocks.redeemEarliestResetCredit).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Recover original request" }));
    await waitFor(() => expect(mocks.recoverPendingResetCredit).toHaveBeenCalledOnce());
    expect(mocks.redeemEarliestResetCredit).not.toHaveBeenCalled();
  });

  it("keeps account actions available when a local quota projection cannot be read", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockRejectedValue(new Error("quota cache unavailable"));
    render(<App />);

    expect(await screen.findByText("Personal")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Switch" })).toBeEnabled();
    expect(screen.getAllByText("Not available")).toHaveLength(2);
  });

  it("lets the user cancel a browser OAuth flow", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "0 saved accounts" });

    await userEvent.click(screen.getByRole("button", { name: /^Add account$/ }));
    const dialog = await screen.findByRole("dialog", { name: "Add a Codex account" });
    await userEvent.click(within(dialog).getByRole("button", { name: /Sign in with your browser/ }));
    expect(await screen.findByText("Finish sign-in in your browser")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Cancel sign-in" }));
    await waitFor(() => expect(mocks.cancelOAuth).toHaveBeenCalledWith("login-1"));
    expect(await screen.findByText("Sign-in cancelled")).toBeInTheDocument();
  });

  it("shows a per-operation Wake surface instead of silently running in the background", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    render(<App />);
    await screen.findByText("Personal");

    await userEvent.click(screen.getByRole("button", { name: /^Wake all$/ }));
    expect(await screen.findByRole("dialog", { name: "Wake" })).toBeInTheDocument();
    expect(screen.getByText("Wake is running safely")).toBeInTheDocument();
    expect(mocks.startWakeAll).toHaveBeenCalledOnce();
  });
});
