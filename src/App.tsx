import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  ArrowRightLeft,
  Check,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  Copy,
  Download,
  FileJson,
  FolderOpen,
  Globe2,
  Info,
  KeyRound,
  ListChecks,
  LoaderCircle,
  MoreHorizontal,
  Plus,
  RefreshCw,
  ShieldAlert,
  Terminal,
  Trash2,
  Upload,
  X,
  Zap,
} from "lucide-react";
import {
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { api, type UpdateDelivery } from "./api";
import {
  createTranslator,
  formatCompactExpiry,
  formatDateTime,
  formatDateTimeWithRelative,
  formatNumber,
  formatPercent,
  formatQuotaResetTime,
  formatRelativeTime,
  readLanguagePreference,
  resolveLocale,
  saveLanguagePreference,
  type LanguagePreference,
  type Translator,
} from "./i18n";
import { RefreshLanes } from "./refreshLanes";
import { quotaRefreshDue } from "./refreshPolicy";
import type {
  AccountView,
  CodexCliInfo,
  CodexCliUpdateFailure,
  LiveAccountView,
  MigrationCandidate,
  MigrationPreview,
  OAuthFailureCode,
  OAuthLoginStart,
  OAuthLoginStatus,
  PendingResetView,
  QuotaRefreshFailureCode,
  QuotaView,
  QuotaWindow,
  ResetCreditDetail,
  ResetCreditFailure,
  ResetCreditFailureCode,
  ResetCreditOutcome,
  StorageView,
  SwitchFailure,
  SwitchFailureCode,
  WakeOperationView,
} from "./types";
import { updater, type AvailableUpdate } from "./updater";

type Dialog =
  | "add"
  | "enable-switching"
  | "recover-switch"
  | "recover-reset-credit"
  | "storage-recovery"
  | "reset"
  | "remove"
  | "export"
  | "wake"
  | "codex-cli"
  | null;

type AddMethod = "start" | "oauth" | "json" | "api-key" | "migration";
type AccountBusyAction = "refresh" | "wake" | "switch";

interface QuotaRefreshEntry {
  promise: Promise<QuotaView>;
  // A user's refresh, as opposed to an automatic read.
  manual: boolean;
  // Whether its provider request has begun, so it can no longer be replaced.
  started: boolean;
}

interface Notice {
  kind: "success" | "error" | "info";
  text: string;
}

interface OAuthFlow extends OAuthLoginStart {
  status: OAuthLoginStatus;
}

type UpdatePhase = "available" | "downloading" | "installing" | "ready" | "error";

interface PendingUpdate {
  update: AvailableUpdate;
  delivery: UpdateDelivery;
}

function fileFilters(t: Translator) {
  return [{ name: t("file.accountExports"), extensions: ["json"] }];
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

function friendlyError(t: Translator, error: unknown, fallback = t("error.actionIncomplete")) {
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

const switchFailureCodes = new Set<SwitchFailureCode>([
  "operation_busy",
  "codex_open",
  "account_needs_sign_in",
  "file_store_required",
  "credentials_changed",
  "recovery_required",
  "local_verification_failed",
  "codex_app_server_unavailable",
  "codex_config_unavailable",
  "codex_config_cleanup_failed",
  "current_credential_unreadable",
  "current_account_not_saved",
  "target_check_unavailable",
  "target_workspace_mismatch",
  "post_write_verification_failed",
  "verification_failed",
]);

function asSwitchFailure(error: unknown): SwitchFailure | undefined {
  if (typeof error !== "object" || error === null || !("code" in error)) {
    return undefined;
  }
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" && switchFailureCodes.has(code as SwitchFailureCode)
    ? { code: code as SwitchFailureCode }
    : undefined;
}

function switchFailureMessage(t: Translator, error: unknown, account: AccountView) {
  const failure = asSwitchFailure(error);
  const messages = {
    operation_busy: "switch.error.operationBusy",
    codex_open: "switch.error.codexOpen",
    account_needs_sign_in: "switch.error.needsSignIn",
    file_store_required: "switch.error.fileStoreRequired",
    credentials_changed: "switch.error.credentialsChanged",
    recovery_required: "switch.error.recoveryRequired",
    local_verification_failed: "switch.error.localVerificationFailed",
    codex_app_server_unavailable: "switch.error.codexAppServerUnavailable",
    codex_config_unavailable: "switch.error.codexConfigUnavailable",
    codex_config_cleanup_failed: "switch.error.codexConfigCleanupFailed",
    current_credential_unreadable: "switch.error.currentCredentialUnreadable",
    current_account_not_saved: "switch.error.currentAccountNotSaved",
    target_check_unavailable: "switch.error.targetCheckUnavailable",
    target_workspace_mismatch: "switch.error.targetWorkspaceMismatch",
    post_write_verification_failed: "switch.error.postWriteVerificationFailed",
    verification_failed: "switch.error.verificationFailed",
  } as const;
  const key = failure ? messages[failure.code] : messages.verification_failed;
  return t(key, { name: accountPrimaryName(account) });
}

const quotaFailureCodes = new Set<QuotaRefreshFailureCode>([
  "operation_busy", "codex_account_unknown", "authentication", "rate_limited",
  "manual_refresh_needed", "network", "service", "invalid_response", "identity_mismatch", "unavailable",
]);

function quotaFailureCode(error: unknown): QuotaRefreshFailureCode {
  if (typeof error !== "object" || error === null || !("code" in error)) return "unavailable";
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" && quotaFailureCodes.has(code as QuotaRefreshFailureCode)
    ? code as QuotaRefreshFailureCode : "unavailable";
}

function quotaFailureMessage(t: Translator, code: QuotaRefreshFailureCode) {
  const keys = {
    operation_busy: "quota.failureBusy",
    codex_account_unknown: "quota.failureCodexUnknown",
    authentication: "quota.failureAuthentication",
    manual_refresh_needed: "quota.failureManualRefresh",
    rate_limited: "quota.failureRateLimited",
    network: "quota.failureNetwork",
    service: "quota.failureService",
    invalid_response: "quota.failureInvalidResponse",
    identity_mismatch: "quota.failureIdentity",
    unavailable: "quota.failureUnavailable",
  } as const;
  return t(keys[code]);
}

function oauthFailureMessage(t: Translator, code: OAuthFailureCode) {
  const keys = {
    not_completed: "oauth.failureNotCompleted",
    timed_out: "oauth.failureTimedOut",
    identity_mismatch: "oauth.failureIdentity",
    authentication: "oauth.failureAuthentication",
    network: "oauth.failureNetwork",
    local_codex: "oauth.failureLocalCodex",
    verification_failed: "oauth.failureVerification",
    save_failed: "oauth.failureSave",
    unavailable: "oauth.failureUnavailable",
  } as const;
  return t(keys[code]);
}

function removeFailureMessage(t: Translator, error: unknown) {
  const message = String(error);
  if (/another gswitch operation is already in progress/i.test(message)) {
    return t("remove.operationBusy");
  }
  if (/active codex account cannot be removed/i.test(message)) {
    return t("remove.activeAccount");
  }
  if (/recover a previous switch before starting another/i.test(message)) {
    return t("remove.recoveryRequired");
  }
  if (/unfinished reset/i.test(message)) {
    return t("remove.pendingReset");
  }
  return t("remove.failed");
}

// A projection Rust saved after this view loaded it, such as the quota read
// after a reset, replaces the in-memory copy. An older saved copy does not.
function mergeNewerQuotas(current: Record<string, QuotaView>, saved: Record<string, QuotaView>) {
  const next = { ...current };
  for (const [accountId, quota] of Object.entries(saved)) {
    const held = next[accountId];
    if (!held || (quota.snapshot?.fetched_at_unix_ms ?? 0) > (held.snapshot?.fetched_at_unix_ms ?? 0)) {
      next[accountId] = quota;
    }
  }
  return next;
}

function accountGridHasGlobalMutation(busy: string | null) {
  if (!busy) {
    return false;
  }

  return [
    "import",
    "migration-import",
    "import-json",
    "import-key",
    "save-current",
    "enable-switching",
    "recover-switch",
    "recover-reset-credit",
    "reset-damaged-store",
    "codex-cli-update",
  ].includes(busy) || busy.startsWith("switch:") || busy.startsWith("reset:") || busy.startsWith("remove:");
}

function cliUpdateFailureCode(error: unknown): CodexCliUpdateFailure {
  const code = String(error);
  if (["not_installed", "unsupported", "busy", "codex_open", "update_failed", "verification_failed"].includes(code)) {
    return code as CodexCliUpdateFailure;
  }
  return "update_failed";
}

function codexQuotaWindows(quota: QuotaView | undefined): QuotaWindow[] {
  return quota?.snapshot?.buckets
    .filter((bucket) => bucket.kind === "codex")
    .flatMap((bucket) => bucket.windows) ?? [];
}

function quotaWindowLabel(window: QuotaWindow, t: Translator): string {
  if (window.kind === "five_hour") return t("quota.fiveHour");
  if (window.kind === "weekly") return t("quota.weekly");
  const minutes = window.window_duration_mins;
  if (minutes === 50_400) return t("quota.fiveWeeks");
  if (minutes && minutes % 10_080 === 0) return t("quota.durationWeeks", { count: minutes / 10_080 });
  if (minutes && minutes % 1_440 === 0) return t("quota.durationDays", { count: minutes / 1_440 });
  if (minutes && minutes % 60 === 0) return t("quota.durationHours", { count: minutes / 60 });
  return t("quota.otherWindow");
}

function accountPlan(account: AccountView, t: Translator) {
  return account.kind === "api_key" ? t("account.apiKey") : account.plan_type ? planLabel(account.plan_type, t) : t("account.chatGpt");
}

function planLabel(plan: string, t: Translator) {
  const key = ({
    free: "plan.free",
    plus: "plan.plus",
    pro: "plan.pro",
    team: "plan.team",
  } as const)[plan.trim().toLowerCase() as "free" | "plus" | "pro" | "team"];
  return key ? t(key) : t("plan.other");
}

function migrationSourceLabel(candidate: MigrationCandidate, t: Translator) {
  return candidate.source === "official_codex"
    ? t("migration.sourceOfficial")
    : t("migration.sourceCockpit");
}

function migrationStateLabel(candidate: MigrationCandidate, t: Translator) {
  if (candidate.state === "new") return t("migration.stateNew");
  if (candidate.state === "already_present") return t("migration.stateAlready");
  return t("migration.stateUnsupported");
}

function accountPrimaryName(account: AccountView) {
  if (account.kind === "chat_gpt" && account.email?.trim()) {
    return account.email.trim();
  }
  return account.label;
}

function accountSecondaryName(account: AccountView, primary: string, t: Translator) {
  if (account.kind === "api_key") {
    return t("account.storedLocally");
  }
  const workspace = account.workspace_name?.trim();
  if (workspace && workspace !== primary) {
    return workspace;
  }
  const legacyLabel = account.label.trim();
  if (!workspace && legacyLabel && legacyLabel !== primary) {
    return legacyLabel;
  }
  return undefined;
}

function wakeResultLabel(result: WakeOperationView["results"][number], alternateModel: boolean, t: Translator) {
  const labels = {
    reply_received: "wake.replyReceived",
    rate_limited: "wake.rateLimited",
    needs_sign_in: "wake.needsSignIn",
    sent_not_confirmed: "wake.sentNotConfirmed",
    model_unavailable: "wake.modelUnavailable",
    invalid_request: "wake.invalidRequest",
    service_unavailable: "wake.serviceUnavailable",
    request_rejected: "wake.requestRejected",
    failed: "wake.failed",
    cancelled: "wake.cancelled",
  } as const;
  const label = t(result.result === "model_unavailable" && alternateModel
    ? "wake.alternateModelUnavailable"
    : labels[result.result]);
  const requestState = result.request_state === "not_sent" && result.result !== "cancelled"
    ? `${t("wake.requestNotSent")} · `
    : "";
  const status = result.http_status ? ` (HTTP ${result.http_status})` : "";
  return `${requestState}${label}${status}`;
}

function wakeSummary(wake: WakeOperationView, t: Translator) {
  const counts = { replied: 0, unconfirmed: 0, failed: 0, cancelled: 0 };
  for (const { result } of wake.results) {
    if (result === "reply_received") counts.replied++;
    else if (result === "sent_not_confirmed") counts.unconfirmed++;
    else if (result === "cancelled") counts.cancelled++;
    else counts.failed++;
  }
  return ([
    ["wake.summaryReplied", counts.replied],
    ["wake.summaryUnconfirmed", counts.unconfirmed],
    ["wake.summaryFailed", counts.failed],
    ["wake.summaryCancelled", counts.cancelled],
  ] as const).filter(([, count]) => count > 0).map(([key, count]) => t(key, { count })).join(" · ");
}

// Irreversible actions take two clicks on the same button, as Codex's own
// reset button does. A second click within 300 ms of the first is ignored so a
// double-click cannot act on its own, and pressing anywhere else disarms it.
const CONFIRM_GUARD_MS = 300;

function useConfirmClick() {
  const [armed, setArmed] = useState<string | null>(null);
  const armedAt = useRef(0);

  useEffect(() => {
    if (armed === null) return;
    const disarmElsewhere = (event: PointerEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest("[data-confirm]")?.getAttribute("data-confirm") !== armed) {
        setArmed(null);
      }
    };
    document.addEventListener("pointerdown", disarmElsewhere, true);
    return () => document.removeEventListener("pointerdown", disarmElsewhere, true);
  }, [armed]);

  const click = (id: string, action: () => void) => {
    if (armed !== id) {
      setArmed(id);
      armedAt.current = Date.now();
      return;
    }
    if (Date.now() - armedAt.current < CONFIRM_GUARD_MS) {
      return;
    }
    setArmed(null);
    action();
  };

  return { armed, click, disarm: () => setArmed(null) };
}

function Modal({
  title,
  children,
  onClose,
  t,
  wide = false,
  dismissible = true,
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
  t: Translator;
  wide?: boolean;
  dismissible?: boolean;
}) {
  const dialogRef = useRef<HTMLElement>(null);

  useEffect(() => {
    const previousFocus = document.activeElement;
    dialogRef.current?.focus();
    return () => {
      if (previousFocus instanceof HTMLElement && previousFocus.isConnected) {
        previousFocus.focus();
      }
    };
  }, []);

  const handleKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      if (dismissible) onClose();
      return;
    }
    if (event.key !== "Tab") return;

    const controls = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), a[href], summary, [tabindex]:not([tabindex="-1"])',
    ) ?? []).filter((control) => (control.tagName === "SUMMARY" || !control.closest("details:not([open])")) && getComputedStyle(control).visibility !== "hidden");
    if (controls.length === 0) {
      event.preventDefault();
      dialogRef.current?.focus();
      return;
    }
    const first = controls[0];
    const last = controls[controls.length - 1];
    if (document.activeElement === dialogRef.current) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    } else if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (dismissible && event.currentTarget === event.target) {
          onClose();
        }
      }}
    >
      <section
        aria-labelledby="modal-title"
        aria-modal="true"
        className={"modal-card" + (wide ? " modal-wide" : "")}
        role="dialog"
        tabIndex={-1}
        ref={dialogRef}
        onKeyDown={handleKeyDown}
      >
        <div className="modal-header">
          <h2 id="modal-title">{title}</h2>
          <div className="modal-header-actions">
            <button aria-label={t("common.closeDialog", { title })} className="icon-button" disabled={!dismissible} onClick={onClose} type="button">
              <X size={18} strokeWidth={2} />
            </button>
          </div>
        </div>
        {children}
      </section>
    </div>
  );
}

