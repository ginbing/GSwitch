import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { open } from "@tauri-apps/plugin-dialog";

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
  importAuthFiles: vi.fn(),
  discoverLocalAccounts: vi.fn(),
  importLocalAccounts: vi.fn(),
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
  updateDelivery: vi.fn(),
  openLatestRelease: vi.fn(),
}));

const updaterMocks = vi.hoisted(() => ({
  check: vi.fn(),
  relaunch: vi.fn(),
}));

const webviewMocks = vi.hoisted(() => ({
  onDragDropEvent: vi.fn(),
}));

vi.mock("./api", () => ({
  api: mocks,
}));

vi.mock("./updater", () => ({
  updater: updaterMocks,
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: webviewMocks.onDragDropEvent }),
}));

const chatAccount: AccountView = {
  id: "account-1",
  label: "Personal",
  kind: "chat_gpt",
  email: "person@example.com",
  workspace_name: "Personal",
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
  mocks.importAuthFiles.mockResolvedValue({
    imported: [chatAccount],
    duplicate_count: 0,
    unsupported_count: 0,
    failed_count: 0,
  });
  mocks.discoverLocalAccounts.mockResolvedValue({ candidates: [] });
  mocks.importLocalAccounts.mockResolvedValue({
    imported: [chatAccount],
    duplicate_count: 0,
    unsupported_count: 0,
    failed_count: 0,
  });
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
  mocks.updateDelivery.mockResolvedValue("installer_exits");
  mocks.openLatestRelease.mockResolvedValue(undefined);
  updaterMocks.check.mockResolvedValue(null);
  updaterMocks.relaunch.mockResolvedValue(undefined);
  webviewMocks.onDragDropEvent.mockResolvedValue(() => undefined);
}

