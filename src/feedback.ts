import type { Translator } from "./i18n";
import type { ResetCreditFailure, ResetCreditFailureCode } from "./types";

export interface Notice {
  kind: "success" | "error" | "info";
  text: string;
}

const resetFailureCodes = new Set<ResetCreditFailureCode>([
  "operation_busy",
  "codex_open",
  "recovery_required",
  "details_unavailable",
  "credits_changed",
  "not_started",
  "provider_rejected",
  "result_unknown",
]);

function asResetCreditFailure(error: unknown): ResetCreditFailure | undefined {
  if (typeof error !== "object" || error === null || !("code" in error)) {
    return undefined;
  }
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" && resetFailureCodes.has(code as ResetCreditFailureCode)
    ? { code: code as ResetCreditFailureCode }
    : undefined;
}

export function friendlyError(t: Translator, error: unknown, fallback = t("error.actionIncomplete")) {
  const resetFailure = asResetCreditFailure(error);
  if (resetFailure) {
    return t(`resetFailure.${resetFailure.code}`);
  }
  const message = String(error);
  if (/Quit Codex|Codex is running|external Codex|Unable to reliably inspect/i.test(message)) {
    return t("error.quitCodex");
  }
  if (/recovery/i.test(message)) {
    return t("error.resolveRecovery");
  }
  if (/file-backed|file store|required/i.test(message)) {
    return t("error.enableFileStore");
  }
  if (/No account files were selected/i.test(message)) {
    return t("error.importNoFiles");
  }
  if (/migration preview is stale|preview is no longer importable|scan again/i.test(message)) {
    return t("error.migrationStale");
  }
  if (/selected Cockpit folder could not be read|selected Cockpit folder is empty/i.test(message)) {
    return t("error.migrationFolder");
  }
  if (/no more than 64|64 MiB batch limit/i.test(message)) {
    return t("error.importBatchLimit");
  }
  if (/No supported|not valid JSON|Invalid auth/i.test(message)) {
    return t("error.importUnsupported");
  }
  return fallback;
}
