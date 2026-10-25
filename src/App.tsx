import {
  Check,
  CircleAlert,
  CircleCheck,
  Download,
  ListChecks,
  LoaderCircle,
  Plus,
  RefreshCw,
  ShieldAlert,
  Trash2,
  X,
  Zap,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { api, type UpdateDelivery } from "./api";
import {
  createTranslator,
  formatDateTime,
  formatNumber,
  formatRelativeTime,
  readLanguagePreference,
  resolveLocale,
  saveLanguagePreference,
  type AppLanguage,
  type LanguagePreference,
  type Translator,
} from "./i18n";
import { AccountCard, accountPrimaryName, accountSecondaryName, type AccountBusyAction } from "./AccountCard";
import { PopoverMenu } from "./PopoverMenu";
import { useAccountQuota } from "./useAccountQuota";
import type {
  AccountView,
  LiveAccountView,
  PendingResetView,
  ResetCreditDetail,
  ResetCreditOutcome,
  StorageView,
  SwitchFailure,
  SwitchFailureCode,
} from "./types";
import { AccountIntakeDialog, FirstRun, useAccountIntake } from "./AccountIntake";
import { WakeDialog, useWake } from "./Wake";
import { Modal } from "./Modal";
import { friendlyError, type Notice } from "./feedback";
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
  | null;


type UpdatePhase = "available" | "downloading" | "installing" | "ready" | "error";

interface PendingUpdate {
  update: AvailableUpdate;
  delivery: UpdateDelivery;
}

// A running GSwitch checks again after six hours, when it comes back to the
// foreground an hour after its last check, and five minutes after a failure.
const UPDATE_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;
const UPDATE_FOCUS_CHECK_MS = 60 * 60 * 1000;
const UPDATE_RETRY_MS = 5 * 60 * 1000;
const MAX_UPDATE_NOTES = 12;
const MAX_UPDATE_NOTE_LENGTH = 300;

// Notes come from the unsigned latest.json, so they are only ever shown as
// bounded plain text. GSwitch's manifest adds them per UI language.
function updateNotes(update: AvailableUpdate, language: AppLanguage) {
  const localized = update.rawJson?.localized_notes;
  const value = localized && typeof localized === "object"
    ? (localized as Record<string, unknown>)[language]
    : undefined;
  const text = typeof value === "string" ? value : update.body ?? "";
  return text
    .split(/\r?\n/)
    .map((line) => line.replace(/^\s*-\s+/, "").trim())
    .filter(Boolean)
    .slice(0, MAX_UPDATE_NOTES)
    .map((line) => line.slice(0, MAX_UPDATE_NOTE_LENGTH));
}

function closeUpdate(update: AvailableUpdate) {
  void Promise.resolve().then(() => update.close()).catch(() => undefined);
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
  ].includes(busy) || busy.startsWith("switch:") || busy.startsWith("reset:") || busy.startsWith("remove:");
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

function UpdateNotice({
  pending,
  notes,
  phase,
  progress,
  onInstall,
  onLater,
  onRestart,
  t,
}: {
  pending: PendingUpdate;
  notes: string[];
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
        {phase === "available" && notes.length ? (
          <ul aria-label={t("update.notes")} className="update-notes" tabIndex={0}>
            {notes.map((note, index) => <li key={index}>{note}</li>)}
          </ul>
        ) : null}
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
  const [accountSignInNeeded, setAccountSignInNeeded] = useState<Record<string, boolean>>({});
  const [live, setLive] = useState<LiveAccountView | null>(null);
  const [storage, setStorage] = useState<StorageView | null>(null);
  const [pendingReset, setPendingReset] = useState<PendingResetView | null>(null);
  const pendingResetCredit = pendingReset !== null;
  const [discardConfirmation, setDiscardConfirmation] = useState(false);
  const [loading, setLoading] = useState(true);
  const [taskBusy, setBusy] = useState<string | null>(null);
  const [accountBusy, setAccountBusy] = useState<Record<string, AccountBusyAction>>({});
  const [dialog, setDialog] = useState<Dialog>(null);
  const [selectionMode, setSelectionMode] = useState(false);
  const [selectedAccountIds, setSelectedAccountIds] = useState<string[]>([]);
  const confirm = useConfirmClick();
  const [notice, setNotice] = useState<Notice | null>(null);
  const [resetAccount, setResetAccount] = useState<AccountView | null>(null);
  const [removeAccount, setRemoveAccount] = useState<AccountView | null>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);
  const [redeemingCredit, setRedeemingCredit] = useState<string | null>(null);
  const [pendingUpdate, setPendingUpdate] = useState<PendingUpdate | null>(null);
  const [updatePhase, setUpdatePhase] = useState<UpdatePhase>("available");
  const [updateProgress, setUpdateProgress] = useState<number | null>(null);
  const dismissedUpdateVersions = useRef(new Set<string>());
  const updateCheckInFlight = useRef(false);
  const updateChecksActive = useRef(false);
  const updateCheckTimer = useRef<number | undefined>(undefined);
  const lastUpdateCheck = useRef(0);
  const pendingUpdateRef = useRef<PendingUpdate | null>(null);
  const updatePhaseRef = useRef<UpdatePhase>("available");
  const accountOperations = useRef(new Set<string>());
  const snapshotSequence = useRef(0);
  const initialSnapshotLoaded = useRef(false);
  const locale = useMemo(() => resolveLocale(languagePreference), [languagePreference]);
  const t = useMemo(() => createTranslator(locale.language), [locale.language]);

  const { quotas, failures: quotaFailures, now: clock, progress: refreshProgress, refresh: requestQuotaRefresh, refreshAll: refreshAllQuotas, refreshAfterChange: refreshChangedQuota, invalidate: invalidateQuota, applyReset: applyResetQuota } = useAccountQuota({
    accounts, activeId: live?.account?.id, enabled: !loading && storage?.status === "ready",
  });

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
  }, [t]);

  useEffect(() => {
    void loadSnapshot();
  }, [loadSnapshot]);

  useEffect(() => { pendingUpdateRef.current = pendingUpdate; }, [pendingUpdate]);
  useEffect(() => { updatePhaseRef.current = updatePhase; }, [updatePhase]);

  const checkForUpdate = useCallback(async () => {
    if (!("__TAURI_INTERNALS__" in window) || updateCheckInFlight.current) {
      return;
    }
    updateCheckInFlight.current = true;
    lastUpdateCheck.current = Date.now();
    let nextCheck = UPDATE_RETRY_MS;
    try {
      const update = await updater.check();
      if (update) {
        const current = pendingUpdateRef.current;
        const installing = ["downloading", "installing", "ready"].includes(updatePhaseRef.current);
        if (dismissedUpdateVersions.current.has(update.version)
          || current?.update.version === update.version
          || (current && installing)) {
          closeUpdate(update);
        } else {
          const delivery = await api.updateDelivery().catch((error: unknown) => {
            closeUpdate(update);
            throw error;
          });
          if (current) closeUpdate(current.update);
          pendingUpdateRef.current = { update, delivery };
          setPendingUpdate(pendingUpdateRef.current);
          setUpdatePhase("available");
          setUpdateProgress(null);
        }
      }
      nextCheck = UPDATE_CHECK_INTERVAL_MS;
    } catch {
      // A failed background check stays quiet and is retried soon. A
      // user-initiated install reports an actionable retry state instead.
    } finally {
      updateCheckInFlight.current = false;
      window.clearTimeout(updateCheckTimer.current);
      if (updateChecksActive.current) {
        updateCheckTimer.current = window.setTimeout(() => void checkForUpdate(), nextCheck);
      }
    }
  }, []);

  useEffect(() => {
    updateChecksActive.current = true;
    void checkForUpdate();
    const checkIfStale = () => {
      if (document.visibilityState !== "hidden" && Date.now() - lastUpdateCheck.current >= UPDATE_FOCUS_CHECK_MS) {
        void checkForUpdate();
      }
    };
    window.addEventListener("focus", checkIfStale);
    document.addEventListener("visibilitychange", checkIfStale);
    return () => {
      updateChecksActive.current = false;
      window.clearTimeout(updateCheckTimer.current);
      window.removeEventListener("focus", checkIfStale);
      document.removeEventListener("visibilitychange", checkIfStale);
    };
  }, [checkForUpdate]);

  const intake = useAccountIntake({
    t, formatLocale: locale.formatLocale, notify: setNotice, onChanged: loadSnapshot,
    onShow: visible => setDialog(visible ? "add" : null),
    onStartingSignIn: invalidateQuota,
    onAuthenticated: (account, target) => {
      if (target) setAccountSignInNeeded(current => ({ ...current, [target.id]: false }));
      invalidateQuota(account.id); refreshChangedQuota(account);
    },
  });
  const { startOAuth, chooseImportFile, openAddDialog } = intake;
  const wakeFlow = useWake({
    t, notify: setNotice, onChanged: loadSnapshot, onRefreshed: refreshChangedQuota,
    onShow: visible => setDialog(visible ? "wake" : null),
  });
  const { wake, startWake } = wakeFlow;
  const startSelectedWake = () => wakeFlow.startSelectedWake(accounts.filter(account => account.kind === "chat_gpt" && selectedAccountIds.includes(account.id)).map(account => account.id));
  const busy = taskBusy ?? intake.busy ?? wakeFlow.busy;

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

  // A refresh only reads quota, so the same account can start Wake while it
  // runs; a second request for an action already running is ignored.
  const runAccountTask = useCallback(
    async <T,>(accountId: string, action: Exclude<AccountBusyAction, "switch">, task: () => Promise<T>): Promise<T | undefined> => {
      const key = accountId + ":" + action;
      if (accountOperations.current.has(key)) {
        return undefined;
      }

      accountOperations.current.add(key);
      setAccountBusy((current) => ({ ...current, [accountId]: action }));
      try {
        return await task();
      } catch (error) {
        setNotice({ kind: "error", text: friendlyError(t, error) });
        return undefined;
      } finally {
        accountOperations.current.delete(key);
        const stillRunning = (["refresh", "wake"] as const).find((other) =>
          accountOperations.current.has(accountId + ":" + other));
        setAccountBusy((current) => {
          const remaining = { ...current };
          if (stillRunning) {
            remaining[accountId] = stillRunning;
          } else {
            delete remaining[accountId];
          }
          return remaining;
        });
      }
    },
    [t],
  );

  const dismissUpdate = () => {
    if (pendingUpdate) {
      dismissedUpdateVersions.current.add(pendingUpdate.update.version);
      closeUpdate(pendingUpdate.update);
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

  const copyAccountEmail = async (account: AccountView) => {
    if (!account.email) return;
    try {
      await navigator.clipboard.writeText(account.email);
      setNotice({ kind: "success", text: t("notice.emailCopied") });
    } catch {
      setNotice({ kind: "error", text: t("notice.emailCopyFailed") });
    }
  };

  const refreshAll = async () => {
    const { failures } = await refreshAllQuotas();
    if (failures) setNotice({ kind: "info", text: t("notice.quotaUnavailable", { count: formatNumber(failures, locale.formatLocale) }) });
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
      refreshChangedQuota(outcome.account);
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
    invalidateQuota(resetAccount.id);
    setRedeemingCredit(pick);
    const result = await runTask(
      "reset:" + resetAccount.id,
      () => api.redeemResetCredit(resetAccount.id, credit),
    );
    invalidateQuota(resetAccount.id);
    setRedeemingCredit(null);
    if (!result) {
      // A failed request may have left a pending record that needs recovery.
      void loadSnapshot();
      return;
    }
    applyResetQuota(result.account_id, result.quota);
    setNotice(resetOutcomeNotice(result));
    setResetAccount(null);
    setDialog(null);
  };

  const recoverPendingResetCredit = async () => {
    if (pendingReset) invalidateQuota(pendingReset.account_id);
    const result = await runTask("recover-reset-credit", api.recoverPendingResetCredit);
    if (!result) {
      void loadSnapshot();
      return;
    }
    applyResetQuota(result.account_id, result.quota);
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

  const currentActiveId = live?.account?.id;
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
            disabled={busy !== null || loading || storageRecovery || (!wake && chatGptAccounts.length === 0)}
            onClick={() => wake ? setDialog("wake") : void startWake()}
            type="button"
          >
            {wake?.status === "running" ? <LoaderCircle className="spin" size={16} /> : <Zap size={16} />}
            {wake ? wake.status === "running"
              ? t("wake.progress", { count: formatNumber(wake.results.length, locale.formatLocale) })
              : t("wake.viewResults")
              : t("toolbar.wakeAll")}
          </button>
          <button
            className="button button-primary"
            disabled={loading || busy !== null || storageRecovery}
            onClick={() => openAddDialog()}
            type="button"
          >
            <Plus size={16} />
            {t("common.addAccount")}
          </button>
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
            notes={updateNotes(pendingUpdate.update, locale.language)}
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
                  disabled={busy !== null || selectedChatGptCount === 0 || wake?.status === "running"}
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
                  busyAction={wakeFlow.startingAccount === account.id ? "wake" : accountBusy[account.id] ?? (busy === "switch:" + account.id ? "switch" : undefined)}
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
              busy={loading || busy !== null}
              onSignIn={() => void startOAuth()}
              onImport={() => void chooseImportFile()}
              onSelectMethod={openAddDialog}
              t={t}
            />
          )}
        </section>
      </div>

      {dialog === "add" ? <AccountIntakeDialog flow={intake} busy={busy} onCopyEmail={copyAccountEmail} t={t} /> : null}

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

      {dialog === "wake" ? <WakeDialog flow={wakeFlow} accounts={accounts} busy={busy} startOAuth={startOAuth} t={t} /> : null}
    </main>
  );
}