describe("GSwitch account workspace", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    Object.defineProperty(window.navigator, "language", { configurable: true, value: "en-US" });
    prepareDefaults();
  });

  afterEach(() => {
    delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it("uses the Simplified Chinese system locale and keeps the main account flow localized", async () => {
    Object.defineProperty(window.navigator, "language", { configurable: true, value: "zh-CN" });
    render(<App />);

    expect(await screen.findByRole("heading", { name: "已保存 0 个账户" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "添加账户" }));
    expect(await screen.findByRole("dialog", { name: "添加 Codex 账户" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /粘贴 auth JSON/ })).toBeInTheDocument();
  });

  it("applies and remembers a manual language choice immediately", async () => {
    const first = render(<App />);
    await screen.findByRole("heading", { name: "0 saved accounts" });

    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));
    await userEvent.selectOptions(screen.getByLabelText("Language"), "zh-CN");
    expect(screen.getByRole("button", { name: "添加账户" })).toBeInTheDocument();
    expect(window.localStorage.getItem("gswitch.language")).toBe("zh-CN");

    first.unmount();
    render(<App />);
    expect(await screen.findByRole("heading", { name: "已保存 0 个账户" })).toBeInTheDocument();
  });

  it("guides a first-time user to an explicit import or manual add", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "0 saved accounts" })).toBeInTheDocument();
    expect(screen.getByText(/does not inspect another application's account storage automatically/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import account files" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Add manually" }));
    const dialog = await screen.findByRole("dialog", { name: "Add a Codex account" });
    expect(dialog).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /Paste auth JSON/ })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /Cockpit Tools exports, Codex auth\.json/i })).toBeInTheDocument();
    expect(within(dialog).getByText(/export account files from Cockpit Tools/i)).toBeInTheDocument();
    expect(within(dialog).getByText(/does not inspect another application's account storage automatically/i)).toBeInTheDocument();
  });

  it("does not scan local account sources at startup or when the method row opens", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "0 saved accounts" })).toBeInTheDocument();
    expect(mocks.discoverLocalAccounts).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Add manually" }));
    await userEvent.click(screen.getByRole("button", { name: /Import from this computer/ }));
    expect(screen.getByText(/Before scanning, GSwitch will read only/i)).toBeInTheDocument();
    expect(mocks.discoverLocalAccounts).not.toHaveBeenCalled();
  });

  it("shows a sanitized migration preview and imports only selected new candidates", async () => {
    mocks.discoverLocalAccounts.mockResolvedValue({
      candidates: [
        {
          id: "new-id",
          source: "official_codex",
          email: "person@example.com",
          workspace_name: "Personal",
          plan_type: "Plus",
          state: "new",
        },
        {
          id: "existing-id",
          source: "cockpit_tools",
          email: "saved@example.com",
          state: "already_present",
        },
        {
          id: "unsupported-id",
          source: "cockpit_tools",
          email: "unknown@example.com",
          state: "unsupported",
        },
      ],
    });
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Add manually" }));
    await userEvent.click(screen.getByRole("button", { name: /Import from this computer/ }));
    await userEvent.click(screen.getByRole("button", { name: "Scan supported locations" }));

    expect(await screen.findByText("person@example.com")).toBeInTheDocument();
    expect(screen.getByText(/Official Codex · New/)).toBeInTheDocument();
    expect(screen.getByText(/Already in GSwitch/)).toBeInTheDocument();
    expect(screen.getByText(/Unsupported/)).toBeInTheDocument();
    expect(screen.getAllByRole("checkbox")).toHaveLength(3);
    expect(screen.getAllByRole("checkbox")[0]).toBeChecked();
    expect(screen.getAllByRole("checkbox")[1]).toBeDisabled();
    expect(screen.getAllByRole("checkbox")[2]).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: "Import selected" }));
    await waitFor(() => expect(mocks.importLocalAccounts).toHaveBeenCalledWith(undefined, ["new-id"]));
  });

  it("uses the explicit folder choice only for a migration scan", async () => {
    vi.mocked(open).mockResolvedValue("C:\\custom\\cockpit");
    mocks.discoverLocalAccounts.mockResolvedValue({ candidates: [] });
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Add manually" }));
    await userEvent.click(screen.getByRole("button", { name: /Import from this computer/ }));
    await userEvent.click(screen.getByRole("button", { name: "Choose another Cockpit folder" }));

    await waitFor(() => expect(mocks.discoverLocalAccounts).toHaveBeenCalledWith("C:\\custom\\cockpit"));
    expect(open).toHaveBeenCalledWith(expect.objectContaining({ directory: true, multiple: false }));
  });

  it("opens the native picker for multiple account files and reports one summary", async () => {
    vi.mocked(open).mockResolvedValue(["C:\\exports\\one.json", "C:\\exports\\two.json"]);
    mocks.importAuthFiles.mockResolvedValue({
      imported: [chatAccount, { ...chatAccount, id: "account-2", email: "other@example.com" }],
      duplicate_count: 1,
      unsupported_count: 1,
      failed_count: 1,
    });
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Import account files" }));
    await waitFor(() =>
      expect(mocks.importAuthFiles).toHaveBeenCalledWith([
        "C:\\exports\\one.json",
        "C:\\exports\\two.json",
      ]),
    );
    expect(open).toHaveBeenCalledWith(expect.objectContaining({ multiple: true }));
    expect(await screen.findByText("Imported 2 account(s); 1 already present or duplicate; 2 unsupported or failed.")).toBeInTheDocument();
  });

  it("uses the same bounded batch command for one selected file", async () => {
    vi.mocked(open).mockResolvedValue(["C:\\exports\\one.json"]);
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Import account files" }));
    await waitFor(() => expect(mocks.importAuthFiles).toHaveBeenCalledWith(["C:\\exports\\one.json"]));
  });

  it("keeps a saved batch account when its later quota refresh fails", async () => {
    const imported = { ...chatAccount, id: "batch-account" };
    vi.mocked(open).mockResolvedValue(["C:\\exports\\one.json"]);
    mocks.importAuthFiles.mockImplementation(async () => {
      mocks.listAccounts.mockResolvedValue([imported]);
      return {
        imported: [imported],
        duplicate_count: 0,
        unsupported_count: 0,
        failed_count: 0,
      };
    });
    mocks.accountQuota.mockResolvedValue({ account_id: imported.id, status: "unknown" });
    mocks.refreshAccountQuota.mockRejectedValue(new Error("quota cache unavailable"));
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Import account files" }));
    expect(await screen.findByRole("heading", { name: "person@example.com" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Switch to person@example.com" })).toBeEnabled();
  });

  it("passes every dropped path to the bounded batch command", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    render(<App />);

    await waitFor(() => expect(webviewMocks.onDragDropEvent).toHaveBeenCalledOnce());
    const onEvent = webviewMocks.onDragDropEvent.mock.calls[0][0] as (event: unknown) => void;
    onEvent({ payload: { type: "drop", paths: ["C:\\exports\\one.json", "C:\\exports\\two.json"] } });
    await waitFor(() =>
      expect(mocks.importAuthFiles).toHaveBeenCalledWith([
        "C:\\exports\\one.json",
        "C:\\exports\\two.json",
      ]),
    );
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

  it("uses ChatGPT email as the primary identity and workspace as context", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    render(<App />);

    expect(await screen.findByRole("heading", { name: "person@example.com" })).toBeInTheDocument();
    expect(screen.getByText("Personal")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Refresh person@example.com" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Wake person@example.com" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Switch to person@example.com" })).toBeEnabled();
  });

  it("localizes ChatGPT card actions in Simplified Chinese", async () => {
    Object.defineProperty(window.navigator, "language", { configurable: true, value: "zh-CN" });
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    render(<App />);

    expect(await screen.findByRole("button", { name: "切换到 person@example.com" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "唤醒 person@example.com" })).toBeEnabled();
  });

  it("identifies the current account in its disabled action", async () => {
    mocks.listAccounts.mockResolvedValue([{ ...chatAccount, active: true }]);
    render(<App />);

    const current = await screen.findByRole("button", { name: "Current account: person@example.com" });
    expect(current).toBeDisabled();
  });

  it("keeps same-workspace ChatGPT accounts distinct by email", async () => {
    mocks.listAccounts.mockResolvedValue([
      chatAccount,
      { ...chatAccount, id: "account-2", email: "other@example.com" },
    ]);
    render(<App />);

    expect(await screen.findByRole("heading", { name: "person@example.com" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "other@example.com" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Switch to person@example.com" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Switch to other@example.com" })).toBeEnabled();
  });

  it("omits a duplicate workspace and falls back to a distinct legacy label", async () => {
    mocks.listAccounts.mockResolvedValue([
      { ...chatAccount, workspace_name: "person@example.com", label: "person@example.com" },
      { ...chatAccount, id: "account-2", workspace_name: undefined, label: "Legacy workspace" },
    ]);
    const { container } = render(<App />);

    expect(await screen.findAllByRole("heading", { name: "person@example.com" })).toHaveLength(2);
    expect(container.querySelectorAll(".account-copy p")).toHaveLength(1);
    expect(screen.getByText("Legacy workspace")).toBeInTheDocument();
  });

  it("keeps API-key cards label-first", async () => {
    mocks.listAccounts.mockResolvedValue([
      { id: "api-1", label: "Build key", kind: "api_key", active: false },
    ]);
    render(<App />);

    expect(await screen.findByRole("heading", { name: "Build key" })).toBeInTheDocument();
    expect(screen.getByText("Stored locally")).toBeInTheDocument();
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
    expect(screen.getByRole("button", { name: "Switch to person@example.com" })).toBeEnabled();
    expect(screen.getAllByText("Not available")).toHaveLength(2);
  });

  it("saves the current account and refreshes its unknown quota in the background", async () => {
    mocks.liveAccount.mockResolvedValue({
      status: "unknown_account",
      credential_store: "file",
      message: "Save the current Codex account before replacing its credentials",
    });
    mocks.accountQuota.mockResolvedValue({ account_id: "account-1", status: "unknown" });
    mocks.saveCurrentAccount.mockImplementation(async () => {
      mocks.listAccounts.mockResolvedValue([chatAccount]);
      mocks.liveAccount.mockResolvedValue({
        status: "ready",
        credential_store: "file",
        account: { ...chatAccount, active: true },
      });
      return chatAccount;
    });
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Save current account" }));
    await waitFor(() => expect(mocks.saveCurrentAccount).toHaveBeenCalledOnce());
    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledWith("account-1"));
    expect(await screen.findByText("Personal was saved safely.")).toBeInTheDocument();
  });

  it("coalesces a manual refresh with the background quota refresh", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ account_id: "account-1", status: "unknown" });
    let resolveRefresh: ((quota: QuotaView) => void) | undefined;
    mocks.refreshAccountQuota.mockImplementation(
      () => new Promise<QuotaView>((resolve) => { resolveRefresh = resolve; }),
    );
    render(<App />);

    await screen.findByText("Personal");
    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledOnce());
    await userEvent.click(screen.getByRole("button", { name: "Refresh person@example.com" }));
    expect(mocks.refreshAccountQuota).toHaveBeenCalledOnce();

    mocks.accountQuota.mockResolvedValue(staleQuota);
    resolveRefresh?.(staleQuota);
    expect(await screen.findByText("30%")).toBeInTheDocument();
  });

  it("keeps account actions available when the live Codex account cannot be identified", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ account_id: "account-1", status: "unknown" });
    mocks.refreshAccountQuota.mockRejectedValue(
      new Error("Codex is running and GSwitch cannot safely identify its active account"),
    );
    render(<App />);

    expect(await screen.findByText(/could not identify Codex's live account/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Switch to person@example.com" })).toBeEnabled();
  });

  it("maps the existing Quit Codex guard to an actionable retry message", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.switchAccount.mockRejectedValue(new Error("Quit Codex before changing the active account"));
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Switch to person@example.com" }));
    expect(await screen.findByText(/Quit the other Codex session/)).toBeInTheDocument();
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

  it("offers a signed update once and keeps Later local to the current session", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    updaterMocks.check.mockResolvedValue({ version: "1.0.2" });
    render(<App />);

    expect(await screen.findByText("GSwitch 1.0.2 is ready")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Later" }));
    expect(screen.queryByText("GSwitch 1.0.2 is ready")).not.toBeInTheDocument();
  });

  it("uses the release page fallback for a Debian package without touching accounts", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    updaterMocks.check.mockResolvedValue({ version: "1.0.2" });
    mocks.updateDelivery.mockResolvedValue("release_download");
    render(<App />);

    expect(await screen.findByText("This Linux package updates from the GSwitch release page.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Open release" }));
    await waitFor(() => expect(mocks.openLatestRelease).toHaveBeenCalledOnce());
    expect(mocks.switchAccount).not.toHaveBeenCalled();
  });

  it("restarts only after a macOS or AppImage update finishes", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
    updaterMocks.check.mockResolvedValue({ version: "1.0.2", downloadAndInstall });
    mocks.updateDelivery.mockResolvedValue("relaunch_required");
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Update" }));
    await waitFor(() => expect(downloadAndInstall).toHaveBeenCalledOnce());
    await userEvent.click(await screen.findByRole("button", { name: "Restart now" }));
    expect(updaterMocks.relaunch).toHaveBeenCalledOnce();
  });
});