function PopoverMenu({
  className,
  label,
  title,
  popoverLabel,
  icon,
  children,
}: {
  className: string;
  label: string;
  title?: string;
  popoverLabel?: string;
  icon: ReactNode;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverId = useId();

  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(false);
    const closeOnOutsidePress = (event: PointerEvent) => {
      if (event.target instanceof Node && !rootRef.current?.contains(event.target)) close();
    };
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return;
      close();
      triggerRef.current?.focus();
    };
    document.addEventListener("pointerdown", closeOnOutsidePress);
    document.addEventListener("keydown", closeOnEscape);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePress);
      document.removeEventListener("keydown", closeOnEscape);
      window.removeEventListener("blur", close);
    };
  }, [open]);

  return (
    <div
      className={className}
      ref={rootRef}
      onBlur={(event) => {
        // WebKit does not focus a clicked button, so a missing target is not
        // proof that focus left the menu; the pointer listener covers that.
        if (event.relatedTarget instanceof Node && !event.currentTarget.contains(event.relatedTarget)) {
          setOpen(false);
        }
      }}
    >
      <button
        aria-controls={open ? popoverId : undefined}
        aria-expanded={open}
        aria-label={label}
        onClick={() => setOpen((current) => !current)}
        ref={triggerRef}
        title={title}
        type="button"
      >
        {icon}
      </button>
      {open ? (
        <div
          aria-label={popoverLabel}
          className={className + "-popover"}
          id={popoverId}
          onClick={(event) => {
            if (!(event.target instanceof Element) || !event.target.closest("button")) return;
            // Return focus first so a dialog opened by this action restores it
            // to the trigger rather than to a removed menu item.
            triggerRef.current?.focus();
            setOpen(false);
          }}
        >
          {children}
        </div>
      ) : null}
    </div>
  );
}

function QuotaResetTime({ timestamp, t, formatLocale }: {
  timestamp: number;
  t: Translator;
  formatLocale: string;
}) {
  const [nowMs, setNowMs] = useState(Date.now);
  useEffect(() => {
    const interval = window.setInterval(() => setNowMs(Date.now()), 60_000);
    return () => window.clearInterval(interval);
  }, []);

  const fullTime = formatDateTime(timestamp, formatLocale);
  const passed = timestamp * 1000 <= nowMs;
  const [relative, absolute] = formatQuotaResetTime(timestamp, formatLocale, nowMs).split(" · ");
  return (
    <time
      aria-label={passed
        ? `${t("quota.resetTimePassed")}: ${fullTime}`
        : t("quota.resets", { time: fullTime })}
      className="quota-reset-time"
      data-full-time={passed ? `${fullTime} · ${t("quota.resetTimePassed")}` : t("quota.resets", { time: fullTime })}
      dateTime={new Date(timestamp * 1000).toISOString()}
      tabIndex={0}
    >
      {passed ? t("quota.resetTimePassed") : <><span>{relative}</span><span className="quota-reset-date"> · {absolute}</span></>}
    </time>
  );
}

function QuotaAlert({ accountName, failure, id, lastSuccess, formatLocale, t }: {
  accountName: string;
  failure?: QuotaRefreshFailureCode;
  id: string;
  lastSuccess?: number;
  formatLocale: string;
  t: Translator;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLSpanElement>(null);
  const detailsRef = useRef<HTMLSpanElement>(null);
  const [position, setPosition] = useState({ top: 26, left: -4 });

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      if (!rootRef.current || !detailsRef.current) return;
      const anchor = rootRef.current.getBoundingClientRect();
      const details = detailsRef.current.getBoundingClientRect();
      const below = anchor.bottom + 4;
      const above = anchor.top - details.height - 4;
      const top = below + details.height <= window.innerHeight - 8 ? below
        : above >= 8 ? above : Math.max(8, window.innerHeight - details.height - 8);
      const left = Math.max(8, Math.min(anchor.left - 4, window.innerWidth - details.width - 8));
      const next = { top: top - anchor.top, left: left - anchor.left };
      setPosition((current) => current.top === next.top && current.left === next.left ? current : next);
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, failure, lastSuccess, formatLocale, t]);

  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: Event) => {
      if (event.target instanceof Node && !rootRef.current?.contains(event.target)) setOpen(false);
    };
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    const close = () => setOpen(false);
    document.addEventListener("pointerdown", closeOutside);
    document.addEventListener("focusin", closeOutside);
    document.addEventListener("keydown", closeOnEscape);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
      document.removeEventListener("focusin", closeOutside);
      document.removeEventListener("keydown", closeOnEscape);
      window.removeEventListener("blur", close);
    };
  }, [open]);

  return (
    <span
      className="quota-alert"
      data-open={open}
      ref={rootRef}
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => {
        if (!rootRef.current?.contains(document.activeElement)) setOpen(false);
      }}
      onFocus={() => setOpen(true)}
      onBlur={(event) => {
        if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}
    >
      <button
        aria-controls={id}
        aria-describedby={id}
        aria-expanded={open}
        aria-label={t(failure ? "quota.refreshErrorFor" : "quota.pendingFor", { name: accountName })}
        className="quota-alert-button"
        onClick={() => setOpen(true)}
        type="button"
      ><CircleAlert size={15} /></button>
      <span className="quota-alert-tooltip" hidden={!open} id={id} ref={detailsRef} role="tooltip" style={position}>
        <strong>{t(failure ? "quota.refreshErrorLabel" : "quota.pending")}</strong>
        {failure ? <span>{quotaFailureMessage(t, failure)}</span> : null}
        {lastSuccess && lastSuccess > 0 && lastSuccess <= Date.now() ? (
          <span>{t("quota.lastUpdated", { time: formatDateTimeWithRelative(lastSuccess / 1000, formatLocale) })}</span>
        ) : null}
      </span>
    </span>
  );
}

function QuotaMeter({
  accountName,
  label,
  window,
  status,
  failure,
  failureId,
  lastSuccess,
  t,
  formatLocale,
}: {
  accountName: string;
  label: string;
  window?: QuotaWindow;
  status?: QuotaView["status"];
  failure?: QuotaRefreshFailureCode;
  failureId?: string;
  lastSuccess?: number;
  t: Translator;
  formatLocale: string;
}) {
  const known = status === "fresh" || status === "stale";
  const remaining = window?.remaining_percent;
  const value = typeof remaining === "number" ? Math.max(0, Math.min(100, remaining)) : undefined;

  return (
    <div className={"quota-meter" + (status === "stale" ? " quota-meter-stale" : "")}>
      <div className="quota-heading">
        <span className="quota-label">{label}{failureId && (failure || status === "stale") ? (
          <QuotaAlert accountName={accountName} failure={failure} id={failureId} lastSuccess={lastSuccess} formatLocale={formatLocale} t={t} />
        ) : null}</span>
        <strong>{value === undefined || !known ? "—" : status === "stale"
          ? t("quota.previousValue", { value: formatPercent(value, formatLocale) })
          : formatPercent(value, formatLocale)}</strong>
      </div>
      <div
        aria-label={t("quota.remaining", { label })}
        aria-valuemax={100}
        aria-valuemin={0}
        aria-valuenow={status === "fresh" ? value : undefined}
        className="quota-track"
        role={value === undefined || status !== "fresh" ? undefined : "progressbar"}
      >
        <span
          className={value !== undefined && value < 15 ? "quota-low" : ""}
          style={{ width: String(value ?? 0) + "%" }}
        />
      </div>
      {status !== "stale" || value === undefined ? <small>
        {!known || value === undefined
          ? t("quota.notAvailable")
          : window?.resets_at
              ? <QuotaResetTime timestamp={window.resets_at} formatLocale={formatLocale} t={t} />
              : t("quota.resetTimeUnavailable")}
      </small> : null}
    </div>
  );
}

