import { invoke } from "@tauri-apps/api/core";
import type {
  QuotaView,
  ResetCreditOutcome,
  WakeOperationView,
  WakeStart,
} from "./types";

// Keep provider payloads on the Rust side. The WebView only deals with this
// small, display-oriented capacity view.
export const api = {
  accountQuota: (id: string) => invoke<QuotaView>("get_account_quota", { id }),
  refreshAccountQuota: (id: string) => invoke<QuotaView>("refresh_account_quota", { id }),
  redeemEarliestResetCredit: (id: string) =>
    invoke<ResetCreditOutcome>("redeem_earliest_reset_credit", { id }),
  recoverPendingResetCredit: () =>
    invoke<ResetCreditOutcome>("recover_pending_reset_credit"),
  startWake: (id: string, model?: string) =>
    invoke<WakeStart>("start_wake", { id, model }),
  startWakeAll: () => invoke<WakeStart>("start_wake_all"),
  wakeOperation: (operationId: string) =>
    invoke<WakeOperationView>("get_wake_operation", { operationId }),
  cancelWake: (operationId: string) =>
    invoke<void>("cancel_wake", { operationId }),
};
