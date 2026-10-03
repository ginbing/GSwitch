import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { open } from "@tauri-apps/plugin-dialog";

import App from "./App";
import type { AccountView, QuotaRefreshFailureCode, QuotaView, SwitchFailureCode } from "./types";

// The second click of a two-click confirmation is ignored for 300 ms.
const pastConfirmGuard = () => new Promise((resolve) => window.setTimeout(resolve, 320));

async function confirmResetRow(row = 0) {
  await userEvent.click(await screen.findByRole("button", { name: "Details" }));
  const use = (await screen.findAllByRole("button", { name: /^Use the reset credit expiring/ }))[row]!;
  await userEvent.click(use);
  await pastConfirmGuard();
  await userEvent.click(screen.getByRole("button", { name: /^Confirm the reset credit expiring/ }));
}

const mocks = vi.hoisted(() => ({
  runtimeInfo: vi.fn(),
  codexCliInfo: vi.fn(),
  updateCodexCli: vi.fn(),
  openCodexCliGuide: vi.fn(),
  appSnapshot: vi.fn(),
  listAccounts: vi.fn(),
  resetDamagedAccountStore: vi.fn(),
  liveAccount: vi.fn(),
  startOAuth: vi.fn(),
  oauthStatus: vi.fn(),
  retryOAuth: vi.fn(),
  cancelOAuth: vi.fn(),
  openOAuth: vi.fn(),
  importAuthJson: vi.fn(),
  importAuthFiles: vi.fn(),
  exportAccounts: vi.fn(),
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
  redeemResetCredit: vi.fn(),
  recoverPendingResetCredit: vi.fn(),
  discardPendingResetCredit: vi.fn(),
  startWake: vi.fn(),
  startWakeAll: vi.fn(),
  startWakeSelected: vi.fn(),
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
    fetched_at_unix_ms: Date.now(),
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
  mocks.codexCliInfo.mockResolvedValue({ version: "0.156.1", latest_version: "0.157.0", update_status: "available", supports_update: true });
  mocks.updateCodexCli.mockResolvedValue({ version: "0.157.0", update_status: "unknown", supports_update: true });
  mocks.openCodexCliGuide.mockResolvedValue(undefined);
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
  mocks.retryOAuth.mockResolvedValue(undefined);
  mocks.openOAuth.mockResolvedValue(undefined);
  mocks.importAuthJson.mockResolvedValue(chatAccount);
  mocks.importAuthFiles.mockResolvedValue({
    imported: [chatAccount],
    duplicate_count: 0,
    unsupported_count: 0,
    failed_count: 0,
  });
  mocks.exportAccounts.mockResolvedValue({ exported_count: 1, cancelled: false });
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
  mocks.redeemResetCredit.mockResolvedValue({
    account_id: "account-1",
    outcome: "reset",
  });
  mocks.discardPendingResetCredit.mockResolvedValue(undefined);
  mocks.recoverPendingResetCredit.mockResolvedValue({
    account_id: "account-1",
    outcome: "nothing_to_reset",
  });
  mocks.startWake.mockResolvedValue({ operation_id: "wake-1" });
  mocks.startWakeAll.mockResolvedValue({ operation_id: "wake-all" });
  mocks.startWakeSelected.mockResolvedValue({ operation_id: "wake-selected" });
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

  it("shows and updates the same Codex CLI without touching accounts", async () => {
    let updated = false;
    mocks.codexCliInfo.mockImplementation(async () => updated
      ? { version: "0.157.0", latest_version: "0.157.0", update_status: "current", supports_update: true }
      : { version: "0.156.1", latest_version: "0.157.0", update_status: "available", supports_update: true });
    mocks.updateCodexCli.mockImplementation(async () => {
      updated = true;
      return { version: "0.157.0", update_status: "unknown", supports_update: true };
    });
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "Codex CLI 0.157.0 available" }, { timeout: 5_000 }));
    const dialog = await screen.findByRole("dialog", { name: "Codex CLI" });
    expect(await within(dialog).findByText("Codex CLI 0.156.1 · 0.157.0 available")).toBeInTheDocument();
    expect(within(dialog).getByText(/updates only the CLI/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Update" }));
    expect(await within(dialog).findByText("Update complete.")).toBeInTheDocument();
    // The version appears once, in the status line.
    expect(within(dialog).getByText("Codex CLI 0.157.0 · up to date")).toBeInTheDocument();
    expect(within(dialog).getAllByText(/0\.157\.0/)).toHaveLength(1);
    expect(within(dialog).queryByText(/updates only the CLI/)).not.toBeInTheDocument();
    expect(mocks.updateCodexCli).toHaveBeenCalledOnce();
    expect(mocks.switchAccount).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: /Codex CLI .* available/ })).not.toBeInTheDocument();
  });

  it("keeps CLI update failures in the Chinese dialog with a next step", async () => {
    Object.defineProperty(window.navigator, "language", { configurable: true, value: "zh-CN" });
    mocks.updateCodexCli.mockRejectedValue("codex_open");
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "Codex CLI 0.157.0 可更新" }, { timeout: 5_000 }));
    const dialog = await screen.findByRole("dialog", { name: "Codex CLI" });
    expect(await within(dialog).findByText("Codex CLI 0.156.1 · 可更新到 0.157.0")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "更新" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("请退出 Codex 后再更新");
    expect(within(dialog).getByText("Codex CLI 0.156.1 · 可更新到 0.157.0")).toBeInTheDocument();
  });

  it("does not offer an update to a CLI without the official command", async () => {
    mocks.codexCliInfo.mockResolvedValue({ version: "0.100.0", latest_version: "0.157.0", update_status: "available", supports_update: false });
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "Codex CLI 0.157.0 available" }, { timeout: 5_000 }));
    const dialog = await screen.findByRole("dialog", { name: "Codex CLI" });
    expect(await within(dialog).findByText("Codex CLI 0.100.0")).toBeInTheDocument();
    expect(within(dialog).getByText(/can't update itself/)).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Update" })).not.toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Official install guide" })).toBeInTheDocument();
  });

  it("keeps current and offline CLI checks out of the toolbar", async () => {
    mocks.codexCliInfo.mockResolvedValue({ version: "0.157.0", latest_version: "0.157.0", update_status: "current", supports_update: true });
    render(<App />);
    await screen.findByRole("heading", { name: "0 saved accounts" });
    await waitFor(() => expect(mocks.codexCliInfo).toHaveBeenCalled(), { timeout: 5_000 });
    expect(screen.queryByRole("button", { name: /Codex CLI .* available/ })).not.toBeInTheDocument();
  });

  it("keeps a failed CLI release check quiet", async () => {
    mocks.codexCliInfo.mockResolvedValue({ version: "0.157.0", update_status: "unknown", supports_update: true });
    render(<App />);
    await waitFor(() => expect(mocks.codexCliInfo).toHaveBeenCalled(), { timeout: 5_000 });
    expect(screen.queryByRole("button", { name: /Codex CLI/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
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
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    const first = render(<App />);
    await screen.findByRole("heading", { name: "1 saved account" });

    await userEvent.click(screen.getAllByLabelText("Language")[0]);
    await userEvent.click(screen.getByRole("button", { name: "简体中文" }));
    expect(screen.getByRole("button", { name: "添加账户" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "已保存 1 个账户" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "批量选择" })).toHaveAttribute("aria-pressed", "false");
    expect(window.localStorage.getItem("gswitch.language")).toBe("zh-CN");

    first.unmount();
    render(<App />);
    expect(await screen.findByRole("heading", { name: "已保存 1 个账户" })).toBeInTheDocument();
    const selectButton = screen.getByRole("button", { name: "批量选择" });
    await userEvent.click(selectButton);
    expect(screen.getByRole("button", { name: "完成" })).toHaveAttribute("aria-pressed", "true");
  });

  it("guides a first-time user to an explicit import or manual add", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "0 saved accounts" })).toBeInTheDocument();
    expect(screen.getByText(/scans this computer only when you choose to/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import account files" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Add manually" }));
    const dialog = await screen.findByRole("dialog", { name: "Add a Codex account" });
    expect(dialog).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /Paste auth JSON/ })).toBeInTheDocument();
    expect(within(dialog).getByText("Import existing")).toBeInTheDocument();
    expect(within(dialog).getByText("Add new")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /Choose files/ })).toBeInTheDocument();
    expect(within(dialog).getByText(/Select one or more account files/i)).toBeInTheDocument();
  });

  it("keeps keyboard focus in a dialog and returns it to the launcher", async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByRole("heading", { name: "0 saved accounts" });
    const launcher = screen.getByRole("button", { name: "Add manually" });
    await user.click(launcher);
    const dialog = screen.getByRole("dialog", { name: "Add a Codex account" });
    expect(dialog).toHaveFocus();

    await user.tab();
    expect(within(dialog).getByRole("button", { name: "Close Add a Codex account" })).toHaveFocus();
    await user.tab({ shift: true });
    expect(within(dialog).getByText("Other methods")).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(launcher).toHaveFocus();
  });

  it("ignores a double-click on a two-click confirmation and disarms on an outside click", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    const user = userEvent.setup();
    render(<App />);
    await screen.findByText("person@example.com");
    await user.click(screen.getByRole("button", { name: "Select accounts" }));
    await user.click(screen.getByRole("button", { name: "Select all" }));
    await user.click(screen.getByRole("button", { name: "Export" }));
    const dialog = screen.getByRole("dialog", { name: "Export 1 account" });

    await user.dblClick(within(dialog).getByRole("button", { name: "Export" }));
    expect(mocks.exportAccounts).not.toHaveBeenCalled();
    expect(within(dialog).getByRole("button", { name: "Confirm export" })).toBeInTheDocument();

    await user.click(within(dialog).getByText(/The exported file isn't encrypted/));
    expect(within(dialog).getByRole("button", { name: "Export" })).toBeInTheDocument();
    expect(mocks.exportAccounts).not.toHaveBeenCalled();
  });

  it("keeps an export confirmation open while its save operation is pending", async () => {
    let finishExport!: (value: { exported_count: number; cancelled: boolean }) => void;
    mocks.exportAccounts.mockImplementation(() => new Promise((resolve) => { finishExport = resolve; }));
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    const user = userEvent.setup();
    render(<App />);
    await screen.findByText("person@example.com");
    await user.click(screen.getByRole("button", { name: "Select accounts" }));
    await user.click(screen.getByRole("button", { name: "Select all" }));
    await user.click(screen.getByRole("button", { name: "Export" }));
    const dialog = screen.getByRole("dialog", { name: "Export 1 account" });
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await pastConfirmGuard();
    await user.click(within(dialog).getByRole("button", { name: "Confirm export" }));
    await waitFor(() => expect(mocks.exportAccounts).toHaveBeenCalledOnce());
    expect(within(dialog).getByRole("button", { name: "Close Export 1 account" })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeDisabled();
    await user.keyboard("{Escape}");
    expect(dialog).toBeInTheDocument();
    finishExport({ exported_count: 1, cancelled: false });
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
  });

  it("keeps the current account label visible without claiming sign-in readiness", async () => {
    mocks.liveAccount.mockResolvedValue({
      status: "ready",
      credential_store: "file",
      account: { label: "a-very-long-account-name-that-must-truncate@example.com" },
    });
    const { container } = render(<App />);

    await screen.findByRole("heading", { name: "0 saved accounts" });
    expect(container.querySelector(".brand-status > .status-dot")).toBeInTheDocument();
    expect(container.querySelector(".brand-status > .status-ready")).not.toBeInTheDocument();
    expect(container.querySelector(".brand-status-label")).toHaveTextContent("a-very-long-account-name-that-must-truncate@example.com");
  });

  it("does not scan local account sources at startup or when the method row opens", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "0 saved accounts" })).toBeInTheDocument();
    expect(mocks.discoverLocalAccounts).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Add manually" }));
    await userEvent.click(screen.getByRole("button", { name: /Find on this computer/ }));
    expect(screen.getByText(/Nothing is imported until you select accounts/i)).toBeInTheDocument();
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
    await userEvent.click(screen.getByRole("button", { name: /Find on this computer/ }));
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
    await userEvent.click(screen.getByRole("button", { name: /Find on this computer/ }));
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
    expect(await screen.findByText("Import results — added: 2; already present or duplicate: 1; unsupported or failed: 2.")).toBeInTheDocument();
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
    });
    render(<App />);

    expect(await screen.findByText("Saved accounts are protected until recovery")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add account" })).toBeDisabled();
    expect(screen.queryByText("Add your first Codex account")).not.toBeInTheDocument();

    await userEvent.click(screen.getAllByRole("button", { name: "Review recovery" })[0]);
    expect(await screen.findByRole("dialog", { name: "Recover GSwitch account storage" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Clear and rebuild" }));
    expect(mocks.resetDamagedAccountStore).not.toHaveBeenCalled();

    await pastConfirmGuard();
    await userEvent.click(screen.getByRole("button", { name: "Confirm clear" }));
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

  it("lists each reset credit with its own button that needs a second click", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    const withTitles: QuotaView = {
      ...staleQuota,
      snapshot: {
        ...staleQuota.snapshot!,
        reset_credits: {
          ...staleQuota.snapshot!.reset_credits!,
          usable_credits: [
            { expires_at: 1893456000, title: "Full reset (Weekly + 5 hr)" },
            { expires_at: 1894060800 },
          ],
        },
      },
    };
    mocks.accountQuota.mockResolvedValue(withTitles);
    mocks.refreshAccountQuota.mockResolvedValue(withTitles);
    render(<App />);

    expect(await screen.findByText("Personal")).toBeInTheDocument();
    expect(screen.getByText("Stale")).toBeInTheDocument();
    expect(screen.getByText("Last 30%")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Details" }));
    const dialog = await screen.findByRole("dialog", { name: "Reset credits · person@example.com" });
    expect(within(dialog).getByText("2 available")).toBeInTheDocument();
    expect(within(dialog).getByText("Full reset (Weekly + 5 hr)")).toBeInTheDocument();
    expect(within(dialog).getByText("Usage limit reset")).toBeInTheDocument();
    expect(within(dialog).queryByText("Personal")).not.toBeInTheDocument();

    const [first] = within(dialog).getAllByRole("button", { name: /^Use the reset credit expiring/ });
    await userEvent.click(first!);
    expect(mocks.redeemResetCredit).not.toHaveBeenCalled();
    expect(within(dialog).getByRole("button", { name: /^Confirm the reset credit expiring/ })).toHaveTextContent("Confirm");

    await pastConfirmGuard();
    await userEvent.click(within(dialog).getByRole("button", { name: /^Confirm the reset credit expiring/ }));
    await waitFor(() =>
      expect(mocks.redeemResetCredit).toHaveBeenCalledWith("account-1", { expires_at: 1893456000, title: "Full reset (Weekly + 5 hr)" }),
    );
  });

  it("redeems the credit on the row the user picks, not the earliest", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    render(<App />);

    await confirmResetRow(1);

    await waitFor(() =>
      expect(mocks.redeemResetCredit).toHaveBeenCalledWith("account-1", { expires_at: 1894060800 }),
    );
  });

  it("adds the workspace to the reset title only when the email alone is ambiguous", async () => {
    const teamAccount = { ...chatAccount, id: "account-2", workspace_name: "Team" };
    mocks.listAccounts.mockResolvedValue([chatAccount, teamAccount]);
    mocks.accountQuota.mockImplementation(async (accountId: string) => ({ ...staleQuota, account_id: accountId }));
    render(<App />);

    const [details] = await screen.findAllByRole("button", { name: "Details" });
    await userEvent.click(details!);

    expect(await screen.findByRole("dialog", { name: "Reset credits · person@example.com · Personal" })).toBeInTheDocument();
  });

  it("shows the quota a reset returns on the card without a manual refresh", async () => {
    const quotaAfterReset: QuotaView = {
      account_id: "account-1",
      status: "fresh",
      snapshot: {
        ...staleQuota.snapshot!,
        fetched_at_unix_ms: Date.now(),
        reset_credits: {
          available_count: 1,
          nearest_expiry: 1894060800,
          details_available: true,
          can_redeem: true,
          usable_credits: [{ expires_at: 1894060800 }],
        },
      },
    };
    let redeemed = false;
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockImplementation(async () => (redeemed ? quotaAfterReset : staleQuota));
    mocks.redeemResetCredit.mockImplementation(async () => {
      redeemed = true;
      return { account_id: "account-1", outcome: "reset", quota: quotaAfterReset };
    });
    const { container } = render(<App />);

    expect(await screen.findByText("Personal")).toBeInTheDocument();
    await waitFor(() =>
      expect(container.querySelector(".credit-count strong")?.textContent).toBe("2"),
    );

    await confirmResetRow();

    await waitFor(() =>
      expect(container.querySelector(".credit-count strong")?.textContent).toBe("1"),
    );
    expect(mocks.refreshAccountQuota).not.toHaveBeenCalledWith("account-1", false);
  });

  it("names the account and the credit a reset used", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    mocks.redeemResetCredit.mockResolvedValue({
      account_id: "account-1",
      outcome: "reset",
      used_expires_at: 1893456000,
    });
    render(<App />);

    await confirmResetRow();

    expect(await screen.findByText(/^Used the reset credit expiring .+ for person@example.com\.$/)).toBeInTheDocument();
  });

  it("says when an account did not need a reset and no credit was used", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    mocks.redeemResetCredit.mockResolvedValue({ account_id: "account-1", outcome: "nothing_to_reset" });
    render(<App />);

    await confirmResetRow();

    expect(await screen.findByText("person@example.com doesn't need a reset right now. No credit was used.")).toBeInTheDocument();
  });

  it("explains an unconfirmed reset without claiming a credit was or was not used", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    mocks.redeemResetCredit.mockRejectedValue({ code: "result_unknown" });
    render(<App />);

    await confirmResetRow();

    expect(await screen.findByText(
      "GSwitch couldn't confirm whether the reset went through. The request is saved; retrying uses the same credit and won't use another.",
    )).toBeInTheDocument();
  });

  it("keeps the reset dialog closable while a full refresh runs", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.refreshAccountQuota.mockImplementation(() => new Promise(() => undefined));
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });

    await userEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await userEvent.click(screen.getByRole("button", { name: "Details" }));
    const dialog = await screen.findByRole("dialog", { name: "Reset credits · person@example.com" });

    expect(within(dialog).queryByText("Working…")).not.toBeInTheDocument();
    const close = within(dialog).getByRole("button", { name: /^Close / });
    expect(close).toBeEnabled();
    await userEvent.click(close);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows reset recovery after a reset request fails", async () => {
    let failed = false;
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    mocks.appSnapshot.mockImplementation(async () => ({
      storage: { status: "ready" },
      accounts: [chatAccount],
      pending_reset: failed ? { account_id: "account-1", discardable: false } : undefined,
      runtime: await mocks.runtimeInfo(),
      live: await mocks.liveAccount(),
    }));
    mocks.redeemResetCredit.mockImplementation(async () => {
      failed = true;
      throw "Codex App Server did not respond";
    });
    render(<App />);

    expect(await screen.findByText("Personal")).toBeInTheDocument();
    await confirmResetRow();

    expect(await screen.findByText("A reset hasn't been confirmed")).toBeInTheDocument();
  });

  it("shows the provider-supplied five-week free quota and reset time", async () => {
    const freeAccount = { ...chatAccount, plan_type: "free" };
    const resetAt = Math.floor(Date.now() / 1000) + 21 * 24 * 60 * 60;
    mocks.listAccounts.mockResolvedValue([freeAccount]);
    mocks.accountQuota.mockResolvedValue({
      account_id: freeAccount.id,
      status: "fresh",
      snapshot: {
        fetched_at_unix_ms: Date.now(),
        buckets: [{
          limit_id: "codex",
          kind: "codex",
          windows: [{ kind: "other", window_duration_mins: 50_400, remaining_percent: 100, used_percent: 0, resets_at: resetAt }],
        }],
      },
    });
    render(<App />);

    const card = (await screen.findByRole("heading", { name: "person@example.com" })).closest(".account-card");
    expect(card).toHaveTextContent("5 weeks");
    expect(card).toHaveTextContent("100%");
    const resetTime = card?.querySelector("time.quota-reset-time");
    expect(resetTime).toHaveTextContent(/\d+d/);
    expect(resetTime).toHaveAttribute("aria-label", expect.stringContaining("Resets"));
    expect(resetTime).toHaveAttribute("data-full-time", expect.any(String));
    expect(card).not.toHaveTextContent(/Resets \d/);
    expect(card).not.toHaveTextContent("5-hour");
    expect(card).not.toHaveTextContent("Reset time unavailable");
    expect(screen.getByRole("button", { name: "Wake person@example.com" })).toBeEnabled();
  });

  it("shows a short Chinese reset time with keyboard access to the exact date", async () => {
    Object.defineProperty(window.navigator, "language", { configurable: true, value: "zh-CN" });
    const resetAt = Math.floor(Date.now() / 1000) + 5 * 60 * 60;
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({
      account_id: chatAccount.id,
      status: "fresh",
      snapshot: {
        fetched_at_unix_ms: Date.now(),
        buckets: [{
          limit_id: "codex",
          kind: "codex",
          windows: [{ kind: "five_hour", window_duration_mins: 300, remaining_percent: 50, used_percent: 50, resets_at: resetAt }],
        }],
      },
    });
    render(<App />);

    const card = (await screen.findByRole("heading", { name: "person@example.com" })).closest(".account-card");
    const resetTime = card?.querySelector("time.quota-reset-time");
    expect(resetTime).toHaveTextContent(/5小时 · \d{2}\/\d{2} \d{2}:\d{2}/);
    expect(resetTime).toHaveAttribute("aria-label", expect.stringContaining("重置于"));
    expect(resetTime).toHaveAttribute("tabindex", "0");
    expect(card).not.toHaveTextContent("重置于");
    expect(screen.getByRole("button", { name: "唤醒 person@example.com" })).toBeEnabled();
  });

  it("allows the same account to Wake again after an earlier completed request", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.wakeOperation.mockResolvedValue({
      id: "wake-1",
      status: "completed",
      results: [{ account_id: chatAccount.id, label: "Personal", result: "reply_received", request_state: "sent" }],
    });
    render(<App />);
    const wakeButton = await screen.findByRole("button", { name: "Wake person@example.com" });
    await userEvent.click(wakeButton);
    const firstDialog = await screen.findByRole("dialog", { name: "Wake" });
    expect(await within(firstDialog).findByText("Succeeded")).toBeInTheDocument();
    await userEvent.click(within(firstDialog).getByRole("button", { name: "Done" }));
    expect(wakeButton).toBeEnabled();
    await userEvent.click(wakeButton);
    expect(mocks.startWake).toHaveBeenCalledTimes(2);
  });

  it("does not show a past reset time as a current countdown", async () => {
    const resetAt = Math.floor(Date.now() / 1000) - 60;
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({
      account_id: chatAccount.id,
      status: "fresh",
      snapshot: {
        fetched_at_unix_ms: Date.now(),
        buckets: [{
          limit_id: "codex",
          kind: "codex",
          windows: [{ kind: "five_hour", window_duration_mins: 300, remaining_percent: 50, used_percent: 50, resets_at: resetAt }],
        }],
      },
    });
    render(<App />);

    const card = (await screen.findByRole("heading", { name: "person@example.com" })).closest(".account-card");
    expect(card?.querySelector("time.quota-reset-time")).toHaveTextContent("Refresh to update");
  });
  it("uses ChatGPT email as the primary identity and workspace as context", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    render(<App />);

    expect(await screen.findByRole("heading", { name: "person@example.com" })).toBeInTheDocument();
    expect(screen.getByText("Personal")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Refresh person@example.com" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Wake person@example.com" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Switch to person@example.com" })).toBeEnabled();
  });

  it("localizes ChatGPT card actions in Simplified Chinese", async () => {
    Object.defineProperty(window.navigator, "language", { configurable: true, value: "zh-CN" });
    mocks.listAccounts.mockResolvedValue([{ ...chatAccount, plan_type: "team" }]);
    render(<App />);

    expect(await screen.findByRole("button", { name: "切换到 person@example.com" })).toBeEnabled();
    expect(screen.getByText("团队")).toBeInTheDocument();
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
      pending_reset: { account_id: "account-1", discardable: false },
      runtime: await mocks.runtimeInfo(),
      live: await mocks.liveAccount(),
    }));
    render(<App />);

    expect(await screen.findByText("A reset hasn't been confirmed")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Review" }));
    const dialog = await screen.findByRole("dialog", { name: "Unfinished reset" });
    expect(within(dialog).getByText(/The last reset for person@example.com wasn't confirmed/)).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Discard this record" })).not.toBeInTheDocument();
    expect(mocks.recoverPendingResetCredit).not.toHaveBeenCalled();
    expect(mocks.redeemResetCredit).not.toHaveBeenCalled();

    await userEvent.click(within(dialog).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(mocks.recoverPendingResetCredit).toHaveBeenCalledOnce());
    expect(mocks.redeemResetCredit).not.toHaveBeenCalled();
  });

  it("discards an unrecoverable pending reset only after a second click", async () => {
    let pending: { account_id: string; discardable: boolean } | undefined = {
      account_id: "removed-account",
      discardable: true,
    };
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    mocks.discardPendingResetCredit.mockImplementation(async () => {
      pending = undefined;
    });
    mocks.appSnapshot.mockImplementation(async () => ({
      storage: { status: "ready" },
      accounts: await mocks.listAccounts(),
      pending_reset: pending,
      runtime: await mocks.runtimeInfo(),
      live: await mocks.liveAccount(),
    }));
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Review" }));
    const dialog = await screen.findByRole("dialog", { name: "Unfinished reset" });
    expect(within(dialog).getByText(/The last reset for a removed account/)).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole("button", { name: "Discard this record" }));
    expect(mocks.discardPendingResetCredit).not.toHaveBeenCalled();
    expect(within(dialog).getByText(/can't confirm whether this request went through/)).toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole("button", { name: "Confirm discard" }));
    await waitFor(() => expect(mocks.discardPendingResetCredit).toHaveBeenCalledOnce());
    expect(await screen.findByText("The unfinished reset was discarded.")).toBeInTheDocument();
    expect(screen.queryByText("A reset hasn't been confirmed")).not.toBeInTheDocument();
  });

  it("keeps account actions available when a local quota projection cannot be read", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockRejectedValue(new Error("quota cache unavailable"));
    render(<App />);

    expect(await screen.findByText("Personal")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Switch to person@example.com" })).toBeEnabled();
    expect(screen.getAllByText("Not available")).toHaveLength(1);
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
    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledWith("account-1", true));
    expect(await screen.findByText("person@example.com was saved safely.")).toBeInTheDocument();
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
    expect(await screen.findByText("Last 30%")).toBeInTheDocument();
  });

  it("shows how long ago each card's quota was read", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({
      ...staleQuota,
      status: "fresh",
      snapshot: { ...staleQuota.snapshot!, fetched_at_unix_ms: Date.now() - 3 * 60_000 },
    });
    render(<App />);

    expect(await screen.findByText("Updated 3 minutes ago")).toBeInTheDocument();
    expect(mocks.refreshAccountQuota).not.toHaveBeenCalled();
  });

  it("reads the current account again when the window regains focus after a minute", async () => {
    const active = { ...chatAccount, active: true };
    mocks.listAccounts.mockResolvedValue([active]);
    mocks.liveAccount.mockResolvedValue({ status: "ready", credential_store: "file", account: active });
    mocks.accountQuota.mockResolvedValue({
      ...staleQuota,
      status: "fresh",
      snapshot: { ...staleQuota.snapshot!, fetched_at_unix_ms: Date.now() - 90_000 },
    });
    render(<App />);

    expect(await screen.findByText("Updated 1 minute ago")).toBeInTheDocument();
    // Under two minutes old, so startup leaves the current account alone.
    expect(mocks.refreshAccountQuota).not.toHaveBeenCalled();

    fireEvent.focus(window);

    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledWith("account-1", true));
  });

  it("starts a user's refresh without waiting behind automatic refreshes", async () => {
    const secondAccount = { ...chatAccount, id: "account-2", email: "other@example.com" };
    mocks.listAccounts.mockResolvedValue([chatAccount, secondAccount]);
    mocks.accountQuota.mockImplementation(async (accountId: string) => ({ account_id: accountId, status: "unknown" }));
    mocks.refreshAccountQuota.mockImplementation((accountId: string) =>
      accountId === "account-1"
        ? new Promise<QuotaView>(() => undefined)
        : Promise.resolve({ ...staleQuota, account_id: accountId, status: "fresh" }),
    );
    render(<App />);

    await screen.findByRole("heading", { name: "other@example.com" });
    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledWith("account-1", true));
    expect(mocks.refreshAccountQuota).not.toHaveBeenCalledWith("account-2", true);

    await userEvent.click(screen.getByRole("button", { name: "Refresh other@example.com" }));

    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledWith("account-2", true));
    expect(mocks.refreshAccountQuota).toHaveBeenCalledTimes(2);
  });

  it("uses the official sign-in refresh only after a read shows the saved sign-in needs it", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.refreshAccountQuota.mockImplementation(async (_accountId: string, background: boolean) => {
      if (background) {
        throw { code: "manual_refresh_needed" };
      }
      return { ...staleQuota, status: "fresh" };
    });
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Refresh person@example.com" }));

    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledWith("account-1", false));
    expect(mocks.refreshAccountQuota.mock.calls).toEqual([["account-1", true], ["account-1", false]]);
    expect(screen.queryByRole("button", { name: "Quota update failed for person@example.com" })).not.toBeInTheDocument();
  });

  it("shows full-refresh progress on the toolbar without locking the workspace", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.refreshAccountQuota.mockImplementation(() => new Promise<QuotaView>(() => undefined));
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Refresh" }));

    expect(await screen.findByRole("button", { name: "Refreshing 0/1" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Wake person@example.com" })).toBeEnabled();
  });

  it("refreshes up to three accounts at once and continues after one quota failure", async () => {
    const accounts = Array.from({ length: 6 }, (_, index) => ({
      ...chatAccount,
      id: `account-${index + 1}`,
      email: `person${index + 1}@example.com`,
      label: `Personal ${index + 1}`,
      workspace_name: `Personal ${index + 1}`,
    }));
    mocks.listAccounts.mockResolvedValue(accounts);
    mocks.accountQuota.mockImplementation(async (accountId: string) => ({
      ...staleQuota,
      account_id: accountId,
      status: "fresh",
    }));
    let activeRequests = 0;
    let maximumConcurrentRequests = 0;
    mocks.refreshAccountQuota.mockImplementation(async (accountId: string) => {
      activeRequests += 1;
      maximumConcurrentRequests = Math.max(maximumConcurrentRequests, activeRequests);
      try {
        await new Promise<void>((resolve) => window.setTimeout(resolve, 2));
        if (accountId === "account-3") {
          throw new Error("provider unavailable");
        }
        return { ...staleQuota, account_id: accountId, status: "fresh" };
      } finally {
        activeRequests -= 1;
      }
    });
    render(<App />);

    await screen.findByRole("heading", { name: "person1@example.com" });
    await userEvent.click(screen.getByRole("button", { name: "Refresh" }));

    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledTimes(6));
    expect(maximumConcurrentRequests).toBe(3);
    expect(mocks.refreshAccountQuota.mock.calls.map(([id]) => id)).toEqual(
      accounts.map((account) => account.id),
    );
    expect(await screen.findByText("Could not refresh quota for 1 of your accounts. See the affected cards for their latest result.")).toBeInTheDocument();
    const failedCard = screen.getByRole("heading", { name: "person3@example.com" }).closest(".account-card");
    expect(failedCard).toHaveTextContent(/Last result is stale|Not available/);
    expect(within(failedCard as HTMLElement).getByRole("button", { name: "Quota update failed for person3@example.com" })).toBeInTheDocument();
    expect(failedCard).toHaveTextContent("GSwitch could not update this quota. Try again later.");
    expect(failedCard).toHaveTextContent("Last 30%");
    expect(mocks.appSnapshot).toHaveBeenCalledOnce();
  });

  it("keeps failed quota values visibly historical and makes sign-in the primary action for a rejected sign-in", async () => {
    const copyEmail = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, value: { writeText: copyEmail } });
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    mocks.refreshAccountQuota.mockRejectedValue({ code: "authentication" });
    render(<App />);

    const alert = await screen.findByRole("button", { name: "Quota update failed for person@example.com" });
    expect(alert).toHaveAttribute("aria-describedby", "quota-error-account-1");
    expect(screen.getByText("Last 30%")).toBeInTheDocument();
    expect(screen.getByText("This saved sign-in was rejected. Sign in to this account again.")).toBeInTheDocument();
    expect(screen.getByText(/Last successful update:/)).toBeInTheDocument();
    expect(screen.queryByText("Quota could not be refreshed. The last result is shown when available.")).not.toBeInTheDocument();

    await userEvent.click(screen.getByLabelText("More actions for person@example.com"));
    await userEvent.click(screen.getByRole("button", { name: "Copy email" }));
    expect(copyEmail).toHaveBeenCalledWith("person@example.com");
    await userEvent.click(screen.getByRole("button", { name: "Sign in again" }));
    await waitFor(() => expect(mocks.startOAuth).toHaveBeenCalledWith("account-1"));
    expect(mocks.openOAuth).not.toHaveBeenCalled();
    expect(await screen.findByRole("dialog", { name: "Sign in again · person@example.com" })).toBeInTheDocument();
  });

  it("shows a direct recovery action with the saved email and workspace", async () => {
    mocks.listAccounts.mockResolvedValue([{ ...chatAccount, sign_in_required: true }]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    render(<App />);
    const card = (await screen.findByRole("heading", { name: "person@example.com" })).closest("article");
    expect(card).toHaveTextContent("Sign in required");
    await userEvent.click(within(card as HTMLElement).getByRole("button", { name: "Sign in again" }));
    expect(mocks.startOAuth).toHaveBeenCalledWith("account-1");
    const dialog = await screen.findByRole("dialog", { name: "Sign in again · person@example.com" });
    expect(dialog).toHaveTextContent("Account: person@example.com");
    expect(dialog).toHaveTextContent("Workspace: Personal");
  });

  it("updates one account quota without reloading the account workspace", async () => {
    const secondAccount = { ...chatAccount, id: "account-2", email: "other@example.com" };
    mocks.listAccounts.mockResolvedValue([chatAccount, secondAccount]);
    mocks.accountQuota.mockImplementation(async (accountId: string) => ({
      ...staleQuota,
      account_id: accountId,
      status: "fresh",
    }));
    render(<App />);

    await screen.findByRole("heading", { name: "person@example.com" });
    expect(await screen.findByRole("heading", { name: "other@example.com" })).toBeInTheDocument();
    expect(mocks.appSnapshot).toHaveBeenCalledTimes(1);

    await userEvent.click(screen.getByRole("button", { name: "Refresh person@example.com" }));

    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledWith("account-1", true));
    expect(mocks.appSnapshot).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("heading", { name: "person@example.com" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "other@example.com" })).toBeInTheDocument();
  });

  it("keeps unrelated account actions interactive during a point refresh", async () => {
    const secondAccount = { ...chatAccount, id: "account-2", email: "other@example.com" };
    mocks.listAccounts.mockResolvedValue([chatAccount, secondAccount]);
    mocks.accountQuota.mockImplementation(async (accountId: string) => ({
      ...staleQuota,
      account_id: accountId,
      status: "fresh",
      snapshot: accountId === "account-2" ? {
        ...staleQuota.snapshot!,
        buckets: [{
          ...staleQuota.snapshot!.buckets[0]!,
          windows: staleQuota.snapshot!.buckets[0]!.windows.map((window) => window.kind === "five_hour"
            ? { ...window, remaining_percent: 0, used_percent: 100 }
            : window),
        }],
      } : staleQuota.snapshot,
    }));
    let resolveRefresh: ((quota: QuotaView) => void) | undefined;
    mocks.refreshAccountQuota.mockImplementation(
      () => new Promise<QuotaView>((resolve) => { resolveRefresh = resolve; }),
    );
    render(<App />);

    await screen.findByRole("heading", { name: "person@example.com" });
    await userEvent.click(screen.getByRole("button", { name: "Refresh person@example.com" }));
    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledWith("account-1", true));

    expect(screen.getByRole("button", { name: "Refresh person@example.com" })).toBeDisabled();
    const otherWake = screen.getByRole("button", { name: "Wake other@example.com" });
    expect(otherWake).toBeEnabled();
    await userEvent.click(otherWake);
    await waitFor(() => expect(mocks.startWake).toHaveBeenCalledWith("account-2", false));

    resolveRefresh?.(staleQuota);
  });

  it("keeps account actions available when the live Codex account cannot be identified", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ account_id: "account-1", status: "unknown" });
    mocks.refreshAccountQuota.mockRejectedValue({ code: "codex_account_unknown" });
    render(<App />);

    expect(await screen.findByText(/Codex is running, but its account cannot be identified/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Switch to person@example.com" })).toBeEnabled();
  });

  it.each([
    ["operation_busy", /Another GSwitch operation is in progress/],
    ["codex_open", /Quit the other Codex session/],
    ["account_needs_sign_in", /needs sign-in again/],
    ["file_store_required", /Enable file-backed Codex credentials/],
    ["credentials_changed", /Codex credentials changed during the switch/],
    ["recovery_required", /Complete the protected switch recovery/],
    ["codex_app_server_unavailable", /could not start the Codex CLI service/],
    ["codex_config_unavailable", /could not finish checking Codex settings/],
    ["codex_config_cleanup_failed", /could not finish the local Codex check/],
    ["current_credential_unreadable", /could not read the current Codex sign-in/],
    ["current_account_not_saved", /Save the current Codex account/],
    ["local_verification_failed", /could not read its local account state/],
    ["target_check_unavailable", /could not check .* with ChatGPT/],
    ["target_workspace_mismatch", /do not match the selected workspace/],
    ["post_write_verification_failed", /restored the previous Codex account/],
    ["verification_failed", /could not verify/],
  ] satisfies Array<[SwitchFailureCode, RegExp]>)(
    "shows an actionable %s switch failure for the selected account",
    async (code, message) => {
      mocks.listAccounts.mockResolvedValue([chatAccount]);
      mocks.switchAccount.mockRejectedValue({ code });
      render(<App />);

      await userEvent.click(
        await screen.findByRole("button", { name: "Switch to person@example.com" }),
      );
      const notice = await screen.findByRole("alert");
      expect(notice).toHaveTextContent(message);
      expect(notice).toHaveTextContent("person@example.com");
    },
  );

  it("uses the account label in a structured switch failure when email is unavailable", async () => {
    const account = { ...chatAccount, email: undefined, label: "Fallback account" };
    mocks.listAccounts.mockResolvedValue([account]);
    mocks.switchAccount.mockRejectedValue({ code: "verification_failed" });
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Switch to Fallback account" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("could not verify Fallback account");
  });

  it("localizes a structured switch failure in Simplified Chinese", async () => {
    Object.defineProperty(window.navigator, "language", { configurable: true, value: "zh-CN" });
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.switchAccount.mockRejectedValue({ code: "account_needs_sign_in" });
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "切换到 person@example.com" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "person@example.com 需要重新登录。请登录或重新导入该账户后重试。",
    );
  });

  // Whether each failure makes Sign in again the card's primary action. The
  // account menu must offer it after every failure, so recovery never depends
  // on GSwitch having diagnosed the failure correctly.
  const switchFailurePromotesSignIn = {
    operation_busy: false,
    codex_open: false,
    account_needs_sign_in: true,
    file_store_required: false,
    credentials_changed: false,
    recovery_required: false,
    local_verification_failed: false,
    codex_app_server_unavailable: false,
    codex_config_unavailable: false,
    codex_config_cleanup_failed: false,
    current_credential_unreadable: false,
    current_account_not_saved: false,
    target_check_unavailable: false,
    target_workspace_mismatch: false,
    post_write_verification_failed: false,
    verification_failed: false,
  } satisfies Record<SwitchFailureCode, boolean>;

  const quotaFailurePromotesSignIn = {
    operation_busy: false,
    codex_account_unknown: false,
    authentication: true,
    manual_refresh_needed: false,
    rate_limited: false,
    network: false,
    service: false,
    invalid_response: false,
    identity_mismatch: false,
    unavailable: false,
  } satisfies Record<QuotaRefreshFailureCode, boolean>;

  async function expectSignInAgainReachable(promoted: boolean) {
    const card = screen.getByRole("heading", { name: "person@example.com" }).closest("article") as HTMLElement;
    expect(within(card).queryAllByRole("button", { name: "Sign in again" })).toHaveLength(promoted ? 1 : 0);
    expect(within(card).queryAllByRole("button", { name: "Switch to person@example.com" })).toHaveLength(promoted ? 0 : 1);

    await userEvent.click(within(card).getByRole("button", { name: "More actions for person@example.com" }));
    const menu = card.querySelector<HTMLElement>(".card-menu-popover")!;
    await userEvent.click(within(menu).getByRole("button", { name: "Sign in again" }));
    await waitFor(() => expect(mocks.startOAuth).toHaveBeenCalledWith("account-1"));
    expect(await screen.findByRole("dialog", { name: "Sign in again · person@example.com" })).toBeInTheDocument();
  }

  it.each(Object.entries(switchFailurePromotesSignIn) as Array<[SwitchFailureCode, boolean]>)(
    "keeps Sign in again reachable after a %s switch failure",
    async (code, promoted) => {
      mocks.listAccounts.mockResolvedValue([chatAccount]);
      mocks.switchAccount.mockRejectedValue({ code });
      render(<App />);

      await userEvent.click(await screen.findByRole("button", { name: "Switch to person@example.com" }));
      await screen.findByRole("alert");
      await expectSignInAgainReachable(promoted);
    },
  );

  it.each(Object.entries(quotaFailurePromotesSignIn) as Array<[QuotaRefreshFailureCode, boolean]>)(
    "keeps Sign in again reachable after a %s quota failure",
    async (code, promoted) => {
      mocks.listAccounts.mockResolvedValue([chatAccount]);
      mocks.accountQuota.mockResolvedValue(staleQuota);
      mocks.refreshAccountQuota.mockRejectedValue({ code });
      render(<App />);

      await screen.findByRole("button", { name: "Quota update failed for person@example.com" });
      await expectSignInAgainReachable(promoted);
    },
  );

  it("does not offer browser sign-in for an API-key account", async () => {
    mocks.listAccounts.mockResolvedValue([{ id: "api-1", label: "Key", kind: "api_key", active: false }]);
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "More actions for Key" }));
    expect(screen.getByRole("button", { name: "Remove Key" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Sign in again" })).not.toBeInTheDocument();
  });

  it("updates the active account without reloading the workspace or adding a banner", async () => {
    const currentAccount: AccountView = {
      id: "account-2",
      label: "Current",
      kind: "chat_gpt",
      email: "current@example.com",
      active: true,
    };
    const activatedTarget = { ...chatAccount, active: true };
    mocks.listAccounts
      .mockResolvedValueOnce([currentAccount, chatAccount])
      .mockResolvedValue([{ ...currentAccount, active: false }, activatedTarget]);
    mocks.liveAccount
      .mockResolvedValueOnce({
        status: "ready",
        credential_store: "file",
        account: currentAccount,
      })
      .mockResolvedValue({
        status: "ready",
        credential_store: "file",
        account: activatedTarget,
      });
    mocks.switchAccount.mockResolvedValue({ account: activatedTarget });
    const { container } = render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "Switch to person@example.com" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Current account: person@example.com" })).toBeDisabled());
    expect(mocks.appSnapshot).toHaveBeenCalledOnce();
    expect(screen.queryByText("person@example.com is now the active Codex account.")).not.toBeInTheDocument();
    const cards = container.querySelectorAll<HTMLElement>(".account-card");
    expect(within(cards[0]!).getByText("person@example.com")).toBeInTheDocument();
  });

  it("lets a switch complete while startup quota refresh is still pending", async () => {
    const target = { ...chatAccount, id: "account-2", email: "other@example.com" };
    mocks.listAccounts.mockResolvedValue([chatAccount, target]);
    mocks.accountQuota.mockImplementation(async (id: string) => ({ account_id: id, status: "unknown" }));
    mocks.refreshAccountQuota.mockImplementation(() => new Promise<QuotaView>(() => undefined));
    mocks.switchAccount.mockResolvedValue({ account: { ...target, active: true } });
    render(<App />);

    await waitFor(() => expect(mocks.refreshAccountQuota).toHaveBeenCalledWith("account-1", true));
    await userEvent.click(screen.getByRole("button", { name: "Switch to other@example.com" }));

    await waitFor(() => expect(screen.getByRole("button", { name: "Current account: other@example.com" })).toBeDisabled());
    expect(mocks.appSnapshot).toHaveBeenCalledOnce();
  });

  it("lets the user cancel a browser OAuth flow", async () => {
    mocks.cancelOAuth.mockImplementation(async () => {
      mocks.oauthStatus.mockResolvedValue({ status: "cancelled" });
    });
    render(<App />);
    await screen.findByRole("heading", { name: "0 saved accounts" });

    await userEvent.click(screen.getByRole("button", { name: /^Add account$/ }));
    const dialog = await screen.findByRole("dialog", { name: "Add a Codex account" });
    await userEvent.click(within(dialog).getByRole("button", { name: /Sign in to add an account/ }));
    expect(await screen.findByText("Finish sign-in in your browser")).toBeInTheDocument();
    expect(mocks.openOAuth).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Cancel sign-in" }));
    await waitFor(() => expect(mocks.cancelOAuth).toHaveBeenCalledWith("login-1"));
    expect(await screen.findByText("Sign-in cancelled")).toBeInTheDocument();
  });

  it("keeps a browser sign-in pending after a temporary status read failure", async () => {
    mocks.oauthStatus
      .mockRejectedValueOnce(new Error("temporary status outage"))
      .mockResolvedValue({ status: "complete", account: chatAccount });
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: /^Add account$/ }));
    await userEvent.click(screen.getByRole("button", { name: /Sign in to add an account/ }));

    expect(await screen.findByText(/cannot check the sign-in result right now/i)).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Add a Codex account" })).toBeInTheDocument();
    expect(mocks.startOAuth).toHaveBeenCalledOnce();
    expect(mocks.cancelOAuth).not.toHaveBeenCalled();
    expect(await screen.findByText("person@example.com was imported safely.", {}, { timeout: 5000 })).toBeInTheDocument();
    expect(mocks.oauthStatus).toHaveBeenCalledTimes(2);
  });

  it("finishes a completed browser sign-in and retries saving without a new authorization", async () => {
    mocks.oauthStatus
      .mockResolvedValueOnce({ status: "finishing" })
      .mockResolvedValueOnce({ status: "failed", code: "save_failed", retryable: true })
      .mockResolvedValue({ status: "complete", account: { ...chatAccount, needs_apply: true }, cleanup_warning: false });
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: /^Add account$/ }));
    await userEvent.click(screen.getByRole("button", { name: /Sign in to add an account/ }));
    expect(await screen.findByText("Finishing sign-in…")).toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: "Retry saving" }));
    expect(mocks.retryOAuth).toHaveBeenCalledWith("login-1");
    expect(mocks.startOAuth).toHaveBeenCalledOnce();
    expect(await screen.findByText(/Sign-in saved. Switch to this account/)).toBeInTheDocument();
  });

  it("removes a non-current saved account while keeping the identity and inline retry error clear", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.removeSavedAccount.mockRejectedValueOnce(
      new Error("Another GSwitch operation is already in progress"),
    );
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });

    await userEvent.click(screen.getByLabelText("More actions for person@example.com"));
    await userEvent.click(screen.getByRole("button", { name: "Remove person@example.com" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove person@example.com?" });
    expect(within(dialog).getByText("person@example.com")).toBeInTheDocument();
    expect(within(dialog).getByText("Personal")).toBeInTheDocument();
    expect(within(dialog).getByText(/removes only the copy saved in GSwitch/)).toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "Another GSwitch operation is in progress. Wait for it to finish, then try again.",
    );
    expect(within(dialog).getByRole("button", { name: "Remove" })).toBeEnabled();
    expect(mocks.removeSavedAccount).toHaveBeenCalledWith("account-1");
  });

  it("explains that an account with an unfinished reset cannot be removed yet", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.removeSavedAccount.mockRejectedValueOnce(
      "This account has an unfinished reset; resolve it before removing the account",
    );
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });

    await userEvent.click(screen.getByLabelText("More actions for person@example.com"));
    await userEvent.click(screen.getByRole("button", { name: "Remove person@example.com" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove person@example.com?" });
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "This account has an unfinished reset. Resolve it before removing the account.",
    );
  });

  it("keeps the current account protected from removal", async () => {
    const activeAccount = { ...chatAccount, active: true };
    mocks.listAccounts.mockResolvedValue([activeAccount]);
    mocks.liveAccount.mockResolvedValue({
      status: "ready",
      credential_store: "file",
      account: activeAccount,
    });
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    render(<App />);

    await screen.findByRole("heading", { name: "person@example.com" });
    await userEvent.click(screen.getByLabelText("More actions for person@example.com"));
    expect(screen.getByRole("button", { name: "Remove person@example.com" })).toBeDisabled();
    expect(mocks.removeSavedAccount).not.toHaveBeenCalled();
  });

  it("closes an account menu on an outside press, Escape, or another menu", async () => {
    const otherAccount = { ...chatAccount, id: "account-2", email: "other@example.com" };
    mocks.listAccounts.mockResolvedValue([chatAccount, otherAccount]);
    const user = userEvent.setup();
    render(<App />);
    const trigger = await screen.findByRole("button", { name: "More actions for person@example.com" });
    const menuItem = () => screen.queryByRole("button", { name: "Remove person@example.com" });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(menuItem()).not.toBeInTheDocument();

    await user.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(menuItem()).toBeInTheDocument();
    await user.click(screen.getByRole("heading", { name: "2 saved accounts" }));
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(menuItem()).not.toBeInTheDocument();

    await user.click(trigger);
    await user.keyboard("{Escape}");
    expect(menuItem()).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();

    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: "More actions for other@example.com" }));
    expect(menuItem()).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove other@example.com" })).toBeInTheDocument();
  });

  it("closes an account menu when keyboard focus leaves it or an action is chosen", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    // userEvent.setup() installs its own clipboard, so stub it afterwards.
    const user = userEvent.setup();
    const copyEmail = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, value: { writeText: copyEmail } });
    render(<App />);
    const trigger = await screen.findByRole("button", { name: "More actions for person@example.com" });

    trigger.focus();
    await user.keyboard("{Enter}");
    await user.tab();
    expect(screen.getByRole("button", { name: "Copy email" })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Sign in again" })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Remove person@example.com" })).toHaveFocus();
    await user.tab();
    expect(screen.queryByRole("button", { name: "Copy email" })).not.toBeInTheDocument();

    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: "Copy email" }));
    expect(copyEmail).toHaveBeenCalledWith("person@example.com");
    expect(screen.queryByRole("button", { name: "Copy email" })).not.toBeInTheDocument();

    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: "Remove person@example.com" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove person@example.com?" });
    expect(screen.queryByRole("button", { name: "Copy email" })).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(trigger).toHaveFocus();
  });

  it("closes the language menu on an outside press", async () => {
    const user = userEvent.setup();
    render(<App />);
    const heading = await screen.findByRole("heading", { name: "0 saved accounts" });
    const trigger = screen.getByRole("button", { name: "Language" });

    await user.click(trigger);
    expect(screen.getByRole("button", { name: "English" })).toBeInTheDocument();
    await user.click(heading);
    expect(screen.queryByRole("button", { name: "English" })).not.toBeInTheDocument();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
  });

  it("localizes the remove-operation retry prompt in Simplified Chinese", async () => {
    window.localStorage.setItem("gswitch.language", "zh-CN");
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.removeSavedAccount.mockRejectedValueOnce(
      new Error("Another GSwitch operation is already in progress"),
    );
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });

    await userEvent.click(screen.getByLabelText("person@example.com 的更多操作"));
    await userEvent.click(screen.getByRole("button", { name: "移除 person@example.com" }));
    const dialog = await screen.findByRole("dialog", { name: "移除 person@example.com？" });
    await userEvent.click(within(dialog).getByRole("button", { name: "移除" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "GSwitch 正在执行其他操作。请等待完成后重试。",
    );
  });

  it("keeps Wake visible until completion and shows only this operation's result", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    let resolveWake: ((result: { id: string; status: "completed"; results: Array<{ account_id: string; label: string; result: "failed"; request_state: "not_sent"; message: string }> }) => void) | undefined;
    mocks.wakeOperation.mockImplementation(() => new Promise((resolve) => { resolveWake = resolve; }));
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });

    await userEvent.click(screen.getByRole("button", { name: "Wake all" }));
    const dialog = await screen.findByRole("dialog", { name: "Wake" });
    expect(within(dialog).queryByRole("button", { name: "Done" })).not.toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /Close Wake/ })).toBeDisabled();
    expect(within(dialog).getByText("Waking…")).toBeInTheDocument();
    expect(within(dialog).getByText("Processed 0")).toBeInTheDocument();
    expect(within(dialog).queryByText("Working…")).not.toBeInTheDocument();
    expect(within(dialog).queryByText(/Sending a Codex request/)).not.toBeInTheDocument();
    await waitFor(() => expect(resolveWake).toBeDefined());
    resolveWake?.({
      id: "wake-all",
      status: "completed",
      results: [{ account_id: "account-1", label: "Legacy label", result: "failed", request_state: "not_sent", message: "raw provider error text" }],
    });
    const resultDialog = dialog;
    expect(await within(resultDialog).findByText("person@example.com")).toBeInTheDocument();
    expect(within(resultDialog).getByText("Personal")).toBeInTheDocument();
    expect(within(resultDialog).getByText("Not sent · Could not complete Wake.")).toBeInTheDocument();
    expect(within(resultDialog).getByText(/1 failed/)).toBeInTheDocument();
    expect(within(resultDialog).queryByText("raw provider error text")).not.toBeInTheDocument();
    await userEvent.click(within(resultDialog).getByRole("button", { name: "Done" }));
    expect(await screen.findByRole("button", { name: "Wake all" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Last result" })).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Wake" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Wake all" }));
    expect(await screen.findByRole("dialog", { name: "Wake" })).toBeInTheDocument();
    expect(mocks.startWakeAll).toHaveBeenCalledTimes(2);
  });

  it("distinguishes confirmed replies, rejected requests and uncertain delivery", async () => {
    const saved = [chatAccount,
      { ...chatAccount, id: "account-2", email: "other@example.com" },
      { ...chatAccount, id: "account-3", email: "third@example.com" }];
    mocks.listAccounts.mockResolvedValue(saved);
    mocks.accountQuota.mockImplementation(async (id: string) => ({ ...staleQuota, account_id: id, status: "fresh" }));
    mocks.wakeOperation.mockResolvedValue({
      id: "wake-all",
      status: "completed",
      results: [
        { account_id: "account-1", label: "Personal", result: "reply_received", request_state: "sent" },
        { account_id: "account-2", label: "Other", result: "rate_limited", request_state: "sent" },
        { account_id: "account-3", label: "Third", result: "sent_not_confirmed", request_state: "may_have_sent" },
      ],
    });
    render(<App />);
    await screen.findByRole("heading", { name: "3 saved accounts" });
    await userEvent.click(screen.getByRole("button", { name: "Wake all" }));
    const dialog = await screen.findByRole("dialog", { name: "Wake" });
    expect(await within(dialog).findByText("1 succeeded · 1 unconfirmed · 1 failed")).toBeInTheDocument();
    expect(within(dialog).getByText("Unconfirmed. The request may have been sent.")).toBeInTheDocument();
    expect(within(dialog).getByText("Succeeded")).toBeInTheDocument();
    expect(within(dialog).getByText("Limit reached. Try later.")).toBeInTheDocument();
    expect(within(dialog).queryByText(/Request sent/)).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: /Retry Wake/ })).not.toBeInTheDocument();
  });

  it("shows stopped accounts without implying their requests were sent", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.wakeOperation.mockResolvedValue({
      id: "wake-all",
      status: "cancelled",
      results: [{ account_id: "account-1", label: "Personal", result: "cancelled", request_state: "not_sent" }],
    });
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });
    await userEvent.click(screen.getByRole("button", { name: "Wake all" }));
    const dialog = await screen.findByRole("dialog", { name: "Wake" });
    expect(await within(dialog).findByText("Stopped")).toBeInTheDocument();
    expect(within(dialog).getByText("1 not run")).toBeInTheDocument();
    expect(within(dialog).getByText("Not run.")).toBeInTheDocument();
    expect(within(dialog).queryByText(/Not sent · Not run/)).not.toBeInTheDocument();
  });

  it("shows a sanitized Wake request failure and its HTTP status", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.wakeOperation.mockResolvedValue({
      id: "wake-all",
      status: "completed",
      results: [{ account_id: "account-1", label: "Personal", result: "invalid_request", request_state: "sent", http_status: 400, message: "private provider details" }],
    });
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });
    await userEvent.click(screen.getByRole("button", { name: "Wake all" }));
    const dialog = await screen.findByRole("dialog", { name: "Wake" });
    expect(await within(dialog).findByText("Request parameters were rejected. (HTTP 400)")).toBeInTheDocument();
    expect(within(dialog).queryByText(/private provider details/)).not.toBeInTheDocument();
  });

  it("explains the same Wake request failure in Chinese", async () => {
    Object.defineProperty(window.navigator, "language", { configurable: true, value: "zh-CN" });
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.wakeOperation.mockResolvedValue({
      id: "wake-all",
      status: "completed",
      results: [{ account_id: "account-1", label: "Personal", result: "invalid_request", request_state: "sent", http_status: 400 }],
    });
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });
    await userEvent.click(screen.getByRole("button", { name: "全部唤醒" }));
    expect(await screen.findByText("请求参数未被接受。 (HTTP 400)")).toBeInTheDocument();
  });

  it("shows concise Chinese Wake results for a confirmed reply and a failure", async () => {
    Object.defineProperty(window.navigator, "language", { configurable: true, value: "zh-CN" });
    mocks.listAccounts.mockResolvedValue([
      chatAccount,
      { ...chatAccount, id: "account-2", email: "other@example.com" },
    ]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.wakeOperation.mockResolvedValue({
      id: "wake-all",
      status: "completed",
      results: [
        { account_id: "account-1", label: "Personal", result: "reply_received", request_state: "sent" },
        { account_id: "account-2", label: "Other", result: "needs_sign_in", request_state: "not_sent" },
      ],
    });
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });
    await userEvent.click(screen.getByRole("button", { name: "全部唤醒" }));
    const dialog = await screen.findByRole("dialog", { name: "唤醒" });
    expect(await within(dialog).findByText("已唤醒 1 · 失败 1")).toBeInTheDocument();
    expect(within(dialog).getByText("已唤醒")).toBeInTheDocument();
    expect(within(dialog).getByText("未发送 · 请重新登录。")).toBeInTheDocument();
  });

  it("offers a manual older-model retry only after a definite model rejection", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.startWake.mockResolvedValue({ operation_id: "wake-alternate" });
    mocks.wakeOperation.mockImplementation(async (operationId: string) => ({
      id: operationId,
      status: "completed",
      alternate_model: operationId === "wake-alternate",
      results: [{
        account_id: "account-1",
        label: "Personal",
        result: "model_unavailable",
        request_state: "sent",
      }],
    }));
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });
    await userEvent.click(screen.getByRole("button", { name: "Wake all" }));
    const dialog = await screen.findByRole("dialog", { name: "Wake" });
    expect(await within(dialog).findByText("GPT-6 Luna unavailable.")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Retry Wake for person@example.com with GPT-5.6 Luna" }));
    expect(mocks.startWake).toHaveBeenCalledWith("account-1", true);
    await within(dialog).findByText("GPT-5.6 Luna unavailable.");
    await waitFor(() => expect(within(dialog).queryByRole("button", { name: /Retry Wake/ })).not.toBeInTheDocument());
  });

  it("retains a running Wake operation when a status read briefly fails", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.wakeOperation
      .mockRejectedValueOnce(new Error("temporary status outage"))
      .mockResolvedValue({
        id: "wake-all",
        status: "completed",
        results: [{ account_id: "account-1", label: "Personal", result: "reply_received", request_state: "sent" }],
      });
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });
    await userEvent.click(screen.getByRole("button", { name: "Wake all" }));

    const dialog = await screen.findByRole("dialog", { name: "Wake" });
    expect(await within(dialog).findByText(/cannot check the Wake result right now/i)).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Done" })).not.toBeInTheDocument();
    expect(await within(dialog).findByText("1 succeeded", {}, { timeout: 5000 })).toBeInTheDocument();
    expect(mocks.startWakeAll).toHaveBeenCalledOnce();
    expect(mocks.wakeOperation).toHaveBeenCalledTimes(2);
  });

  it("reports one reply per account even when all five have available quota", async () => {
    const saved = Array.from({ length: 5 }, (_, index) => ({
      ...chatAccount,
      id: `account-${index + 1}`,
      email: `person${index + 1}@example.com`,
    }));
    mocks.listAccounts.mockResolvedValue(saved);
    mocks.accountQuota.mockImplementation(async (id: string) => ({ ...staleQuota, account_id: id, status: "fresh" }));
    mocks.wakeOperation.mockResolvedValue({
      id: "wake-all",
      status: "completed",
      results: saved.map((account) => ({ account_id: account.id, label: account.label, result: "reply_received", request_state: "sent" })),
    });
    render(<App />);
    await screen.findByRole("heading", { name: "5 saved accounts" });
    await userEvent.click(screen.getByRole("button", { name: "Wake all" }));
    const dialog = await screen.findByRole("dialog", { name: "Wake" });
    expect(await within(dialog).findByText("5 succeeded")).toBeInTheDocument();
    expect(within(dialog).getAllByText("Succeeded")).toHaveLength(5);
    expect(screen.getByRole("button", { name: "Wake all" })).toBeEnabled();
  });

  it("opens the affected saved account from a Wake sign-in failure", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue({ ...staleQuota, status: "fresh" });
    mocks.wakeOperation.mockResolvedValue({
      id: "wake-all", status: "completed",
      results: [{ account_id: chatAccount.id, label: chatAccount.label, result: "needs_sign_in", request_state: "sent" }],
    });
    render(<App />);
    await screen.findByRole("heading", { name: "person@example.com" });
    await userEvent.click(screen.getByRole("button", { name: "Wake all" }));
    const dialog = await screen.findByRole("dialog", { name: "Wake" });
    await userEvent.click(await within(dialog).findByRole("button", { name: "Sign in again" }));
    expect(mocks.startOAuth).toHaveBeenCalledWith(chatAccount.id);
    expect(await screen.findByRole("dialog", { name: "Sign in again · person@example.com" })).toBeInTheDocument();
  });

  it("shows a per-operation Wake surface instead of silently running in the background", async () => {
    mocks.listAccounts.mockResolvedValue([chatAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    render(<App />);
    await screen.findByText("Personal");

    await userEvent.click(screen.getByRole("button", { name: /^Wake all$/ }));
    expect(await screen.findByRole("dialog", { name: "Wake" })).toBeInTheDocument();
    expect(screen.getByText("Waking…")).toBeInTheDocument();
    expect(mocks.startWakeAll).toHaveBeenCalledOnce();
  });

  it("selects saved accounts, wakes only selected ChatGPT accounts, and exports through a warning", async () => {
    const apiAccount: AccountView = {
      id: "api-1",
      label: "Key",
      kind: "api_key",
      active: false,
    };
    mocks.listAccounts.mockResolvedValue([chatAccount, apiAccount]);
    mocks.accountQuota.mockResolvedValue(staleQuota);
    render(<App />);
    await screen.findByText("Personal");

    const heading = screen.getByRole("heading", { name: "2 saved accounts" });
    expect(heading.closest("section")?.querySelector(".eyebrow")).not.toBeInTheDocument();
    const selectButton = screen.getByRole("button", { name: "Select accounts" });
    expect(selectButton).toHaveClass("button-secondary", "section-select-button");
    expect(selectButton).toHaveAttribute("aria-pressed", "false");
    selectButton.focus();
    expect(selectButton).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    expect(screen.getByRole("button", { name: "Done" })).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(screen.getByRole("button", { name: "Select all" }));
    expect(screen.getAllByRole("checkbox")).toHaveLength(2);
    for (const checkbox of screen.getAllByRole("checkbox")) {
      expect(checkbox).toBeChecked();
    }
    await userEvent.click(screen.getByRole("button", { name: "Clear" }));
    const accountChoice = screen.getByRole("checkbox", { name: "Select person@example.com" });
    expect(accountChoice).not.toBeChecked();
    await userEvent.click(accountChoice);
    await userEvent.click(screen.getByRole("button", { name: "Wake" }));
    await waitFor(() => expect(mocks.startWakeSelected).toHaveBeenCalledWith(["account-1"]));
    expect(await screen.findByRole("dialog", { name: "Wake" })).toBeInTheDocument();
    const wakeDialog = await screen.findByRole("dialog", { name: "Wake" });
    await userEvent.click(await within(wakeDialog).findByRole("button", { name: "Done" }));

    await userEvent.click(screen.getByRole("button", { name: "Export" }));
    const exportDialog = await screen.findByRole("dialog", { name: "Export 1 account" });
    expect(within(exportDialog).getByText(/The exported file isn't encrypted/)).toBeInTheDocument();
    await userEvent.click(within(exportDialog).getByRole("button", { name: "Export" }));
    expect(mocks.exportAccounts).not.toHaveBeenCalled();
    await pastConfirmGuard();
    await userEvent.click(within(exportDialog).getByRole("button", { name: "Confirm export" }));
    await waitFor(() => expect(mocks.exportAccounts).toHaveBeenCalledWith(["account-1"]));
    expect(await screen.findByText("Export complete: 1 selected. Keep this unencrypted file private.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
    expect(screen.getByRole("button", { name: "Select accounts" })).toHaveAttribute("aria-pressed", "false");
  });

  it("offers a signed update once and keeps Later local to the current session", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    updaterMocks.check.mockResolvedValue({ version: "1.0.2" });
    render(<App />);

    expect(await screen.findByText("GSwitch 1.0.2 is available")).toBeInTheDocument();
    expect(screen.queryByText(/signed update never touches/i)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Later" }));
    expect(screen.queryByText("GSwitch 1.0.2 is available")).not.toBeInTheDocument();
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

    await userEvent.click(await screen.findByRole("button", { name: "Update GSwitch" }));
    await waitFor(() => expect(downloadAndInstall).toHaveBeenCalledOnce());
    expect(await screen.findByText("Restart GSwitch to finish updating")).toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: "Restart now" }));
    expect(updaterMocks.relaunch).toHaveBeenCalledOnce();
  });
});
