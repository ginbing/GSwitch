import { invoke } from "@tauri-apps/api/core";
import type {
  AccountView,
  AppSnapshot,
  CodexCliInfo,
  ExportResult,
  ImportResult,
  LiveAccountView,
  MigrationPreview,
  OAuthLoginStart,
  OAuthLoginStatus,
  QuotaView,
  ResetCreditOutcome,
  RuntimeInfo,
  SwitchOutcome,
  WakeOperationView,
  WakeStart,
} from "./types";

export type UpdateDelivery = "installer_exits" | "relaunch_required" | "release_download";

// Keep provider payloads on the Rust side. The WebView only deals with this
// small, display-oriented capacity view.
export const api = {
  runtimeInfo: () => invoke<RuntimeInfo>("get_runtime_info"),
  codexCliInfo: () => invoke<CodexCliInfo>("get_codex_cli_info"),
  updateCodexCli: () => invoke<CodexCliInfo>("update_codex_cli"),
  openCodexCliGuide: () => invoke<void>("open_codex_cli_guide"),
  appSnapshot: () => invoke<AppSnapshot>("get_app_snapshot"),
  listAccounts: () => invoke<AccountView[]>("list_accounts"),
  resetDamagedAccountStore: () =>
    invoke<void>("reset_damaged_account_store"),
  liveAccount: () => invoke<LiveAccountView>("get_live_account_state"),
  startOAuth: (targetId?: string) => invoke<OAuthLoginStart>("start_oauth_login", { targetId }),
  oauthStatus: (loginId: string) =>
    invoke<OAuthLoginStatus>("get_oauth_login_status", { loginId }),
  cancelOAuth: (loginId: string) =>
    invoke<void>("cancel_oauth_login", { loginId }),
  openOAuth: (loginId: string) =>
    invoke<void>("open_oauth_login", { loginId }),
  importAuthJson: (rawJson: string, label?: string) =>
    invoke<AccountView>("import_auth_json", { rawJson, label }),
  importAuthFile: (path: string) =>
    invoke<ImportResult>("import_auth_file", { path }),
  importAuthFiles: (paths: string[]) =>
    invoke<ImportResult>("import_auth_files", { paths }),
  exportAccounts: (selectedIds: string[]) =>
    invoke<ExportResult>("export_accounts", { selectedIds }),
  discoverLocalAccounts: (customRoot?: string) =>
    invoke<MigrationPreview>("discover_local_accounts", { customRoot }),
  importLocalAccounts: (customRoot: string | undefined, selectedIds: string[]) =>
    invoke<ImportResult>("import_local_accounts", {
      customRoot,
      selectedIds,
    }),
  importApiKey: (apiKey: string, label?: string) =>
    invoke<AccountView>("import_api_key", { apiKey, label }),
  saveCurrentAccount: () => invoke<AccountView>("save_current_account"),
  enableAccountSwitching: () =>
    invoke<boolean>("enable_account_switching"),
  switchAccount: (targetId: string) =>
    invoke<SwitchOutcome>("switch_account", { targetId }),
  recoverPendingSwitch: () => invoke<void>("recover_pending_switch"),
  removeSavedAccount: (id: string) =>
    invoke<void>("remove_saved_account", { id }),
  accountQuota: (id: string) => invoke<QuotaView>("get_account_quota", { id }),
  refreshAccountQuota: (id: string, background = false) =>
    invoke<QuotaView>("refresh_account_quota", { id, background }),
  redeemEarliestResetCredit: (id: string) =>
    invoke<ResetCreditOutcome>("redeem_earliest_reset_credit", { id }),
  recoverPendingResetCredit: () =>
    invoke<ResetCreditOutcome>("recover_pending_reset_credit"),
  startWake: (id: string) => invoke<WakeStart>("start_wake", { id }),
  startWakeAll: () => invoke<WakeStart>("start_wake_all"),
  startWakeSelected: (selectedIds: string[]) =>
    invoke<WakeStart>("start_wake_selected", { selectedIds }),
  wakeOperation: (operationId: string) =>
    invoke<WakeOperationView>("get_wake_operation", { operationId }),
  cancelWake: (operationId: string) =>
    invoke<void>("cancel_wake", { operationId }),
  updateDelivery: () => invoke<UpdateDelivery>("get_update_delivery"),
  openLatestRelease: () => invoke<void>("open_latest_release"),
};