function AccountCard({
  account,
  quota,
  active,
  busyAction,
  globalBusy,
  onSwitch,
  onWake,
  onRefresh,
  onReset,
  resetRecoveryRequired,
  onRemove,
  onCopyEmail,
  onReauthenticate,
  onApply,
  reauthenticationAvailable,
  quotaFailure,
  selectionMode,
  selected,
  onSelectionChange,
  t,
  formatLocale,
  now,
}: {
  account: AccountView;
  quota?: QuotaView;
  active: boolean;
  busyAction?: AccountBusyAction;
  globalBusy: boolean;
  onSwitch: () => void;
  onWake: () => void;
  onRefresh: () => void;
  onReset: () => void;
  resetRecoveryRequired: boolean;
  onRemove: () => void;
  onCopyEmail: () => void;
  onReauthenticate: () => void;
  onApply: () => void;
  reauthenticationAvailable: boolean;
  quotaFailure?: QuotaRefreshFailureCode;
  selectionMode: boolean;
  selected: boolean;
  onSelectionChange: () => void;
  t: Translator;
  formatLocale: string;
  now: number;
}) {
  const credits = quota?.snapshot?.reset_credits;
  const isApiKey = account.kind === "api_key";
  const primaryName = accountPrimaryName(account);
  const secondaryName = accountSecondaryName(account, primaryName, t);
  const controlsBusy = globalBusy || busyAction !== undefined;
  const signInRequired = account.sign_in_required || reauthenticationAvailable;

  return (
    <article className={"account-card" + (active ? " account-active" : "") + (selected ? " account-selected" : "")}>
      <div className="account-card-head">
        <div className="account-identity">
          {selectionMode ? (
            <label className="account-select-control">
              <input
                aria-label={t("accounts.selectAccount", { name: primaryName })}
                checked={selected}
                onChange={onSelectionChange}
                type="checkbox"
              />
            </label>
          ) : null}
          <div className="account-avatar" aria-hidden="true">
            {primaryName.slice(0, 1).toUpperCase()}
          </div>
          <div className="account-copy">
            <h3 title={primaryName}>{primaryName}</h3>
            {secondaryName ? <p title={secondaryName}>{secondaryName}</p> : null}
          </div>
        </div>
        {!selectionMode ? (
          <PopoverMenu
            className="card-menu"
            icon={<MoreHorizontal size={18} />}
            label={t("account.moreActions", { name: primaryName })}
          >
            {account.email ? <button onClick={onCopyEmail} type="button"><Copy size={15} />{t("account.copyEmail")}</button> : null}
            {!isApiKey ? <button disabled={controlsBusy} onClick={onReauthenticate} type="button"><Globe2 size={15} />{t("account.signInAgain")}</button> : null}
            <button
              className="card-menu-danger"
              aria-label={t("account.remove", { name: primaryName })}
              disabled={active || controlsBusy}
              onClick={onRemove}
              type="button"
            >
              <Trash2 size={15} />
              {t("common.removeAccount")}
            </button>
          </PopoverMenu>
        ) : null}
      </div>

      <div className="account-badges">
        {active ? <span className="badge badge-active"><Check size={13} /> {t("account.active")}</span> : null}
        <span className="badge">{accountPlan(account, t)}</span>
        {signInRequired ? <span className="badge badge-error">{t("account.signInRequired")}</span> : null}
        {!signInRequired && active && account.needs_apply ? <span className="badge badge-muted">{t("account.pendingApply")}</span> : null}
      </div>

      {isApiKey ? (
        <div className="api-key-state">
          <KeyRound size={18} />
          <div>
            <strong>{t("account.savedNoBillableCheck")}</strong>
            <p>{t("account.apiKeyDescription")}</p>
          </div>
        </div>
      ) : (
        <>
          <div className={"quota-pair" + (codexQuotaWindows(quota).length === 1 ? " quota-pair-single" : "")}>
            {codexQuotaWindows(quota).length ? codexQuotaWindows(quota).map((window, index) => (
              <QuotaMeter
                accountName={primaryName}
                failure={index === 0 ? quotaFailure : undefined}
                failureId={index === 0 ? "quota-error-" + account.id : undefined}
                lastSuccess={quota?.snapshot?.fetched_at_unix_ms}
                formatLocale={formatLocale}
                key={index}
                label={quotaWindowLabel(window, t)}
                status={quota?.status}
                t={t}
                window={window}
              />
            )) : <QuotaMeter accountName={primaryName} failure={quotaFailure} failureId={"quota-error-" + account.id} lastSuccess={quota?.snapshot?.fetched_at_unix_ms} formatLocale={formatLocale} label={t("quota.otherWindow")} status={quota?.status} t={t} />}
          </div>
          <div className="credit-row">
            <span className="credit-count">
              {t("credit.resetCredits")}
              <strong>{credits ? formatNumber(credits.available_count, formatLocale) : "—"}</strong>
            </span>
            {credits?.available_count && credits.available_count > 0 ? (
              <button className="text-button" onClick={onReset} type="button">
                {resetRecoveryRequired ? t("common.recoveryRequired") : t("common.details")}
                <ChevronRight size={15} />
              </button>
            ) : null}
            {credits?.nearest_expiry ? <small className="credit-expiry" tabIndex={0} data-full-time={formatDateTime(credits.nearest_expiry, formatLocale)} aria-label={t("credit.earliest", { time: formatDateTime(credits.nearest_expiry, formatLocale) })}>{formatCompactExpiry(credits.nearest_expiry, formatLocale)}</small> : null}
          </div>
        </>
      )}

      {!selectionMode ? <div className="card-footer">
        {!isApiKey ? <button
          aria-label={t("account.refresh", { name: primaryName })}
          className="icon-button"
          disabled={controlsBusy}
          onClick={onRefresh}
          type="button"
        >
          <RefreshCw className={busyAction === "refresh" ? "spin" : ""} size={17} />
        </button> : null}
        {!isApiKey && quota?.snapshot?.fetched_at_unix_ms ? (
          <small className="quota-updated">
            {now - quota.snapshot.fetched_at_unix_ms < 60_000
              ? t("quota.updatedJustNow")
              : t("quota.updatedAgo", { time: formatRelativeTime(quota.snapshot.fetched_at_unix_ms / 1000, formatLocale, now) })}
          </small>
        ) : null}
        <div className="card-footer-actions">
          {!isApiKey && !signInRequired ? (
            <button
              aria-label={t("account.wake", { name: primaryName })}
              className="button button-secondary"
              disabled={controlsBusy}
              onClick={onWake}
              type="button"
            >
              {busyAction === "wake" ? <LoaderCircle className="spin" size={15} /> : <Zap size={15} />}
              {t("common.wake")}
            </button>
          ) : null}
          <button
            aria-label={t(signInRequired ? "account.signInAgain" : active && account.needs_apply ? "account.apply" : active ? "account.current" : "account.switch", { name: primaryName })}
            className="button button-primary"
            disabled={controlsBusy || (active && !account.needs_apply && !signInRequired)}
            onClick={signInRequired ? onReauthenticate : active && account.needs_apply ? onApply : onSwitch}
            type="button"
          >
            {busyAction === "switch" ? <LoaderCircle className="spin" size={15} /> : signInRequired ? <Globe2 size={15} /> : <ArrowRightLeft size={15} />}
            {signInRequired ? t("account.signInAgain") : active && account.needs_apply ? t("account.apply") : active ? t("common.current") : t("common.switch")}
          </button>
        </div>
      </div> : null}
    </article>
  );
}

function FirstRun({ onImport, onAdd, t }: { onImport: () => void; onAdd: () => void; t: Translator }) {
  return (
    <section className="first-run">
      <div className="first-run-primary">
        <span className="eyebrow">{t("firstRun.eyebrow")}</span>
        <h2>{t("firstRun.title")}</h2>
        <p>
          {t("firstRun.body")}
        </p>
        <div className="first-run-actions">
          <button className="button button-primary" onClick={onImport} type="button">
            <Upload size={16} />
            {t("firstRun.importExport")}
          </button>
          <button className="button button-secondary" onClick={onAdd} type="button">
            <Plus size={16} />
            {t("firstRun.addManually")}
          </button>
        </div>
      </div>
      <div className="first-run-methods">
        <div><Globe2 size={18} /><span>{t("firstRun.browserSignIn")}</span></div>
        <div><FileJson size={18} /><span>{t("firstRun.authJson")}</span></div>
        <div><KeyRound size={18} /><span>{t("account.apiKey")}</span></div>
      </div>
    </section>
  );
}

function UpdateNotice({
  pending,
  phase,
  progress,
  onInstall,
  onLater,
  onRestart,
  t,
}: {
  pending: PendingUpdate;
  phase: UpdatePhase;
  progress: number | null;
  onInstall: () => void;
  onLater: () => void;
  onRestart: () => void;
  t: Translator;
}) {
  const downloading = phase === "downloading" || phase === "installing";
  const isReleaseDownload = pending.delivery === "release_download";
  const title = t(({
    available: "update.available",
    downloading: "update.downloadTitle",
    installing: "update.installTitle",
    ready: "update.readyTitle",
    error: "update.failedTitle",
  } as const)[phase], { version: pending.update.version });
  const primaryLabel = isReleaseDownload
    ? t("update.download")
    : phase === "ready"
      ? t("update.restart")
      : phase === "error"
        ? t("update.retry")
        : phase === "installing"
          ? t("update.installing")
          : downloading
            ? t("update.downloading", { progress: progress ?? "…" })
            : t("update.install");

  return (
    <section aria-label={title} className="update-notice">
      <div>
        <strong role="status">{title}</strong>
        {isReleaseDownload ? <p>{t("update.debianFallback")}</p>
          : phase === "downloading" ? <p>{t("update.downloading", { progress: progress ?? "…" })}</p>
          : phase === "ready" ? <p>{t("update.restartRequired")}</p>
          : phase === "error" ? <p>{t("update.failed")}</p>
          : null}
      </div>
      <div className="update-actions">
        {phase !== "ready" && !downloading ? (
          <button className="button button-quiet" onClick={onLater} type="button">{t("update.later")}</button>
        ) : null}
        <button
          className="button button-primary"
          disabled={downloading}
          onClick={phase === "ready" ? onRestart : onInstall}
          type="button"
        >
          {downloading ? <LoaderCircle className="spin" size={15} /> : <RefreshCw size={15} />}
          {primaryLabel}
        </button>
      </div>
    </section>
  );
}

export default function App() {
  const [languagePreference, setLanguagePreference] = useState<LanguagePreference>(() => readLanguagePreference());
  const [accounts, setAccounts] = useState<AccountView[]>([]);
  const [quotas, setQuotas] = useState<Record<string, QuotaView>>({});
  const [quotaFailures, setQuotaFailures] = useState<Record<string, QuotaRefreshFailureCode>>({});
  const [accountSignInNeeded, setAccountSignInNeeded] = useState<Record<string, boolean>>({});
  const [live, setLive] = useState<LiveAccountView | null>(null);
  const [storage, setStorage] = useState<StorageView | null>(null);
  const [pendingReset, setPendingReset] = useState<PendingResetView | null>(null);
  const pendingResetCredit = pendingReset !== null;
  const [discardConfirmation, setDiscardConfirmation] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [accountBusy, setAccountBusy] = useState<Record<string, AccountBusyAction>>({});
  const [dialog, setDialog] = useState<Dialog>(null);
  const [addMethod, setAddMethod] = useState<AddMethod>("start");
  const [selectionMode, setSelectionMode] = useState(false);
  const [selectedAccountIds, setSelectedAccountIds] = useState<string[]>([]);
  const confirm = useConfirmClick();
  const [migrationPreview, setMigrationPreview] = useState<MigrationPreview | null>(null);
  const [migrationRoot, setMigrationRoot] = useState<string | undefined>();
  const [migrationSelection, setMigrationSelection] = useState<string[]>([]);
  const [migrationScanned, setMigrationScanned] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [oauth, setOauth] = useState<OAuthFlow | null>(null);
  const [oauthStatusUnavailable, setOauthStatusUnavailable] = useState(false);
  const [oauthTarget, setOauthTarget] = useState<AccountView | null>(null);
  const [wake, setWake] = useState<WakeOperationView | null>(null);
  const [wakeStatusUnavailable, setWakeStatusUnavailable] = useState(false);
  const [resetAccount, setResetAccount] = useState<AccountView | null>(null);
  const [removeAccount, setRemoveAccount] = useState<AccountView | null>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);
  const [redeemingCredit, setRedeemingCredit] = useState<string | null>(null);
  const [pendingUpdate, setPendingUpdate] = useState<PendingUpdate | null>(null);
  const [cliInfo, setCliInfo] = useState<CodexCliInfo | null>(null);
  const [cliChecking, setCliChecking] = useState(false);
  const [cliResult, setCliResult] = useState<{ kind: "updated" | "sameVersion"; version: string } | null>(null);
  const [cliError, setCliError] = useState<CodexCliUpdateFailure | "inspect_failed" | "open_guide" | null>(null);
  const [updatePhase, setUpdatePhase] = useState<UpdatePhase>("available");
  const [updateProgress, setUpdateProgress] = useState<number | null>(null);
  const jsonRef = useRef<HTMLTextAreaElement>(null);
  const apiKeyRef = useRef<HTMLInputElement>(null);
  const dismissedUpdateVersions = useRef(new Set<string>());
  const updateCheckInFlight = useRef(false);
  const quotaRefreshes = useRef(new Map<string, QuotaRefreshEntry>());
  // Automatic reads go one at a time; a user's refresh gets its own lane of
  // up to three read-only reads; only a saved sign-in that needs the official
  // token refresh takes the credential lock, one account at a time.
  const refreshLanes = useRef(new RefreshLanes({ background: 1, manual: 3, signIn: 1 }));
  const lastQuotaAttempt = useRef(new Map<string, number>());
  const refreshInputs = useRef<{
    accounts: AccountView[];
    quotas: Record<string, QuotaView>;
    quotaFailures: Record<string, QuotaRefreshFailureCode>;
    activeId?: string;
  }>({ accounts: [], quotas: {}, quotaFailures: {} });
  const [clock, setClock] = useState(() => Date.now());
  const [refreshProgress, setRefreshProgress] = useState<{ done: number; total: number } | null>(null);
  const accountOperations = useRef(new Set<string>());
  const snapshotSequence = useRef(0);
  const initialSnapshotLoaded = useRef(false);
  const [label, setLabel] = useState("");
  const locale = useMemo(() => resolveLocale(languagePreference), [languagePreference]);
  const t = useMemo(() => createTranslator(locale.language), [locale.language]);

  useEffect(() => {
    if (!notice || notice.kind === "error") return;
    const timeout = window.setTimeout(() => {
      setNotice((current) => current === notice ? null : current);
    }, notice.kind === "success" ? 4000 : 6000);
    return () => window.clearTimeout(timeout);
  }, [notice]);

  const changeLanguage = (preference: LanguagePreference) => {
    setLanguagePreference(preference);
    saveLanguagePreference(preference);
  };

  const checkCodexCli = async () => {
    setCliChecking(true);
    setCliError(null);
    try {
      setCliInfo(await api.codexCliInfo());
    } catch {
      setCliInfo(null);
      setCliError("inspect_failed");
    } finally {
      setCliChecking(false);
    }
  };

  const openCodexCli = () => {
    setCliResult(null);
    setCliError(null);
    setDialog("codex-cli");
    void checkCodexCli();
  };

  useEffect(() => {
    if (loading) return;
    let closed = false;
    const check = () => {
      void api.codexCliInfo().then((info) => {
        if (!closed) setCliInfo(info);
      }).catch(() => {
        if (!closed) setCliInfo(null);
      });
    };
    const initial = window.setTimeout(check, 250);
    const interval = window.setInterval(check, 6 * 60 * 60 * 1000);
    return () => {
      closed = true;
      window.clearTimeout(initial);
      window.clearInterval(interval);
    };
  }, [loading]);

  const updateCodexCli = async () => {
    const previousVersion = cliInfo?.version;
    setCliResult(null);
    setCliError(null);
    setBusy("codex-cli-update");
    try {
      const updated = await api.updateCodexCli();
      setCliInfo(await api.codexCliInfo().catch(() => updated));
      setCliResult({ kind: previousVersion === updated.version ? "sameVersion" : "updated", version: updated.version || "—" });
    } catch (error) {
      setCliError(cliUpdateFailureCode(error));
    } finally {
      setBusy(null);
    }
  };

  const requestQuotaRefresh = useCallback((accountId: string, background = false): Promise<QuotaView> => {
    const current = quotaRefreshes.current.get(accountId);
    // A started read, or any read an automatic caller can share, is joined. A
    // user's refresh replaces an automatic read that is still waiting.
    if (current && (background || current.started || current.manual)) {
      return current.promise;
    }

    lastQuotaAttempt.current.set(accountId, Date.now());
    const lanes = refreshLanes.current;
    const entry: QuotaRefreshEntry = { manual: !background, started: false, promise: Promise.resolve() as never };
    const read = () => {
      entry.started = true;
      return api.refreshAccountQuota(accountId, true);
    };
    const attempt = background
      ? lanes.run("background", () => {
        const replacement = quotaRefreshes.current.get(accountId);
        return replacement && replacement !== entry ? replacement.promise : read();
      })
      : lanes.run("manual", read).catch((error: unknown) => {
        if (quotaFailureCode(error) !== "manual_refresh_needed") {
          throw error;
        }
        return lanes.run("signIn", () => api.refreshAccountQuota(accountId, false));
      });
    const request = attempt.then(
      (quota) => {
        setQuotas((current) => ({ ...current, [accountId]: quota }));
        setQuotaFailures((current) => {
          const next = { ...current };
          delete next[accountId];
          return next;
        });
        return quota;
      },
      (error) => {
        const code = quotaFailureCode(error);
        setQuotaFailures((current) => ({ ...current, [accountId]: code }));
        setQuotas((current) => {
          const previous = current[accountId];
          return {
            ...current,
            [accountId]: {
              account_id: accountId,
              status: previous?.snapshot ? "stale" : "unknown",
              snapshot: previous?.snapshot,
            },
          };
        });
        throw error;
      },
    );
    entry.promise = request;
    quotaRefreshes.current.set(accountId, entry);
    const forget = () => {
      if (quotaRefreshes.current.get(accountId) === entry) {
        quotaRefreshes.current.delete(accountId);
      }
    };
    void request.then(forget, forget);
    return request;
  }, []);

  const loadSnapshot = useCallback(async () => {
    const sequence = ++snapshotSequence.current;
    if (!initialSnapshotLoaded.current) setLoading(true);
    try {
      const initial = await api.appSnapshot();
      if (sequence !== snapshotSequence.current) return;
      const activeAccountId = initial.live?.account?.id;
      const nextAccounts = [...initial.accounts].sort(
        (left, right) =>
          Number(right.active || right.id === activeAccountId) -
          Number(left.active || left.id === activeAccountId),
      );
      setAccounts((current) => {
        if (!current.length) return nextAccounts;
        const byId = new Map(nextAccounts.map((account) => [account.id, account]));
        const retained = current.flatMap((account) => byId.has(account.id) ? [byId.get(account.id)!] : []);
        const added = nextAccounts.filter((account) => !current.some((saved) => saved.id === account.id));
        return [...retained, ...added];
      });
      setSelectedAccountIds((current) =>
        current.filter((id) => nextAccounts.some((account) => account.id === id)),
      );
      setStorage(initial.storage);
      setPendingReset(initial.pending_reset ?? null);
      setLive(initial.live || null);
      const savedIds = new Set(nextAccounts.map((account) => account.id));
      setQuotas((current) => Object.fromEntries(Object.entries(current).filter(([id]) => savedIds.has(id))));
      setQuotaFailures((current) => Object.fromEntries(Object.entries(current).filter(([id]) => savedIds.has(id))));
      if (initial.storage.status === "ready") {
        void Promise.allSettled(
          nextAccounts
            .filter((account) => account.kind === "chat_gpt")
            .map(async (account) => [account.id, await api.accountQuota(account.id)] as const),
        ).then((quotaPairs) => {
          if (sequence !== snapshotSequence.current) return;
          const cached = Object.fromEntries(
            quotaPairs.flatMap((result) => result.status === "fulfilled" ? [result.value] : []),
          ) as Record<string, QuotaView>;
          setQuotas((current) => mergeNewerQuotas(current, cached));
          const now = Date.now();
          for (const account of nextAccounts) {
            const quota = cached[account.id];
            if (quota && quotaRefreshDue(quota, {
              active: account.active || account.id === activeAccountId,
              focused: false,
              failure: refreshInputs.current.quotaFailures[account.id],
              lastAttemptAt: lastQuotaAttempt.current.get(account.id),
              now,
            })) {
              void requestQuotaRefresh(account.id, true).catch(() => undefined);
            }
          }
        });
      }
    } catch (error) {
      setNotice({
        kind: "error",
        text: friendlyError(t, error, t("error.snapshot")),
      });
    } finally {
      if (sequence === snapshotSequence.current) {
        initialSnapshotLoaded.current = true;
        setLoading(false);
      }
    }
  }, [requestQuotaRefresh, t]);

  useEffect(() => {
    void loadSnapshot();
  }, [loadSnapshot]);

  // While the window is visible, read each account when its quota can have
  // changed: every check every 30 seconds, and at once when the window
  // regains focus. A hidden window makes no reads.
  useEffect(() => {
    if (loading) return;
    const check = (focused: boolean) => {
      if (document.visibilityState === "hidden") return;
      const now = Date.now();
      setClock(now);
      const { accounts: saved, quotas: readings, quotaFailures: failures, activeId } = refreshInputs.current;
      for (const account of saved) {
        if (account.kind !== "chat_gpt") continue;
        if (quotaRefreshDue(readings[account.id], {
          active: account.active || account.id === activeId,
          focused,
          failure: failures[account.id],
          lastAttemptAt: lastQuotaAttempt.current.get(account.id),
          now,
        })) {
          void requestQuotaRefresh(account.id, true).catch(() => undefined);
        }
      }
    };
    const interval = window.setInterval(() => check(false), 30_000);
    const onFocus = () => check(true);
    const onVisibility = () => {
      if (document.visibilityState === "visible") check(true);
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.clearInterval(interval);
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [loading, requestQuotaRefresh]);

  const checkForUpdate = useCallback(async () => {
    if (!("__TAURI_INTERNALS__" in window) || updateCheckInFlight.current) {
      return;
    }
    updateCheckInFlight.current = true;
    try {
      const update = await updater.check();
      if (!update || dismissedUpdateVersions.current.has(update.version)) {
        return;
      }
      const delivery = await api.updateDelivery();
      setPendingUpdate({ update, delivery });
      setUpdatePhase("available");
      setUpdateProgress(null);
    } catch {
      // A failed background check is intentionally quiet. A user-initiated
      // install reports an actionable retry state instead.
    } finally {
      updateCheckInFlight.current = false;
    }
  }, []);

  useEffect(() => {
    void checkForUpdate();
    const interval = window.setInterval(() => void checkForUpdate(), 6 * 60 * 60 * 1000);
    return () => window.clearInterval(interval);
  }, [checkForUpdate]);

  const runTask = useCallback(
    async <T,>(key: string, task: () => Promise<T>, reload = true): Promise<T | undefined> => {
      setBusy(key);
      try {
        const result = await task();
        if (reload) {
          await loadSnapshot();
        }
        return result;
      } catch (error) {
        setNotice({ kind: "error", text: friendlyError(t, error) });
        return undefined;
      } finally {
        setBusy(null);
      }
    },
    [loadSnapshot, t],
  );

  const runVoidTask = useCallback(
    async (key: string, task: () => Promise<void>, reload = true): Promise<boolean> => {
      setBusy(key);
      try {
        await task();
        if (reload) {
          await loadSnapshot();
        }
        return true;
      } catch (error) {
        setNotice({ kind: "error", text: friendlyError(t, error) });
        return false;
      } finally {
        setBusy(null);
      }
    },
    [loadSnapshot, t],
  );

  const runAccountTask = useCallback(
    async <T,>(accountId: string, action: Exclude<AccountBusyAction, "switch">, task: () => Promise<T>): Promise<T | undefined> => {
      if (accountOperations.current.has(accountId)) {
        return undefined;
      }

      accountOperations.current.add(accountId);
      setAccountBusy((current) => ({ ...current, [accountId]: action }));
      try {
        return await task();
      } catch (error) {
        setNotice({ kind: "error", text: friendlyError(t, error) });
        return undefined;
      } finally {
        accountOperations.current.delete(accountId);
        setAccountBusy((current) => {
          const remaining = { ...current };
          delete remaining[accountId];
          return remaining;
        });
      }
    },
    [t],
  );

  const importPaths = useCallback(
    async (paths: string[]) => {
      const result = await runTask("import", () => api.importAuthFiles(paths));
      if (result) {
        const skippedCount = result.unsupported_count + result.failed_count;
        setNotice({
          kind: result.imported.length ? "success" : "info",
          text: t("notice.importBatch", {
            imported: formatNumber(result.imported.length, locale.formatLocale),
            duplicates: formatNumber(result.duplicate_count, locale.formatLocale),
            skipped: formatNumber(skippedCount, locale.formatLocale),
          }),
        });
        setDialog(null);
      }
    },
    [locale.formatLocale, runTask, t],
  );

  const chooseImportFile = useCallback(async () => {
    try {
      const selected = await open({
        title: t("add.fileTitle"),
        filters: fileFilters(t),
        multiple: true,
      });
      if (selected?.length) {
        await importPaths(selected);
      }
    } catch (error) {
      setNotice({ kind: "error", text: friendlyError(t, error, t("error.filePicker")) });
    }
  }, [importPaths, t]);

  const scanMigration = useCallback(
    async (customRoot?: string) => {
      const result = await runTask(
        "migration-scan",
        () => api.discoverLocalAccounts(customRoot),
        false,
      );
      if (result) {
        setMigrationRoot(customRoot);
        setMigrationPreview(result);
        setMigrationScanned(true);
        setMigrationSelection(
          result.candidates
            .filter((candidate) => candidate.state === "new")
            .map((candidate) => candidate.id),
        );
      }
    },
    [runTask],
  );

  const chooseMigrationFolder = useCallback(async () => {
    try {
      const selected = await open({
        title: t("migration.chooseFolder"),
        directory: true,
        multiple: false,
      });
      const selectedPath = Array.isArray(selected) ? selected[0] : selected;
      if (selectedPath) {
        await scanMigration(selectedPath);
      }
    } catch (error) {
      setNotice({ kind: "error", text: friendlyError(t, error, t("error.migrationFolder")) });
    }
  }, [scanMigration, t]);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) {
      return;
    }
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type !== "drop") {
          return;
        }
        const paths = event.payload.paths;
        if (paths.length) {
          void importPaths(paths);
        }
      })
      .then((stop) => {
        unlisten = stop;
      })
      .catch(() => {
        // File selection remains available if drag-and-drop cannot register.
      });
    return () => unlisten?.();
  }, [importPaths]);

  useEffect(() => {
    if (!oauth || (oauth.status.status !== "pending" && oauth.status.status !== "finishing")) {
      return;
    }
    let closed = false;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const status = await api.oauthStatus(oauth.login_id);
        if (closed) {
          return;
        }
        setOauthStatusUnavailable(false);
        setOauth((current) =>
          current?.login_id === oauth.login_id ? { ...current, status } : current,
        );
        if (status.status === "complete") {
          if (oauthTarget) {
            setAccountSignInNeeded((current) => ({ ...current, [oauthTarget.id]: false }));
          }
          void requestQuotaRefresh(status.account.id, true).catch(() => undefined);
          if (!status.account.needs_apply) {
            setNotice({ kind: "success", text: t(oauthTarget ? "notice.reauthenticatedAccount" : "notice.importedAccount", { name: accountPrimaryName(status.account) }) });
          }
          void loadSnapshot();
        } else if (status.status === "pending" || status.status === "finishing") {
          timer = window.setTimeout(poll, 800);
        }
      } catch {
        if (!closed) {
          setOauthStatusUnavailable(true);
          timer = window.setTimeout(poll, 2500);
        }
      }
    };
    timer = window.setTimeout(poll, 500);
    return () => {
      closed = true;
      if (timer) {
        window.clearTimeout(timer);
      }
    };
  }, [loadSnapshot, oauth, oauthTarget, requestQuotaRefresh, t]);

  useEffect(() => {
    if (!wake || wake.status !== "running") {
      return;
    }
    let closed = false;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const next = await api.wakeOperation(wake.id);
        if (closed) {
          return;
        }
        setWakeStatusUnavailable(false);
        setWake(next);
        if (next.status === "running") {
          timer = window.setTimeout(poll, 850);
        } else {
          void loadSnapshot();
          for (const result of next.results) {
            void requestQuotaRefresh(result.account_id, true).catch(() => undefined);
          }
        }
      } catch {
        if (!closed) {
          setWakeStatusUnavailable(true);
          timer = window.setTimeout(poll, 2500);
        }
      }
    };
    timer = window.setTimeout(poll, 500);
    return () => {
      closed = true;
      if (timer) {
        window.clearTimeout(timer);
      }
    };
  }, [loadSnapshot, requestQuotaRefresh, t, wake]);

  const startOAuth = async (target?: AccountView) => {
    const result = await runTask("oauth", () => api.startOAuth(target?.id), false);
    if (!result) {
      return;
    }
    setOauth({ ...result, status: { status: "pending" } });
    setOauthStatusUnavailable(false);
    setOauthTarget(target ?? null);
    setDialog("add");
    setAddMethod("oauth");
  };

  const dismissUpdate = () => {
    if (pendingUpdate) {
      dismissedUpdateVersions.current.add(pendingUpdate.update.version);
    }
    setPendingUpdate(null);
    setUpdatePhase("available");
    setUpdateProgress(null);
  };

  const installUpdate = async () => {
    if (!pendingUpdate) {
      return;
    }
    if (pendingUpdate.delivery === "release_download") {
      try {
        await api.openLatestRelease();
      } catch {
        setUpdatePhase("error");
      }
      return;
    }

    setUpdatePhase("downloading");
    setUpdateProgress(null);
    let expectedBytes: number | undefined;
    let downloadedBytes = 0;
    try {
      await pendingUpdate.update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          expectedBytes = event.data.contentLength;
        } else if (event.event === "Progress") {
          downloadedBytes += event.data.chunkLength;
          if (expectedBytes && expectedBytes > 0) {
            setUpdateProgress(Math.min(100, Math.round((downloadedBytes / expectedBytes) * 100)));
          }
        }
      });
      setUpdatePhase(pendingUpdate.delivery === "installer_exits" ? "installing" : "ready");
    } catch {
      setUpdatePhase("error");
    }
  };

  const restartAfterUpdate = async () => {
    try {
      await updater.relaunch();
    } catch {
      setUpdatePhase("error");
    }
  };

  const cancelOAuth = async () => {
    if (!oauth) {
      return;
    }
    await runVoidTask("oauth-cancel", () => api.cancelOAuth(oauth.login_id), false);
  };

  const retryOAuthSave = async () => {
    if (!oauth) return;
    const completed = await runVoidTask("oauth-retry", () => api.retryOAuth(oauth.login_id), false);
    if (completed) setOauth((current) => current ? { ...current, status: { status: "finishing" } } : current);
  };

  const closeAddDialog = () => {
    if (oauth?.status.status === "pending") {
      void api.cancelOAuth(oauth.login_id);
    }
    setDialog(null);
  };

  const openAddDialog = () => {
    setOauth(null);
    setOauthStatusUnavailable(false);
    setOauthTarget(null);
    setAddMethod("start");
    setMigrationPreview(null);
    setMigrationRoot(undefined);
    setMigrationSelection([]);
    setMigrationScanned(false);
    setDialog("add");
  };

  const confirmMigration = async () => {
    if (!migrationSelection.length) {
      return;
    }
    const result = await runTask(
      "migration-import",
      () => api.importLocalAccounts(migrationRoot, migrationSelection),
    );
    if (result) {
      const skippedCount = result.unsupported_count + result.failed_count;
      setNotice({
        kind: result.imported.length ? "success" : "info",
        text: t("notice.migration", {
          imported: formatNumber(result.imported.length, locale.formatLocale),
          duplicates: formatNumber(result.duplicate_count, locale.formatLocale),
          skipped: formatNumber(skippedCount, locale.formatLocale),
        }),
      });
      setMigrationPreview(null);
      setMigrationSelection([]);
      setMigrationRoot(undefined);
      setMigrationScanned(false);
      setDialog(null);
    }
  };

  const copyOAuthUrl = async () => {
    if (!oauth) {
      return;
    }
    try {
      await navigator.clipboard.writeText(oauth.auth_url);
      setNotice({ kind: "success", text: t("notice.oauthCopied") });
    } catch {
      setNotice({ kind: "info", text: t("notice.copyManually") });
    }
  };

  const copyAccountEmail = async (account: AccountView) => {
    if (!account.email) return;
    try {
      await navigator.clipboard.writeText(account.email);
      setNotice({ kind: "success", text: t("notice.emailCopied") });
    } catch {
      setNotice({ kind: "error", text: t("notice.emailCopyFailed") });
    }
  };

  const submitJson = async (event: FormEvent) => {
    event.preventDefault();
    const rawJson = jsonRef.current?.value ?? "";
    if (jsonRef.current) {
      jsonRef.current.value = "";
    }
    if (!rawJson.trim()) {
      setNotice({ kind: "error", text: t("notice.pasteAuth") });
      return;
    }
    const result = await runTask("import-json", () => api.importAuthJson(rawJson, label || undefined));
    if (result) {
      setLabel("");
      setDialog(null);
      setNotice({ kind: "success", text: t("notice.importedAccount", { name: accountPrimaryName(result) }) });
    }
  };

  const submitApiKey = async (event: FormEvent) => {
    event.preventDefault();
    const apiKey = apiKeyRef.current?.value ?? "";
    if (apiKeyRef.current) {
      apiKeyRef.current.value = "";
    }
    if (!apiKey.trim()) {
      setNotice({ kind: "error", text: t("notice.enterApiKey") });
      return;
    }
    const result = await runTask("import-key", () => api.importApiKey(apiKey, label || undefined));
    if (result) {
      setLabel("");
      setDialog(null);
      setNotice({ kind: "success", text: t("notice.savedApiKey", { name: result.label }) });
    }
  };

  // A full refresh shows progress on the toolbar and each card; it does not
  // lock the rest of the workspace.
  const refreshAll = async () => {
    const targets = accounts.filter((account) => account.kind === "chat_gpt");
    if (!targets.length || refreshProgress) {
      return;
    }
    setRefreshProgress({ done: 0, total: targets.length });
    const results = await Promise.allSettled(
      targets.map((account) =>
        requestQuotaRefresh(account.id).finally(() =>
          setRefreshProgress((current) => current && { ...current, done: current.done + 1 }),
        ),
      ),
    );
    setRefreshProgress(null);
    const failures = results.filter((result) => result.status === "rejected").length;
    if (failures) {
      setNotice({
        kind: "info",
        text: t("notice.quotaUnavailable", {
          count: formatNumber(failures, locale.formatLocale),
        }),
      });
    }
  };

  const switchAccount = async (account: AccountView) => {
    setBusy("switch:" + account.id);
    try {
      const outcome = await api.switchAccount(account.id);
      setAccountSignInNeeded((current) => ({ ...current, [account.id]: false }));
      setAccounts((current) => current.map((saved) =>
        saved.id === outcome.account.id
          ? { ...outcome.account, active: true }
          : { ...saved, active: false },
      ).sort((left, right) => Number(right.active) - Number(left.active)));
      setLive({ status: "ready", credential_store: "file", account: outcome.account });
      setNotice(null);
      void requestQuotaRefresh(outcome.account.id, true).catch(() => undefined);
    } catch (error) {
      if (asSwitchFailure(error)?.code === "account_needs_sign_in") {
        setAccountSignInNeeded((current) => ({ ...current, [account.id]: true }));
      }
      setNotice({ kind: "error", text: switchFailureMessage(t, error, account) });
    } finally {
      setBusy(null);
    }
  };

  // The card itself shows the refreshed quota; only failures need a notice.
  const refreshAccount = async (account: AccountView) => {
    await runAccountTask(account.id, "refresh", () => requestQuotaRefresh(account.id));
  };

  const startWake = async (accountId?: string, alternateModel = false) => {
    const result = accountId
      ? await runAccountTask(accountId, "wake", () => api.startWake(accountId, alternateModel))
      : await runTask("wake-all", api.startWakeAll, false);
    if (result) {
      setWake({ id: result.operation_id, status: "running", alternate_model: alternateModel, results: [] });
      setWakeStatusUnavailable(false);
      setDialog("wake");
    }
  };

  const startSelectedWake = async () => {
    const selectedChatGptIds = accounts
      .filter((account) => account.kind === "chat_gpt" && selectedAccountIds.includes(account.id))
      .map((account) => account.id);
    const result = await runTask(
      "wake-selected",
      () => api.startWakeSelected(selectedChatGptIds),
      false,
    );
    if (result) {
      setWake({ id: result.operation_id, status: "running", alternate_model: false, results: [] });
      setWakeStatusUnavailable(false);
      setDialog("wake");
    }
  };

  const exportSelectedAccounts = async () => {
    const result = await runTask(
      "export-accounts",
      () => api.exportAccounts(selectedAccountIds),
      false,
    );
    if (result) {
      confirm.disarm();
      setDialog(null);
      if (!result.cancelled) {
        setNotice({
          kind: "success",
          text: t("notice.exported", {
            count: formatNumber(result.exported_count, locale.formatLocale),
          }),
        });
      }
    }
  };

  // Rust returns the quota it read after a confirmed reset; show it at once
  // instead of waiting for the card's next refresh.
  const showResetQuota = (quota: QuotaView | undefined) => {
    if (!quota) {
      return;
    }
    setQuotas((current) => ({ ...current, [quota.account_id]: quota }));
    setQuotaFailures((current) => {
      const next = { ...current };
      delete next[quota.account_id];
      return next;
    });
  };

  // Each outcome says whether a credit was used, and which one.
  const resetOutcomeNotice = (result: ResetCreditOutcome): Notice => {
    const account = accounts.find((saved) => saved.id === result.account_id);
    const name = account ? accountPrimaryName(account) : t("recovery.removedAccount");
    switch (result.outcome) {
      case "reset":
        if (result.refresh_warning) {
          return { kind: "success", text: t("notice.resetRefreshWarning") };
        }
        return {
          kind: "success",
          text: result.used_expires_at
            ? t("notice.resetUsed", { name, date: formatDateTime(result.used_expires_at, locale.formatLocale) })
            : t("notice.resetUsedUndated", { name }),
        };
      case "already_redeemed":
        return {
          kind: "success",
          text: result.refresh_warning ? t("notice.resetRefreshWarning") : t("notice.resetAlreadyApplied"),
        };
      case "nothing_to_reset":
        return { kind: "info", text: t("notice.resetNotNeeded", { name }) };
      case "no_credit":
        return { kind: "info", text: t("notice.resetNoCredit", { name }) };
    }
  };

  // Redeems the credit on the row the user confirmed. Rust matches it again
  // against a fresh provider read before consuming anything.
  const redeemReset = async (credit: ResetCreditDetail, pick: string) => {
    if (!resetAccount) {
      return;
    }
    setRedeemingCredit(pick);
    const result = await runTask(
      "reset:" + resetAccount.id,
      () => api.redeemResetCredit(resetAccount.id, credit),
    );
    setRedeemingCredit(null);
    if (!result) {
      // A failed request may have left a pending record that needs recovery.
      void loadSnapshot();
      return;
    }
    showResetQuota(result.quota);
    setNotice(resetOutcomeNotice(result));
    setResetAccount(null);
    setDialog(null);
  };

  const recoverPendingResetCredit = async () => {
    const result = await runTask("recover-reset-credit", api.recoverPendingResetCredit);
    if (!result) {
      void loadSnapshot();
      return;
    }
    showResetQuota(result.quota);
    setNotice(resetOutcomeNotice(result));
    setDialog(null);
  };

  // Rust offers a discard only when retrying can no longer consume the
  // recorded credit; the second click confirms.
  const discardPendingReset = async () => {
    if (!discardConfirmation) {
      setDiscardConfirmation(true);
      return;
    }
    const completed = await runVoidTask("discard-reset-credit", api.discardPendingResetCredit);
    setDiscardConfirmation(false);
    if (completed) {
      setDialog(null);
      setNotice({ kind: "success", text: t("notice.resetDiscarded") });
    }
  };

  const removeSavedAccount = async () => {
    if (!removeAccount) {
      return;
    }
    setRemoveError(null);
    setBusy("remove:" + removeAccount.id);
    try {
      await api.removeSavedAccount(removeAccount.id);
      await loadSnapshot();
      setNotice({ kind: "success", text: t("notice.removed", { name: accountPrimaryName(removeAccount) }) });
      setRemoveAccount(null);
      setDialog(null);
    } catch (error) {
      setRemoveError(removeFailureMessage(t, error));
    } finally {
      setBusy(null);
    }
  };

  const resetDamagedAccountStore = async () => {
    const completed = await runVoidTask(
      "reset-damaged-store",
      api.resetDamagedAccountStore,
    );
    if (completed) {
      confirm.disarm();
      setDialog(null);
      setNotice({
        kind: "success",
        text: t("notice.storageReset"),
      });
    }
  };

  const cliUpdateAvailable = cliInfo?.update_status === "available" && cliInfo.supports_update;
  const currentActiveId = live?.account?.id;
  refreshInputs.current = { accounts, quotas, quotaFailures, activeId: currentActiveId };
  const chatGptAccounts = accounts.filter((account) => account.kind === "chat_gpt");
  const selectedChatGptCount = accounts.filter(
    (account) => account.kind === "chat_gpt" && selectedAccountIds.includes(account.id),
  ).length;
  const resetCredits = resetAccount ? quotas[resetAccount.id]?.snapshot?.reset_credits : undefined;
  const resetDialogName = (account: AccountView) => {
    const name = accountPrimaryName(account);
    const sameEmail = account.email
      ? accounts.filter((saved) => saved.email === account.email).length
      : 1;
    return sameEmail > 1 && account.workspace_name ? `${name} · ${account.workspace_name}` : name;
  };
  const pendingResetAccount = pendingReset
    ? accounts.find((account) => account.id === pendingReset.account_id)
    : undefined;
  const storageRecovery = storage?.status === "recovery_required";
  const accountGridBusy = accountGridHasGlobalMutation(busy);
  // A dialog stays closable while unrelated work runs; only its own operation
  // holds it open. A key ending in ":" matches that operation for any account.
  const runningAny = (...keys: string[]) =>
    busy !== null && keys.some((key) => (key.endsWith(":") ? busy.startsWith(key) : busy === key));

  const safetyNotice = useMemo(() => {
    if (storage?.status === "recovery_required") {
      return {
        icon: <ShieldAlert size={20} />,
        title: t("safety.storageTitle"),
        body: t("safety.storageBody"),
        action: t("safety.reviewRecovery"),
        onAction: () => {
          confirm.disarm();
          setDialog("storage-recovery");
        },
      };
    }
    if (pendingResetCredit) {
      return {
        icon: <CircleAlert size={20} />,
        title: t("safety.resetTitle"),
        body: t("safety.resetBody"),
        action: t("safety.reviewReset"),
        onAction: () => setDialog("recover-reset-credit"),
      };
    }
    if (!live) {
      return null;
    }
    if (live.status === "unknown_account") {
      return {
        icon: <ShieldAlert size={20} />,
        title: t("safety.currentTitle"),
        body: t("safety.currentBody"),
        action: t("safety.saveCurrent"),
        onAction: () =>
          void runTask("save-current", api.saveCurrentAccount).then((account) => {
            if (account) {
              setNotice({ kind: "success", text: t("notice.savedCurrent", { name: accountPrimaryName(account) }) });
            }
          }),
      };
    }
    if (live.status === "file_store_required") {
      return {
        icon: <ShieldAlert size={20} />,
        title: t("safety.fileTitle"),
        body: t("safety.fileBody"),
        action: t("safety.enable"),
        onAction: () => setDialog("enable-switching"),
      };
    }
    if (live.status === "recovery_required") {
      return {
        icon: <CircleAlert size={20} />,
        title: t("safety.switchTitle"),
        body: t("safety.switchBody"),
        action: t("safety.reviewRecovery"),
        onAction: () => setDialog("recover-switch"),
      };
    }
    return null;
  }, [live, pendingResetCredit, runTask, storage, t]);

  return (
    <main className="app-shell">
      <header className="toolbar">
        <div className="brand-lockup">
          <img alt="" aria-hidden="true" className="brand-mark" src="/gswitch-icon.svg" />
          <h1>GSwitch</h1>
        </div>
        <div className="toolbar-actions">
          <button className="button button-quiet" disabled={loading || busy !== null || refreshProgress !== null || storageRecovery} onClick={() => void refreshAll()} type="button">
            <RefreshCw className={refreshProgress ? "spin" : ""} size={16} />
            {refreshProgress
              ? t("toolbar.refreshing", {
                done: formatNumber(refreshProgress.done, locale.formatLocale),
                total: formatNumber(refreshProgress.total, locale.formatLocale),
              })
              : t("common.refresh")}
          </button>
          <button
            className="button button-secondary"
            disabled={busy !== null || loading || storageRecovery || chatGptAccounts.length === 0 || wake?.status === "running"}
            onClick={() => void startWake()}
            type="button"
          >
            <Zap size={16} />
            {t("toolbar.wakeAll")}
          </button>
          <button
            className="button button-primary"
            disabled={loading || busy !== null || storageRecovery}
            onClick={openAddDialog}
            type="button"
          >
            <Plus size={16} />
            {t("common.addAccount")}
          </button>
          {cliInfo?.update_status === "available" ? (
            <button className="button button-secondary cli-toolbar-button" disabled={busy !== null} onClick={openCodexCli} type="button">
              <Terminal size={16} />
              {t("cli.available", { version: cliInfo.latest_version || "" })}
            </button>
          ) : null}
          <PopoverMenu
            className="language-menu"
            icon={<span aria-hidden="true" className="language-menu-icon" />}
            label={t("toolbar.language")}
            popoverLabel={t("settings.language")}
            title={t("toolbar.language")}
          >
            {(["system", "zh-CN", "en"] as const).map((preference) => (
              <button
                aria-pressed={languagePreference === preference}
                key={preference}
                onClick={() => changeLanguage(preference)}
                type="button"
              >
                <span>{t(preference === "system" ? "settings.system" : preference === "en" ? "settings.english" : "settings.chinese")}</span>
                {languagePreference === preference ? <Check size={15} /> : null}
              </button>
            ))}
          </PopoverMenu>
        </div>
      </header>

      <div className="content">
        {pendingUpdate ? (
          <UpdateNotice
            onInstall={() => void installUpdate()}
            onLater={dismissUpdate}
            onRestart={() => void restartAfterUpdate()}
            pending={pendingUpdate}
            phase={updatePhase}
            progress={updateProgress}
            t={t}
          />
        ) : null}

        {notice ? (
          <div className={"toast toast-" + notice.kind} role={notice.kind === "error" ? "alert" : "status"}>
            {notice.kind === "success" ? <CircleCheck size={17} /> : <CircleAlert size={17} />}
            <span>{notice.text}</span>
            <button aria-label={t("common.dismissMessage")} onClick={() => setNotice(null)} type="button"><X size={15} /></button>
          </div>
        ) : null}

        {safetyNotice ? (
          <section className="safety-notice">
            <div className="safety-icon">{safetyNotice.icon}</div>
            <div>
              <h2>{safetyNotice.title}</h2>
              <p>{safetyNotice.body}</p>
            </div>
            <button className="button button-secondary" disabled={busy !== null} onClick={safetyNotice.onAction} type="button">
              {safetyNotice.action}
            </button>
          </section>
        ) : null}

        {!safetyNotice && live?.status === "not_signed_in" && accounts.length > 0 ? (
          <p className="signed-out-notice" role="status">{t("safety.signedOut")}</p>
        ) : null}

        <section className="accounts-section" aria-labelledby="accounts-heading">
          <div className="section-heading">
            <div className="section-heading-main">
              <h2 id="accounts-heading">
                {loading
                  ? t("accounts.loading")
                  : accounts.length === 1
                    ? t("accounts.savedOne")
                    : t("accounts.savedMany", { count: formatNumber(accounts.length, locale.formatLocale) })}
              </h2>
              {accounts.length && !storageRecovery ? (
                <button
                  aria-pressed={selectionMode}
                  className="button button-secondary section-select-button"
                  disabled={loading || busy !== null}
                  onClick={() => {
                    setSelectionMode((current) => !current);
                    setSelectedAccountIds([]);
                  }}
                  type="button"
                >
                  <ListChecks aria-hidden="true" size={15} />
                  {selectionMode ? t("accounts.doneSelecting") : t("accounts.select")}
                </button>
              ) : null}
            </div>
            {storageRecovery ? (
              <div className="section-heading-actions">
                <p>{t("accounts.recoveryDescription")}</p>
              </div>
            ) : null}
          </div>

          {selectionMode && accounts.length ? (
            <div className="account-selection-bar" aria-label={t("accounts.selectionActions")}>
              <span>{t("accounts.selected", { count: formatNumber(selectedAccountIds.length, locale.formatLocale) })}</span>
              <div className="account-selection-controls">
                <button
                  className="text-button"
                  disabled={busy !== null || selectedAccountIds.length === accounts.length}
                  onClick={() => setSelectedAccountIds(accounts.map((account) => account.id))}
                  type="button"
                >
                  {t("accounts.selectAll")}
                </button>
                <button
                  className="text-button"
                  disabled={busy !== null || selectedAccountIds.length === 0}
                  onClick={() => setSelectedAccountIds([])}
                  type="button"
                >
                  {t("accounts.clearSelection")}
                </button>
                <button
                  className="button button-secondary"
                  disabled={busy !== null || selectedChatGptCount === 0}
                  onClick={() => void startSelectedWake()}
                  type="button"
                >
                  <Zap size={15} />
                  {t("common.wake")}
                </button>
                <button
                  className="button button-primary"
                  disabled={busy !== null || selectedAccountIds.length === 0}
                  onClick={() => {
                    confirm.disarm();
                    setDialog("export");
                  }}
                  type="button"
                >
                  <Download size={15} />
                  {t("common.export")}
                </button>
              </div>
            </div>
          ) : null}

          {loading ? (
            <div className="loading-state"><LoaderCircle className="spin" size={24} />{t("accounts.reading")}</div>
          ) : storageRecovery ? (
            <section className="storage-recovery-state" aria-labelledby="storage-recovery-title">
              <ShieldAlert size={28} />
              <div>
                <h3 id="storage-recovery-title">{t("accounts.protectedTitle")}</h3>
                <p>{t("accounts.protectedBody")}</p>
              </div>
              <button
                className="button button-primary"
                onClick={() => {
                  confirm.disarm();
                  setDialog("storage-recovery");
                }}
                type="button"
              >
                {t("safety.reviewRecovery")}
              </button>
            </section>
          ) : accounts.length ? (
            <div className="account-grid">
              {accounts.map((account) => (
                <AccountCard
                  account={account}
                  active={account.active || account.id === currentActiveId}
                  busyAction={accountBusy[account.id] ?? (busy === "switch:" + account.id ? "switch" : undefined)}
                  formatLocale={locale.formatLocale}
                  now={clock}
                  globalBusy={accountGridBusy}
                  key={account.id}
                  onCopyEmail={() => void copyAccountEmail(account)}
                  onReauthenticate={() => void startOAuth(account)}
                  onApply={() => void switchAccount(account)}
                  onRefresh={() => void refreshAccount(account)}
                  onRemove={() => {
                    setRemoveAccount(account);
                    setRemoveError(null);
                    setDialog("remove");
                  }}
                  onReset={() => {
                    if (pendingResetCredit) {
                      setDialog("recover-reset-credit");
                      return;
                    }
                    setResetAccount(account);
                    confirm.disarm();
                    setDialog("reset");
                  }}
                  resetRecoveryRequired={pendingResetCredit}
                  onSwitch={() => void switchAccount(account)}
                  onWake={() => void startWake(account.id)}
                  onSelectionChange={() => {
                    setSelectedAccountIds((current) =>
                      current.includes(account.id)
                        ? current.filter((id) => id !== account.id)
                        : [...current, account.id],
                    );
                  }}
                  quota={quotas[account.id]}
                  quotaFailure={quotaFailures[account.id]}
                  reauthenticationAvailable={account.kind === "chat_gpt" && (account.sign_in_required === true || quotaFailures[account.id] === "authentication" || accountSignInNeeded[account.id] === true)}
                  selected={selectedAccountIds.includes(account.id)}
                  selectionMode={selectionMode}
                  t={t}
                />
              ))}
            </div>
          ) : (
            <FirstRun
              onAdd={openAddDialog}
              onImport={() => void chooseImportFile()}
              t={t}
            />
          )}
        </section>
      </div>

      {dialog === "codex-cli" ? (
        <Modal dismissible={busy !== "codex-cli-update"} onClose={() => setDialog(null)} t={t} title={t("toolbar.codexCli")}>
          <div className="cli-panel">
            {cliChecking ? <p className="cli-status" role="status"><LoaderCircle className="spin" size={18} />{t("cli.checking")}</p> : (
              <p className="cli-status">
                <Terminal size={18} />
                {!cliInfo?.version
                  ? t("cli.missing")
                  : cliUpdateAvailable
                    ? t("cli.updateAvailable", { version: cliInfo.version, latest: cliInfo.latest_version || "" })
                    : cliInfo.latest_version === cliInfo.version
                      ? t("cli.upToDate", { version: cliInfo.version })
                      : t("cli.version", { version: cliInfo.version })}
              </p>
            )}
            {cliInfo?.version && !cliInfo.supports_update ? <p>{t("cli.unsupported")}</p> : null}
            {cliUpdateAvailable ? <p>{busy === "codex-cli-update" ? t("cli.updatingNote") : t("cli.updateNote")}</p> : null}
            {cliResult ? <p className="cli-result" role="status">{t(cliResult.kind === "updated" ? "cli.updated" : "cli.sameVersion")}</p> : null}
            {cliError ? <p className="cli-error" role="alert">{t(({
              not_installed: "cli.error.notInstalled",
              unsupported: "cli.error.unsupported",
              busy: "cli.error.busy",
              codex_open: "cli.error.codexOpen",
              update_failed: "cli.error.updateFailed",
              verification_failed: "cli.error.verificationFailed",
              inspect_failed: "cli.error.inspectFailed",
              open_guide: "cli.error.openGuide",
            } as const)[cliError])}</p> : null}
            <div className="modal-actions">
              <button className="button button-quiet" disabled={busy !== null} onClick={() => void api.openCodexCliGuide().catch(() => setCliError("open_guide"))} type="button">{t("cli.guide")}</button>
              <button className="button button-secondary" disabled={busy !== null || cliChecking} onClick={() => void checkCodexCli()} type="button">{t("cli.checkAgain")}</button>
              {cliUpdateAvailable ? (
                <button className="button button-primary" disabled={busy !== null || cliChecking} onClick={() => void updateCodexCli()} type="button">
                  {busy === "codex-cli-update" ? <LoaderCircle className="spin" size={15} /> : <Download size={15} />}
                  {busy === "codex-cli-update" ? t("cli.updating") : t("cli.update")}
                </button>
              ) : null}
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "add" ? (
        <Modal dismissible={!runningAny("import", "migration-scan", "migration-import", "oauth", "oauth-cancel", "oauth-retry", "import-json", "import-key")} onClose={closeAddDialog} t={t} title={t(oauthTarget ? "oauth.reauthenticateTitle" : "add.title", oauthTarget ? { name: accountPrimaryName(oauthTarget) } : undefined)} wide>
          {addMethod === "start" ? (
            <div className="add-methods">
              <section className="add-method-group" aria-labelledby="import-existing-heading">
                <h3 id="import-existing-heading">{t("add.importExisting")}</h3>
                <button
                  className="add-method-card"
                  onClick={() => {
                    setMigrationPreview(null);
                    setMigrationRoot(undefined);
                    setMigrationSelection([]);
                    setMigrationScanned(false);
                    setAddMethod("migration");
                  }}
                  type="button"
                >
                  <span className="method-icon"><Upload size={22} /></span>
                  <span><strong>{t("add.localTitle")}</strong><small>{t("add.localDescription")}</small></span>
                  <ChevronRight size={18} />
                </button>
                <button className="add-method-card" onClick={() => void chooseImportFile()} type="button">
                  <span className="method-icon"><FolderOpen size={22} /></span>
                  <span><strong>{t("add.fileTitle")}</strong><small>{t("add.fileDescription")}</small></span>
                  <ChevronRight size={18} />
                </button>
              </section>
              <section className="add-method-group" aria-labelledby="add-new-heading">
                <h3 id="add-new-heading">{t("add.addNew")}</h3>
                <button className="add-method-card" onClick={() => void startOAuth()} type="button">
                  <span className="method-icon"><Globe2 size={22} /></span>
                  <span><strong>{t("add.browserTitle")}</strong><small>{t("add.browserDescription")}</small></span>
                  <ChevronRight size={18} />
                </button>
              </section>
              <details className="add-other-methods">
                <summary>{t("add.otherMethods")}</summary>
                <div>
                  <button className="add-method-card" onClick={() => setAddMethod("json")} type="button">
                    <span className="method-icon"><FileJson size={22} /></span>
                    <span><strong>{t("add.jsonTitle")}</strong><small>{t("add.jsonDescription")}</small></span>
                    <ChevronRight size={18} />
                  </button>
                  <button className="add-method-card" onClick={() => setAddMethod("api-key")} type="button">
                    <span className="method-icon"><KeyRound size={22} /></span>
                    <span><strong>{t("add.apiKeyTitle")}</strong><small>{t("add.apiKeyDescription")}</small></span>
                    <ChevronRight size={18} />
                  </button>
                </div>
              </details>
            </div>
          ) : null}

          {addMethod === "migration" ? (
            <div className="migration-flow">
              <button className="back-link" disabled={busy !== null} onClick={() => setAddMethod("start")} type="button">{t("migration.back")}</button>
              <h3>{t("migration.title")}</h3>
              <p>{t("migration.body")}</p>
              {!migrationScanned ? (
                <div className="modal-actions migration-actions">
                  <button className="button button-secondary" disabled={busy !== null} onClick={() => void chooseMigrationFolder()} type="button">
                    <FolderOpen size={16} />
                    {t("migration.chooseFolder")}
                  </button>
                  <button className="button button-primary" disabled={busy !== null} onClick={() => void scanMigration()} type="button">
                    {busy === "migration-scan" ? <LoaderCircle className="spin" size={16} /> : <Upload size={16} />}
                    {t("migration.scan")}
                  </button>
                </div>
              ) : (
                <>
                  {migrationPreview?.candidates.length ? (
                    <div className="migration-list" aria-label={t("migration.title")}>
                      {migrationPreview.candidates.map((candidate) => {
                        const disabled = candidate.state !== "new";
                        const checked = migrationSelection.includes(candidate.id);
                        const primary = candidate.email || candidate.workspace_name || t("common.unknown");
                        return (
                          <label className={`migration-item migration-${candidate.state}`} key={candidate.id}>
                            <input
                              checked={checked}
                              disabled={disabled || busy !== null}
                              onChange={() => {
                                setMigrationSelection((current) =>
                                  checked
                                    ? current.filter((id) => id !== candidate.id)
                                    : [...current, candidate.id],
                                );
                              }}
                              type="checkbox"
                            />
                            <span className="migration-item-copy">
                              <strong>{primary}</strong>
                              <small>
                                {migrationSourceLabel(candidate, t)} · {migrationStateLabel(candidate, t)}
                                {candidate.workspace_name && candidate.workspace_name !== primary
                                  ? ` · ${t("migration.workspace", { name: candidate.workspace_name })}`
                                  : ""}
                                {candidate.plan_type ? ` · ${t("migration.plan", { plan: planLabel(candidate.plan_type, t) })}` : ""}
                              </small>
                            </span>
                          </label>
                        );
                      })}
                    </div>
                  ) : (
                    <p className="dialog-footnote">{t("migration.empty")}</p>
                  )}
                  {!migrationSelection.length && migrationPreview?.candidates.some((candidate) => candidate.state === "new") ? (
                    <p className="dialog-footnote">{t("migration.noSelection")}</p>
                  ) : null}
                  <div className="modal-actions migration-actions">
                    <button className="button button-secondary" disabled={busy !== null} onClick={() => void chooseMigrationFolder()} type="button">
                      <FolderOpen size={16} />
                      {t("migration.chooseFolder")}
                    </button>
                    <button className="button button-quiet" disabled={busy !== null} onClick={() => void scanMigration(migrationRoot)} type="button">
                      <RefreshCw size={16} />
                      {t("migration.rescan")}
                    </button>
                    <button className="button button-primary" disabled={!migrationSelection.length || busy !== null} onClick={() => void confirmMigration()} type="button">
                      {busy === "migration-import" ? <LoaderCircle className="spin" size={16} /> : <Check size={16} />}
                      {t("migration.import")}
                    </button>
                  </div>
                </>
              )}
            </div>
          ) : null}

          {addMethod === "oauth" && oauth ? (
            <div className="oauth-flow">
              {oauthTarget ? <div className="oauth-target">
                <span>{t("oauth.targetEmail", { email: oauthTarget.email || accountPrimaryName(oauthTarget) })}</span>
                {oauthTarget.workspace_name ? <span>{t("oauth.targetWorkspace", { workspace: oauthTarget.workspace_name })}</span> : null}
                {oauthTarget.email ? <button className="text-button" onClick={() => void copyAccountEmail(oauthTarget)} type="button">{t("account.copyEmail")}</button> : null}
              </div> : null}
              {oauth.status.status === "pending" ? (
                <>
                  <div className="oauth-hero">
                    <LoaderCircle className="spin" size={26} />
                    <div><h3>{t("oauth.finishTitle")}</h3><p>{t(oauthTarget ? "oauth.reauthenticateBody" : "oauth.finishBody")}</p></div>
                  </div>
                  {oauthStatusUnavailable ? <p className="inline-warning" role="status">{t("error.oauthStatus")}</p> : null}
                  <label className="field-label" htmlFor="oauth-link">{t("oauth.link")}</label>
                  <div className="copy-field">
                    <input id="oauth-link" readOnly value={oauth.auth_url} />
                    <button aria-label={t("oauth.copyLink")} className="icon-button" onClick={() => void copyOAuthUrl()} type="button"><Copy size={17} /></button>
                  </div>
                  <div className="modal-actions">
                    <button className="button button-secondary" onClick={() => void cancelOAuth()} type="button">{t("oauth.cancel")}</button>
                    <button className="button button-primary" onClick={() => void api.openOAuth(oauth.login_id).catch(() => setNotice({ kind: "error", text: t("oauth.openFailed") }))} type="button"><Globe2 size={16} />{t("oauth.openBrowser")}</button>
                  </div>
                </>
              ) : oauth.status.status === "finishing" ? (
                <div className="oauth-hero" role="status">
                  <LoaderCircle className="spin" size={26} />
                  <div><h3>{t("oauth.finishing")}</h3><p>{t("oauth.finishingBody")}</p></div>
                </div>
              ) : oauth.status.status === "complete" ? (
                <div className="outcome-panel">
                  <CircleCheck size={26} />
                  <h3>{t(oauthTarget ? "oauth.reauthenticated" : "oauth.added", { name: accountPrimaryName(oauth.status.account) })}</h3>
                  {oauth.status.account.needs_apply ? <p>{t(oauth.status.account.active ? "oauth.pendingApplyCurrent" : "oauth.pendingApplyOther")}</p> : null}
                  {oauth.status.cleanup_warning ? <p>{t("oauth.cleanupWarning")}</p> : null}
                  <button className="button button-primary" onClick={closeAddDialog} type="button">{t("common.done")}</button>
                </div>
              ) : (
                <div className="outcome-panel">
                  <CircleAlert size={26} />
                  <h3>{oauth.status.status === "cancelled" ? t("oauth.cancelled") : t("oauth.incomplete")}</h3>
                  <p>{oauth.status.status === "failed" ? oauthFailureMessage(t, oauth.status.code) : t("oauth.unchanged")}</p>
                  {oauth.status.status === "failed" && oauth.status.retryable ? (
                    <button className="button button-primary" onClick={() => void retryOAuthSave()} type="button">{t("oauth.retrySave")}</button>
                  ) : <button className="button button-primary" onClick={() => void startOAuth(oauthTarget ?? undefined)} type="button">{t("oauth.retry")}</button>}
                </div>
              )}
            </div>
          ) : null}

          {addMethod === "json" ? (
            <form className="credential-form" onSubmit={submitJson}>
              <button className="back-link" disabled={busy !== null} onClick={() => setAddMethod("start")} type="button">{t("form.allMethods")}</button>
              <h3>{t("form.pasteTitle")}</h3>
              <p>{t("form.pasteBody")}</p>
              <label className="field-label" htmlFor="account-label">{t("form.displayName")} <span>{t("common.optional")}</span></label>
              <input id="account-label" onChange={(event) => setLabel(event.target.value)} value={label} />
              <label className="field-label" htmlFor="auth-json">auth.json</label>
              <textarea id="auth-json" ref={jsonRef} required spellCheck={false} />
              <div className="modal-actions">
                <button className="button button-secondary" disabled={busy !== null} onClick={() => setAddMethod("start")} type="button">{t("common.cancel")}</button>
                <button className="button button-primary" disabled={busy !== null} type="submit">
                  {busy === "import-json" ? <LoaderCircle className="spin" size={16} /> : <FileJson size={16} />}
                  {t("common.addAccount")}
                </button>
              </div>
            </form>
          ) : null}

          {addMethod === "api-key" ? (
            <form className="credential-form" onSubmit={submitApiKey}>
              <button className="back-link" disabled={busy !== null} onClick={() => setAddMethod("start")} type="button">{t("form.allMethods")}</button>
              <h3>{t("form.apiKeyTitle")}</h3>
              <p>{t("form.apiKeyBody")}</p>
              <label className="field-label" htmlFor="api-label">{t("form.displayName")} <span>{t("common.optional")}</span></label>
              <input id="api-label" onChange={(event) => setLabel(event.target.value)} value={label} />
              <label className="field-label" htmlFor="api-key">{t("account.apiKey")}</label>
              <input autoComplete="off" id="api-key" ref={apiKeyRef} required spellCheck={false} type="password" />
              <div className="modal-actions">
                <button className="button button-secondary" disabled={busy !== null} onClick={() => setAddMethod("start")} type="button">{t("common.cancel")}</button>
                <button className="button button-primary" disabled={busy !== null} type="submit">
                  {busy === "import-key" ? <LoaderCircle className="spin" size={16} /> : <KeyRound size={16} />}
                  {t("common.addAccount")}
                </button>
              </div>
            </form>
          ) : null}
        </Modal>
      ) : null}

      {dialog === "export" ? (
        <Modal
          dismissible={!runningAny("export-accounts")}
          onClose={() => {
            confirm.disarm();
            setDialog(null);
          }}
          t={t}
          title={t(selectedAccountIds.length === 1 ? "export.headingOne" : "export.headingMany", { count: formatNumber(selectedAccountIds.length, locale.formatLocale) })}
        >
          <div className="confirm-panel">
            <ShieldAlert size={26} />
            <p>{t("export.body")}</p>
            <div className="modal-actions">
              <button
                className="button button-secondary"
                disabled={busy !== null}
                onClick={() => {
                  confirm.disarm();
                  setDialog(null);
                }}
                type="button"
              >
                {t("common.cancel")}
              </button>
              <button
                className="button button-danger"
                data-confirm="export"
                disabled={busy !== null || selectedAccountIds.length === 0}
                onClick={() => confirm.click("export", () => void exportSelectedAccounts())}
                type="button"
              >
                {busy === "export-accounts" ? <LoaderCircle className="spin" size={16} /> : <Download size={16} />}
                {confirm.armed === "export" ? t("export.confirm") : t("export.write")}
              </button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "storage-recovery" ? (
        <Modal
          dismissible={!runningAny("reset-damaged-store")}
          onClose={() => {
            confirm.disarm();
            setDialog(null);
          }}
          t={t}
          title={t("recovery.storageTitle")}
        >
          <div className="confirm-panel">
            <ShieldAlert size={26} />
            <p>{t("recovery.storageBody")}</p>
            <div className="modal-actions">
              <button
                className="button button-secondary"
                disabled={busy !== null}
                onClick={() => {
                  confirm.disarm();
                  setDialog(null);
                }}
                type="button"
              >
                {t("common.cancel")}
              </button>
              <button
                className="button button-danger"
                data-confirm="reset-storage"
                disabled={busy !== null}
                onClick={() => confirm.click("reset-storage", () => void resetDamagedAccountStore())}
                type="button"
              >
                {busy === "reset-damaged-store" ? <LoaderCircle className="spin" size={16} /> : <ShieldAlert size={16} />}
                {confirm.armed === "reset-storage" ? t("recovery.confirmResetStorage") : t("recovery.resetStorage")}
              </button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "enable-switching" ? (
        <Modal dismissible={!runningAny("enable-switching")} onClose={() => setDialog(null)} t={t} title={t("switching.title")}>
          <div className="confirm-panel">
            <ShieldAlert size={26} />
            <p>{t("switching.body")}</p>
            <div className="modal-actions">
              <button className="button button-secondary" disabled={busy !== null} onClick={() => setDialog(null)} type="button">{t("common.cancel")}</button>
              <button
                className="button button-primary"
                disabled={busy !== null}
                onClick={() =>
                  void runTask("enable-switching", api.enableAccountSwitching).then((enabled) => {
                    if (enabled) {
                      setDialog(null);
                      setNotice({ kind: "success", text: t("notice.switchingReady") });
                    }
                  })
                }
                type="button"
              >
                {t("switching.enable")}
              </button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "recover-switch" ? (
        <Modal dismissible={!runningAny("recover-switch")} onClose={() => setDialog(null)} t={t} title={t("recovery.switchTitle")}>
          <div className="confirm-panel">
            <CircleAlert size={26} />
            <p>{t("recovery.switchBody")}</p>
            <div className="modal-actions">
              <button className="button button-secondary" disabled={busy !== null} onClick={() => setDialog(null)} type="button">{t("common.cancel")}</button>
              <button
                className="button button-primary"
                disabled={busy !== null}
                onClick={() =>
                  void runVoidTask("recover-switch", api.recoverPendingSwitch).then((recovered) => {
                    if (recovered) {
                      setDialog(null);
                      setNotice({ kind: "success", text: t("notice.switchRecovered") });
                    }
                  })
                }
                type="button"
              >
                {t("recovery.recoverSafely")}
              </button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "recover-reset-credit" ? (
        <Modal
          dismissible={!runningAny("recover-reset-credit", "discard-reset-credit")}
          onClose={() => {
            setDiscardConfirmation(false);
            setDialog(null);
          }}
          t={t}
          title={t("recovery.resetTitle")}
        >
          <div className="confirm-panel">
            <CircleAlert size={26} />
            <p>{t("recovery.resetBody", { name: pendingResetAccount ? accountPrimaryName(pendingResetAccount) : t("recovery.removedAccount") })}</p>
            {discardConfirmation ? <p>{t("recovery.discardWarning")}</p> : null}
            <div className="modal-actions">
              {pendingReset?.discardable ? (
                <button className="button button-danger" disabled={busy !== null} onClick={() => void discardPendingReset()} type="button">
                  {busy === "discard-reset-credit" ? <LoaderCircle className="spin" size={16} /> : <Trash2 size={16} />}
                  {discardConfirmation ? t("recovery.confirmDiscard") : t("recovery.discard")}
                </button>
              ) : null}
              {pendingResetAccount ? (
                <button
                  className="button button-primary"
                  disabled={busy !== null}
                  onClick={() => void recoverPendingResetCredit()}
                  type="button"
                >
                  {busy === "recover-reset-credit" ? <LoaderCircle className="spin" size={16} /> : <RefreshCw size={16} />}
                  {t("recovery.retry")}
                </button>
              ) : null}
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "reset" && resetAccount ? (
        <Modal
          dismissible={!runningAny("reset:")}
          onClose={() => {
            confirm.disarm();
            setDialog(null);
          }}
          t={t}
          title={t("reset.title", { name: resetDialogName(resetAccount) })}
        >
          <div className="reset-details">
            <div className="credit-list-head">
              <span className="badge badge-active">{t("reset.availableCount", { count: formatNumber(resetCredits?.available_count ?? 0, locale.formatLocale) })}</span>
            </div>
            {!resetCredits?.details_available ? (
              <div className="inline-warning"><CircleAlert size={17} />{t("reset.detailsUnavailable")}</div>
            ) : resetCredits.usable_credits.length ? (
              <ul className="credit-list">
                {resetCredits.usable_credits.map((credit, index) => {
                  const pick = `reset-${credit.expires_at ?? "none"}-${credit.granted_at ?? "none"}-${index}`;
                  const expiry = credit.expires_at ? formatDateTime(credit.expires_at, locale.formatLocale) : undefined;
                  const armed = confirm.armed === pick;
                  return (
                    <li key={pick}>
                      <span className="credit-copy">
                        <strong>{credit.title ?? t("reset.creditTitleFallback")}</strong>
                        <small>
                          {credit.expires_at
                            ? t("reset.expiresAt", { date: expiry!, relative: formatRelativeTime(credit.expires_at, locale.formatLocale) })
                            : t("reset.expiryUnknown")}
                        </small>
                      </span>
                      <button
                        aria-label={expiry
                          ? t(armed ? "reset.confirmLabel" : "reset.useLabel", { date: expiry })
                          : t(armed ? "reset.confirmLabelUndated" : "reset.useLabelUndated")}
                        className={"button " + (armed ? "button-danger" : "button-secondary")}
                        data-confirm={pick}
                        disabled={busy !== null}
                        onClick={() => confirm.click(pick, () => void redeemReset(credit, pick))}
                        type="button"
                      >
                        {redeemingCredit === pick ? <LoaderCircle className="spin" size={15} /> : null}
                        {armed ? t("reset.confirm") : t("reset.use")}
                      </button>
                    </li>
                  );
                })}
              </ul>
            ) : (
              <div className="inline-warning"><CircleAlert size={17} />{t("reset.noneUsable")}</div>
            )}
            {resetCredits?.details_available && resetCredits.available_count > resetCredits.usable_credits.length ? (
              <p className="credit-unlisted">
                {t("reset.unlisted", { count: formatNumber(resetCredits.available_count - resetCredits.usable_credits.length, locale.formatLocale) })}
              </p>
            ) : null}
          </div>
        </Modal>
      ) : null}

      {dialog === "remove" && removeAccount ? (
        <Modal
          dismissible={!runningAny("remove:")}
          onClose={() => {
            setDialog(null);
            setRemoveError(null);
          }}
          t={t}
          title={t("remove.title", { name: accountPrimaryName(removeAccount) })}
        >
          <div className="confirm-panel">
            <Trash2 size={26} />
            <p>{t("remove.body")}</p>
            <div className="remove-account-identity">
              <strong>{accountPrimaryName(removeAccount)}</strong>
              {accountSecondaryName(removeAccount, accountPrimaryName(removeAccount), t) ? (
                <span>{accountSecondaryName(removeAccount, accountPrimaryName(removeAccount), t)}</span>
              ) : null}
            </div>
            {removeError ? <p className="remove-error" role="alert">{removeError}</p> : null}
            <div className="modal-actions">
              <button className="button button-secondary" disabled={busy !== null} onClick={() => { setDialog(null); setRemoveError(null); }} type="button">{t("common.cancel")}</button>
              <button className="button button-danger" disabled={busy !== null} onClick={() => void removeSavedAccount()} type="button"><Trash2 size={16} />{t("remove.confirm")}</button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "wake" && wake ? (
        <Modal dismissible={wake.status !== "running" && !runningAny("wake-all", "wake-selected", "cancel-wake")} onClose={() => { setDialog(null); setWake(null); }} t={t} title={t("wake.title")}>
          <div className="wake-panel">
            <div className="wake-status">
              {wake.status === "running" ? <LoaderCircle className="spin" size={21} /> : wake.status === "completed" ? <Info size={21} /> : <CircleAlert size={21} />}
              <div>
                <strong>{wake.status === "running" ? t("wake.running") : wake.status === "completed" ? t("wake.resultsTitle") : t("wake.stopped")}</strong>
                <p>{wake.status === "running" ? t("wake.processed", { count: wake.results.length }) : wakeSummary(wake, t)}</p>
                {wake.status === "running" && wakeStatusUnavailable ? <p role="status">{t("error.wakeStatus")}</p> : null}
              </div>
            </div>
            {wake.results.length > 0 ? <ul className="wake-results">
              {wake.results.map((result, index) => {
                const account = accounts.find((saved) => saved.id === result.account_id);
                const primary = account ? accountPrimaryName(account) : result.label;
                const secondary = account ? accountSecondaryName(account, primary, t) : undefined;
                return (
                  <li key={result.account_id + "-" + result.result + "-" + index}>
                    <span className={"wake-result-dot wake-" + result.result} />
                    <div>
                      <div className="wake-result-identity">
                        <strong>{primary}</strong>
                        {secondary ? <span>{secondary}</span> : null}
                      </div>
                      <p>{wakeResultLabel(result, wake.alternate_model === true, t)}</p>
                      {result.result === "needs_sign_in" && account ? (
                        <button className="wake-retry" onClick={() => void startOAuth(account)} type="button">{t("account.signInAgain")}</button>
                      ) : null}
                      {wake.status !== "running" && result.result === "model_unavailable" && !wake.alternate_model ? (
                        <button
                          className="wake-retry"
                          disabled={busy !== null}
                          onClick={() => void startWake(result.account_id, true)}
                          aria-label={t("wake.retryAlternateFor", { name: primary })}
                          type="button"
                        >
                          {t("wake.retryAlternate")}
                        </button>
                      ) : null}
                    </div>
                  </li>
                );
              })}
            </ul> : null}
            <div className="modal-actions">
              {wake.status === "running" ? (
                <button className="button button-secondary" disabled={busy !== null} onClick={() => void runVoidTask("cancel-wake", () => api.cancelWake(wake.id), false)} type="button">{t("wake.cancelRemaining")}</button>
              ) : null}
              {wake.status !== "running" ? <button className="button button-primary" disabled={busy !== null} onClick={() => {
                setDialog(null);
                setWake(null);
              }} type="button">{t("common.done")}</button> : null}
            </div>
          </div>
        </Modal>
      ) : null}
    </main>
  );
}
