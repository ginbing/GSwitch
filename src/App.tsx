import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  ArrowRightLeft,
  Check,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  Copy,
  FileJson,
  FolderOpen,
  Globe2,
  KeyRound,
  LoaderCircle,
  MoreHorizontal,
  Plus,
  RefreshCw,
  Settings,
  ShieldAlert,
  Trash2,
  Upload,
  X,
  Zap,
} from "lucide-react";
import {
  type FormEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { api, type UpdateDelivery } from "./api";
import {
  createTranslator,
  formatDateTimeWithRelative,
  formatNumber,
  formatPercent,
  readLanguagePreference,
  resolveLocale,
  saveLanguagePreference,
  type LanguagePreference,
  type Translator,
} from "./i18n";
import type {
  AccountView,
  LiveAccountView,
  MigrationCandidate,
  MigrationPreview,
  OAuthLoginStart,
  OAuthLoginStatus,
  QuotaView,
  QuotaWindow,
  RuntimeInfo,
  StorageView,
  WakeOperationView,
} from "./types";
import { updater, type AvailableUpdate } from "./updater";

type Dialog =
  | "add"
  | "settings"
  | "enable-switching"
  | "recover-switch"
  | "recover-reset-credit"
  | "storage-recovery"
  | "reset"
  | "remove"
  | "wake"
  | null;

type AddMethod = "start" | "oauth" | "json" | "api-key" | "migration";

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

function friendlyError(t: Translator, error: unknown, fallback = t("error.actionIncomplete")) {
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
  return `${fallback} ${t("error.currentUnchanged")}`;
}

function quotaRefreshMessage(t: Translator, error: unknown) {
  if (/currently using this account|cannot safely identify its active account/i.test(String(error))) {
    return t("quota.runningCodex");
  }
  return undefined;
}

function primaryWindow(quota: QuotaView | undefined, kind: "five_hour" | "weekly") {
  return quota?.snapshot?.buckets
    .filter((bucket) => bucket.kind === "codex")
    .flatMap((bucket) => bucket.windows)
    .find((window) => window.kind === kind);
}

function accountPlan(account: AccountView, t: Translator) {
  return account.kind === "api_key" ? t("account.apiKey") : account.plan_type || t("account.chatGpt");
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

function credentialStoreLabel(store: RuntimeInfo["credential_store"] | undefined, t: Translator) {
  const labels = {
    file: "credentialStore.file",
    keyring: "credentialStore.keyring",
    auto: "credentialStore.auto",
    ephemeral: "credentialStore.ephemeral",
    unknown: "credentialStore.unknown",
  } as const;
  return t(labels[store || "unknown"]);
}

function wakeStatusLabel(status: WakeOperationView["status"], t: Translator) {
  const key = `wake.status${status.slice(0, 1).toUpperCase()}${status.slice(1)}` as
    | "wake.statusRunning"
    | "wake.statusCompleted"
    | "wake.statusCancelled"
    | "wake.statusFailed";
  return t(key);
}

function wakeResultLabel(result: WakeOperationView["results"][number]["result"], t: Translator) {
  const labels = {
    started: "wake.started",
    already_active: "wake.alreadyActive",
    no_ordinary_capacity: "wake.noOrdinaryCapacity",
    needs_sign_in: "wake.needsSignIn",
    sent_not_confirmed: "wake.sentNotConfirmed",
    failed: "wake.failed",
    cancelled: "wake.cancelled",
  } as const;
  return t(labels[result]);
}

function Modal({
  title,
  children,
  onClose,
  t,
  wide = false,
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
  t: Translator;
  wide?: boolean;
}) {
  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target) {
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
      >
        <div className="modal-header">
          <h2 id="modal-title">{title}</h2>
          <button aria-label={t("common.closeDialog", { title })} className="icon-button" onClick={onClose} type="button">
            <X size={18} strokeWidth={2} />
          </button>
        </div>
        {children}
      </section>
    </div>
  );
}

function QuotaMeter({
  label,
  window,
  status,
  t,
  formatLocale,
}: {
  label: string;
  window?: QuotaWindow;
  status?: QuotaView["status"];
  t: Translator;
  formatLocale: string;
}) {
  const known = status === "fresh" || status === "stale";
  const remaining = window?.remaining_percent;
  const value = typeof remaining === "number" ? Math.max(0, Math.min(100, remaining)) : undefined;

  return (
    <div className="quota-meter">
      <div className="quota-heading">
        <span>{label}</span>
        <strong>{value === undefined || !known ? "—" : formatPercent(value, formatLocale)}</strong>
      </div>
      <div
        aria-label={t("quota.remaining", { label })}
        aria-valuemax={100}
        aria-valuemin={0}
        aria-valuenow={value}
        className="quota-track"
        role={value === undefined ? undefined : "progressbar"}
      >
        <span
          className={value !== undefined && value < 15 ? "quota-low" : ""}
          style={{ width: String(value ?? 0) + "%" }}
        />
      </div>
      <small>
        {!known
          ? t("quota.notAvailable")
          : status === "stale"
            ? t("quota.lastResultStale")
            : window?.resets_at
              ? t("quota.resets", { time: formatDateTimeWithRelative(window.resets_at, formatLocale) })
              : t("quota.resetTimeUnavailable")}
      </small>
    </div>
  );
}

function AccountCard({
  account,
  quota,
  active,
  busy,
  onSwitch,
  onWake,
  onRefresh,
  onReset,
  resetRecoveryRequired,
  onRemove,
  t,
  formatLocale,
}: {
  account: AccountView;
  quota?: QuotaView;
  active: boolean;
  busy: boolean;
  onSwitch: () => void;
  onWake: () => void;
  onRefresh: () => void;
  onReset: () => void;
  resetRecoveryRequired: boolean;
  onRemove: () => void;
  t: Translator;
  formatLocale: string;
}) {
  const credits = quota?.snapshot?.reset_credits;
  const isApiKey = account.kind === "api_key";
  const primaryName = accountPrimaryName(account);
  const secondaryName = accountSecondaryName(account, primaryName, t);

  return (
    <article className={"account-card" + (active ? " account-active" : "")}>
      <div className="account-card-head">
        <div className="account-identity">
          <div className="account-avatar" aria-hidden="true">
            {primaryName.slice(0, 1).toUpperCase()}
          </div>
          <div className="account-copy">
            <h3>{primaryName}</h3>
            {secondaryName ? <p>{secondaryName}</p> : null}
          </div>
        </div>
        <details className="card-menu">
          <summary aria-label={t("account.moreActions", { name: primaryName })}>
            <MoreHorizontal size={18} />
          </summary>
          <div className="card-menu-popover">
            <button
              aria-label={t("account.remove", { name: primaryName })}
              disabled={active || busy}
              onClick={onRemove}
              type="button"
            >
              <Trash2 size={15} />
              {t("common.removeAccount")}
            </button>
          </div>
        </details>
      </div>

      <div className="account-badges">
        {active ? <span className="badge badge-active"><Check size={13} /> {t("account.active")}</span> : null}
        <span className="badge">{accountPlan(account, t)}</span>
        {quota?.status === "stale" ? <span className="badge badge-muted">{t("account.stale")}</span> : null}
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
          <div className="quota-pair">
            <QuotaMeter formatLocale={formatLocale} label={t("quota.fiveHour")} status={quota?.status} t={t} window={primaryWindow(quota, "five_hour")} />
            <QuotaMeter formatLocale={formatLocale} label={t("quota.weekly")} status={quota?.status} t={t} window={primaryWindow(quota, "weekly")} />
          </div>
          {quota?.message ? <p className="quota-status-message">{quota.message}</p> : null}
          <div className="credit-row">
            <span>
              {t("credit.resetCredits")}
              <strong>{credits ? formatNumber(credits.available_count, formatLocale) : "—"}</strong>
            </span>
            {credits?.nearest_expiry ? <small>{t("credit.earliest", { time: formatDateTimeWithRelative(credits.nearest_expiry, formatLocale) })}</small> : null}
            {credits?.available_count && credits.available_count > 0 ? (
              <button className="text-button" onClick={onReset} type="button">
                {resetRecoveryRequired ? t("common.recoveryRequired") : t("common.details")}
                <ChevronRight size={15} />
              </button>
            ) : null}
          </div>
        </>
      )}

      <div className="card-footer">
        <button
          aria-label={t("account.refresh", { name: primaryName })}
          className="icon-button"
          disabled={busy || isApiKey}
          onClick={onRefresh}
          type="button"
        >
          <RefreshCw className={busy ? "spin" : ""} size={17} />
        </button>
        <div className="card-footer-actions">
          {!isApiKey ? (
            <button
              aria-label={t("account.wake", { name: primaryName })}
              className="button button-secondary"
              disabled={busy}
              onClick={onWake}
              type="button"
            >
              <Zap size={15} />
              {t("common.wake")}
            </button>
          ) : null}
          <button
            aria-label={t(active ? "account.current" : "account.switch", { name: primaryName })}
            className="button button-primary"
            disabled={busy || active}
            onClick={onSwitch}
            type="button"
          >
            {busy ? <LoaderCircle className="spin" size={15} /> : <ArrowRightLeft size={15} />}
            {active ? t("common.current") : t("common.switch")}
          </button>
        </div>
      </div>
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
    <section aria-label={t("update.available", { version: pending.update.version })} className="update-notice">
      <div>
        <strong>{t("update.available", { version: pending.update.version })}</strong>
        <p>
          {isReleaseDownload
            ? t("update.debianFallback")
            : phase === "ready"
              ? t("update.restartRequired")
              : phase === "error"
                ? t("update.failed")
                : t("update.safe")}
        </p>
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
  const [live, setLive] = useState<LiveAccountView | null>(null);
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [storage, setStorage] = useState<StorageView | null>(null);
  const [pendingResetCredit, setPendingResetCredit] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [addMethod, setAddMethod] = useState<AddMethod>("start");
  const [migrationPreview, setMigrationPreview] = useState<MigrationPreview | null>(null);
  const [migrationRoot, setMigrationRoot] = useState<string | undefined>();
  const [migrationSelection, setMigrationSelection] = useState<string[]>([]);
  const [migrationScanned, setMigrationScanned] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [oauth, setOauth] = useState<OAuthFlow | null>(null);
  const [wake, setWake] = useState<WakeOperationView | null>(null);
  const [resetAccount, setResetAccount] = useState<AccountView | null>(null);
  const [removeAccount, setRemoveAccount] = useState<AccountView | null>(null);
  const [resetConfirmation, setResetConfirmation] = useState(false);
  const [storageResetConfirmation, setStorageResetConfirmation] = useState(false);
  const [pendingUpdate, setPendingUpdate] = useState<PendingUpdate | null>(null);
  const [updatePhase, setUpdatePhase] = useState<UpdatePhase>("available");
  const [updateProgress, setUpdateProgress] = useState<number | null>(null);
  const jsonRef = useRef<HTMLTextAreaElement>(null);
  const apiKeyRef = useRef<HTMLInputElement>(null);
  const dismissedUpdateVersions = useRef(new Set<string>());
  const updateCheckInFlight = useRef(false);
  const quotaRefreshes = useRef(new Map<string, Promise<QuotaView>>());
  const [label, setLabel] = useState("");
  const locale = useMemo(() => resolveLocale(languagePreference), [languagePreference]);
  const t = useMemo(() => createTranslator(locale.language), [locale.language]);

  const changeLanguage = (preference: LanguagePreference) => {
    setLanguagePreference(preference);
    saveLanguagePreference(preference);
  };

  const requestQuotaRefresh = useCallback((accountId: string): Promise<QuotaView> => {
    const inFlight = quotaRefreshes.current.get(accountId);
    if (inFlight) {
      return inFlight;
    }

    const request = api.refreshAccountQuota(accountId).then(
      (quota) => {
        setQuotas((current) => ({ ...current, [accountId]: quota }));
        return quota;
      },
      (error) => {
        const message = quotaRefreshMessage(t, error);
        if (message) {
          setQuotas((current) => {
            const previous = current[accountId];
            return {
              ...current,
              [accountId]: {
                account_id: accountId,
                status: previous?.snapshot ? "stale" : "unknown",
                snapshot: previous?.snapshot,
                message,
              },
            };
          });
        }
        throw error;
      },
    );
    quotaRefreshes.current.set(accountId, request);
    void request.then(
      () => quotaRefreshes.current.get(accountId) === request && quotaRefreshes.current.delete(accountId),
      () => quotaRefreshes.current.get(accountId) === request && quotaRefreshes.current.delete(accountId),
    );
    return request;
  }, [t]);

  const loadSnapshot = useCallback(async () => {
    setLoading(true);
    try {
      const initial = await api.appSnapshot();
      const nextAccounts = initial.accounts;
      setAccounts(nextAccounts);
      setStorage(initial.storage);
      setPendingResetCredit(initial.pending_reset_credit);
      setLive(initial.live || null);
      setRuntime(initial.runtime || null);
      setQuotas({});
      if (initial.storage.status === "ready") {
        void Promise.allSettled(
          nextAccounts
            .filter((account) => account.kind === "chat_gpt")
            .map(async (account) => [account.id, await api.accountQuota(account.id)] as const),
        ).then((quotaPairs) => {
          const cached = Object.fromEntries(
            quotaPairs.flatMap((result) => result.status === "fulfilled" ? [result.value] : []),
          ) as Record<string, QuotaView>;
          setQuotas(cached);
          for (const quota of Object.values(cached)) {
            if (quota.status === "unknown" || quota.status === "stale") {
              void requestQuotaRefresh(quota.account_id).catch(() => undefined);
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
      setLoading(false);
    }
  }, [requestQuotaRefresh, t]);

  useEffect(() => {
    void loadSnapshot();
  }, [loadSnapshot]);

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
    if (!oauth || oauth.status.status !== "pending") {
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
        setOauth((current) =>
          current?.login_id === oauth.login_id ? { ...current, status } : current,
        );
        if (status.status === "complete") {
          setNotice({ kind: "success", text: t("notice.importedAccount", { name: status.account.label }) });
          setDialog(null);
          void loadSnapshot();
        } else if (status.status === "pending") {
          timer = window.setTimeout(poll, 800);
        }
      } catch {
        if (!closed) {
          setOauth((current) =>
            current?.login_id === oauth.login_id
              ? { ...current, status: { status: "failed", message: t("error.oauthStatus") } }
              : current,
          );
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
  }, [loadSnapshot, oauth, t]);

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
        setWake(next);
        if (next.status === "running") {
          timer = window.setTimeout(poll, 850);
        } else {
          void loadSnapshot();
        }
      } catch {
        if (!closed) {
          setNotice({ kind: "error", text: t("error.wakeStatus") });
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
  }, [loadSnapshot, t, wake]);

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && dialog) {
        setDialog(null);
      }
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [dialog]);

  const startOAuth = async () => {
    const result = await runTask("oauth", api.startOAuth, false);
    if (!result) {
      return;
    }
    setOauth({ ...result, status: { status: "pending" } });
    setAddMethod("oauth");
    try {
      await api.openOAuth(result.login_id);
    } catch {
      setNotice({ kind: "info", text: t("notice.oauthReady") });
    }
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
    const completed = await runVoidTask("oauth-cancel", () => api.cancelOAuth(oauth.login_id), false);
    if (completed) {
      setOauth((current) => current ? { ...current, status: { status: "cancelled" } } : current);
    }
  };

  const closeAddDialog = () => {
    if (oauth?.status.status === "pending") {
      void api.cancelOAuth(oauth.login_id);
    }
    setDialog(null);
  };

  const openAddDialog = () => {
    setOauth(null);
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
      setNotice({ kind: "success", text: t("notice.importedAccount", { name: result.label }) });
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

  const refreshAll = async () => {
    await runVoidTask(
      "refresh-all",
      async () => {
        const results = await Promise.allSettled(
          accounts
            .filter((account) => account.kind === "chat_gpt")
            .map((account) => requestQuotaRefresh(account.id)),
        );
        const failures = results.filter((result) => result.status === "rejected").length;
        if (failures) {
          setNotice({
            kind: "info",
            text: t("notice.quotaUnavailable", {
              count: formatNumber(failures, locale.formatLocale),
              suffix: failures === 1 ? " was" : "s were",
            }),
          });
        }
      },
      true,
    );
  };

  const switchAccount = async (account: AccountView) => {
    const result = await runTask("switch:" + account.id, () => api.switchAccount(account.id));
    if (result) {
      setNotice({ kind: "success", text: t("notice.switched", { name: account.label }) });
    }
  };

  const refreshAccount = async (account: AccountView) => {
    const result = await runTask("refresh:" + account.id, () => requestQuotaRefresh(account.id));
    if (result) {
      setNotice({ kind: "success", text: t("notice.quotaRefreshed", { name: account.label }) });
    }
  };

  const startWake = async (accountId?: string) => {
    const key = accountId ? "wake:" + accountId : "wake-all";
    const result = await runTask(
      key,
      () => accountId ? api.startWake(accountId) : api.startWakeAll(),
      false,
    );
    if (result) {
      setWake({ id: result.operation_id, status: "running", results: [] });
      setDialog("wake");
    }
  };

  const redeemReset = async () => {
    if (!resetAccount) {
      return;
    }
    if (!resetConfirmation) {
      setResetConfirmation(true);
      return;
    }
    const result = await runTask(
      "reset:" + resetAccount.id,
      () => api.redeemEarliestResetCredit(resetAccount.id),
    );
    if (result) {
      const complete = result.outcome === "reset" || result.outcome === "already_redeemed";
      setNotice({
        kind: complete ? "success" : "info",
        text: complete
          ? result.refresh_warning ? t("notice.resetRefreshWarning") : t("notice.resetUsed")
          : t("notice.noResetUsed"),
      });
      setResetAccount(null);
      setResetConfirmation(false);
      setDialog(null);
    }
  };

  const recoverPendingResetCredit = async () => {
    const result = await runTask("recover-reset-credit", api.recoverPendingResetCredit);
    if (result) {
      const confirmed = result.outcome === "reset" || result.outcome === "already_redeemed";
      setNotice({
        kind: confirmed ? "success" : "info",
        text: result.refresh_warning ? t("notice.resetRefreshWarning") : (confirmed
          ? t("notice.recoveredReset")
          : t("notice.resetNotConsumed")),
      });
      setDialog(null);
    }
  };

  const removeSavedAccount = async () => {
    if (!removeAccount) {
      return;
    }
    const completed = await runVoidTask(
      "remove:" + removeAccount.id,
      () => api.removeSavedAccount(removeAccount.id),
    );
    if (completed) {
      setNotice({ kind: "success", text: t("notice.removed", { name: removeAccount.label }) });
      setRemoveAccount(null);
      setDialog(null);
    }
  };

  const resetDamagedAccountStore = async () => {
    if (!storageResetConfirmation) {
      setStorageResetConfirmation(true);
      return;
    }
    const completed = await runVoidTask(
      "reset-damaged-store",
      api.resetDamagedAccountStore,
    );
    if (completed) {
      setStorageResetConfirmation(false);
      setDialog(null);
      setNotice({
        kind: "success",
        text: t("notice.storageReset"),
      });
    }
  };

  const currentActiveId = live?.account?.id;
  const liveAccountLabel = live?.account?.label || t("toolbar.currentAccount");
  const chatGptAccounts = accounts.filter((account) => account.kind === "chat_gpt");
  const resetCredits = resetAccount ? quotas[resetAccount.id]?.snapshot?.reset_credits : undefined;
  const storageRecovery = storage?.status === "recovery_required";

  const safetyNotice = useMemo(() => {
    if (storage?.status === "recovery_required") {
      return {
        icon: <ShieldAlert size={20} />,
        title: t("safety.storageTitle"),
        body: t("safety.storageBody"),
        action: t("safety.reviewRecovery"),
        onAction: () => {
          setStorageResetConfirmation(false);
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
              setNotice({ kind: "success", text: t("notice.savedCurrent", { name: account.label }) });
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
          <div className="brand-mark" aria-hidden="true"><ArrowRightLeft size={20} strokeWidth={2.4} /></div>
          <div>
            <h1>GSwitch</h1>
            <p>
              <span className={live?.status === "ready" ? "status-dot status-ready" : "status-dot"} />
              {liveAccountLabel}
            </p>
          </div>
        </div>
        <div className="toolbar-actions">
          <button className="button button-quiet" disabled={loading || busy !== null || storageRecovery} onClick={() => void refreshAll()} type="button">
            <RefreshCw className={busy === "refresh-all" ? "spin" : ""} size={16} />
            {t("common.refresh")}
          </button>
          <button
            className="button button-secondary"
            disabled={loading || busy !== null || storageRecovery || chatGptAccounts.length === 0}
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
          <button aria-label={t("toolbar.openSettings")} className="icon-button" onClick={() => setDialog("settings")} type="button">
            <Settings size={18} />
          </button>
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
          <div className={"toast toast-" + notice.kind} role="status">
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

        <section className="accounts-section" aria-labelledby="accounts-heading">
          <div className="section-heading">
            <div>
              <span className="eyebrow">{t("accounts.eyebrow")}</span>
              <h2 id="accounts-heading">
                {loading
                  ? t("accounts.loading")
                  : accounts.length === 1
                    ? t("accounts.savedOne")
                    : t("accounts.savedMany", { count: formatNumber(accounts.length, locale.formatLocale) })}
              </h2>
            </div>
            <p>{storageRecovery ? t("accounts.recoveryDescription") : t("accounts.readyDescription")}</p>
          </div>

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
                  setStorageResetConfirmation(false);
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
                  busy={busy !== null}
                  formatLocale={locale.formatLocale}
                  key={account.id}
                  onRefresh={() => void refreshAccount(account)}
                  onRemove={() => {
                    setRemoveAccount(account);
                    setDialog("remove");
                  }}
                  onReset={() => {
                    if (pendingResetCredit) {
                      setDialog("recover-reset-credit");
                      return;
                    }
                    setResetAccount(account);
                    setResetConfirmation(false);
                    setDialog("reset");
                  }}
                  resetRecoveryRequired={pendingResetCredit}
                  onSwitch={() => void switchAccount(account)}
                  onWake={() => void startWake(account.id)}
                  quota={quotas[account.id]}
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

      {dialog === "add" ? (
        <Modal onClose={closeAddDialog} t={t} title={t("add.title")} wide>
          {addMethod === "start" ? (
            <div className="add-methods">
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
              <button className="add-method-card" onClick={() => void startOAuth()} type="button">
                <span className="method-icon"><Globe2 size={22} /></span>
                <span><strong>{t("add.browserTitle")}</strong><small>{t("add.browserDescription")}</small></span>
                <ChevronRight size={18} />
              </button>
              <button className="add-method-card" onClick={() => setAddMethod("json")} type="button">
                <span className="method-icon"><FileJson size={22} /></span>
                <span><strong>{t("add.jsonTitle")}</strong><small>{t("add.jsonDescription")}</small></span>
                <ChevronRight size={18} />
              </button>
              <button className="add-method-card" onClick={() => void chooseImportFile()} type="button">
                <span className="method-icon"><FolderOpen size={22} /></span>
                <span><strong>{t("add.fileTitle")}</strong><small>{t("add.fileDescription")}</small></span>
                <ChevronRight size={18} />
              </button>
              <p className="dialog-footnote">{t("add.cockpitHelper")}</p>
              <button className="add-method-card" onClick={() => setAddMethod("api-key")} type="button">
                <span className="method-icon"><KeyRound size={22} /></span>
                <span><strong>{t("add.apiKeyTitle")}</strong><small>{t("add.apiKeyDescription")}</small></span>
                <ChevronRight size={18} />
              </button>
              <p className="dialog-footnote">{t("add.privateStorage")}</p>
            </div>
          ) : null}

          {addMethod === "migration" ? (
            <div className="migration-flow">
              <button className="back-link" onClick={() => setAddMethod("start")} type="button">{t("migration.back")}</button>
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
                                {candidate.plan_type ? ` · ${t("migration.plan", { plan: candidate.plan_type })}` : ""}
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
              {oauth.status.status === "pending" ? (
                <>
                  <div className="oauth-hero">
                    <LoaderCircle className="spin" size={26} />
                    <div><h3>{t("oauth.finishTitle")}</h3><p>{t("oauth.finishBody")}</p></div>
                  </div>
                  <label className="field-label" htmlFor="oauth-link">{t("oauth.link")}</label>
                  <div className="copy-field">
                    <input id="oauth-link" readOnly value={oauth.auth_url} />
                    <button aria-label={t("oauth.copyLink")} className="icon-button" onClick={() => void copyOAuthUrl()} type="button"><Copy size={17} /></button>
                  </div>
                  <div className="modal-actions">
                    <button className="button button-secondary" onClick={() => void cancelOAuth()} type="button">{t("oauth.cancel")}</button>
                    <button className="button button-primary" onClick={() => void api.openOAuth(oauth.login_id)} type="button"><Globe2 size={16} />{t("oauth.openBrowser")}</button>
                  </div>
                </>
              ) : oauth.status.status === "complete" ? (
                <div className="outcome-panel">
                  <CircleCheck size={26} />
                  <h3>{t("oauth.added", { name: oauth.status.account.label })}</h3>
                  <button className="button button-primary" onClick={closeAddDialog} type="button">{t("common.done")}</button>
                </div>
              ) : (
                <div className="outcome-panel">
                  <CircleAlert size={26} />
                  <h3>{oauth.status.status === "cancelled" ? t("oauth.cancelled") : t("oauth.incomplete")}</h3>
                  <p>{oauth.status.status === "failed" ? t("oauth.retryBody") : t("oauth.unchanged")}</p>
                  <button className="button button-primary" onClick={() => void startOAuth()} type="button">{t("oauth.retry")}</button>
                </div>
              )}
            </div>
          ) : null}

          {addMethod === "json" ? (
            <form className="credential-form" onSubmit={submitJson}>
              <button className="back-link" onClick={() => setAddMethod("start")} type="button">{t("form.allMethods")}</button>
              <h3>{t("form.pasteTitle")}</h3>
              <p>{t("form.pasteBody")}</p>
              <label className="field-label" htmlFor="account-label">{t("form.displayName")} <span>{t("common.optional")}</span></label>
              <input id="account-label" onChange={(event) => setLabel(event.target.value)} value={label} />
              <label className="field-label" htmlFor="auth-json">auth.json</label>
              <textarea id="auth-json" ref={jsonRef} required spellCheck={false} />
              <div className="modal-actions">
                <button className="button button-secondary" onClick={() => setAddMethod("start")} type="button">{t("common.cancel")}</button>
                <button className="button button-primary" disabled={busy !== null} type="submit">
                  {busy === "import-json" ? <LoaderCircle className="spin" size={16} /> : <FileJson size={16} />}
                  {t("common.addAccount")}
                </button>
              </div>
            </form>
          ) : null}

          {addMethod === "api-key" ? (
            <form className="credential-form" onSubmit={submitApiKey}>
              <button className="back-link" onClick={() => setAddMethod("start")} type="button">{t("form.allMethods")}</button>
              <h3>{t("form.apiKeyTitle")}</h3>
              <p>{t("form.apiKeyBody")}</p>
              <label className="field-label" htmlFor="api-label">{t("form.displayName")} <span>{t("common.optional")}</span></label>
              <input id="api-label" onChange={(event) => setLabel(event.target.value)} value={label} />
              <label className="field-label" htmlFor="api-key">{t("account.apiKey")}</label>
              <input autoComplete="off" id="api-key" ref={apiKeyRef} required spellCheck={false} type="password" />
              <div className="modal-actions">
                <button className="button button-secondary" onClick={() => setAddMethod("start")} type="button">{t("common.cancel")}</button>
                <button className="button button-primary" disabled={busy !== null} type="submit">
                  {busy === "import-key" ? <LoaderCircle className="spin" size={16} /> : <KeyRound size={16} />}
                  {t("common.addAccount")}
                </button>
              </div>
            </form>
          ) : null}
        </Modal>
      ) : null}

      {dialog === "settings" ? (
        <Modal onClose={() => setDialog(null)} t={t} title={t("settings.title")}>
          <div className="settings-list">
            <div className="settings-language">
              <label htmlFor="language-preference">{t("settings.language")}</label>
              <select
                id="language-preference"
                onChange={(event) => changeLanguage(event.target.value as LanguagePreference)}
                value={languagePreference}
              >
                <option value="system">{t("settings.system")}</option>
                <option value="en">{t("settings.english")}</option>
                <option value="zh-CN">{t("settings.chinese")}</option>
              </select>
            </div>
            <div><span>{t("settings.credentialStore")}</span><strong>{runtime ? credentialStoreLabel(runtime.credential_store, t) : t("settings.checking")}</strong></div>
            <div><span>{t("settings.accountRecords")}</span><strong>{storage?.status === "recovery_required" ? t("settings.recoveryRequired") : t("settings.storedLocally")}</strong></div>
            <p>{t("settings.secretBoundary")}</p>
          </div>
        </Modal>
      ) : null}

      {dialog === "storage-recovery" ? (
        <Modal
          onClose={() => {
            setStorageResetConfirmation(false);
            setDialog(null);
          }}
          t={t}
          title={t("recovery.storageTitle")}
        >
          <div className="confirm-panel">
            <ShieldAlert size={26} />
            <h3>{t("recovery.keepCodex")}</h3>
            <p>{t("recovery.storageBody")}</p>
            <p>{t("recovery.storageExplanation")}</p>
            {storageResetConfirmation ? (
              <div className="confirm-copy">
                <strong>{t("recovery.resetQuestion")}</strong>
                <p>{t("recovery.resetExplanation")}</p>
              </div>
            ) : null}
            <div className="modal-actions">
              <button
                className="button button-secondary"
                onClick={() => {
                  setStorageResetConfirmation(false);
                  setDialog(null);
                }}
                type="button"
              >
                {t("common.cancel")}
              </button>
              <button className="button button-danger" disabled={busy !== null} onClick={() => void resetDamagedAccountStore()} type="button">
                {busy === "reset-damaged-store" ? <LoaderCircle className="spin" size={16} /> : <ShieldAlert size={16} />}
                {storageResetConfirmation ? t("recovery.resetStorage") : t("common.continue")}
              </button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "enable-switching" ? (
        <Modal onClose={() => setDialog(null)} t={t} title={t("switching.title")}>
          <div className="confirm-panel">
            <ShieldAlert size={26} />
            <h3>{t("switching.prepare")}</h3>
            <p>{t("switching.body")}</p>
            <div className="modal-actions">
              <button className="button button-secondary" onClick={() => setDialog(null)} type="button">{t("common.cancel")}</button>
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
        <Modal onClose={() => setDialog(null)} t={t} title={t("recovery.switchTitle")}>
          <div className="confirm-panel">
            <CircleAlert size={26} />
            <h3>{t("recovery.switchHeading")}</h3>
            <p>{t("recovery.switchBody")}</p>
            <div className="modal-actions">
              <button className="button button-secondary" onClick={() => setDialog(null)} type="button">{t("common.cancel")}</button>
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
        <Modal onClose={() => setDialog(null)} t={t} title={t("recovery.resetTitle")}>
          <div className="confirm-panel">
            <CircleAlert size={26} />
            <h3>{t("recovery.resetHeading")}</h3>
            <p>{t("recovery.resetBody")}</p>
            <p>{t("recovery.pendingResetExplanation")}</p>
            <div className="modal-actions">
              <button className="button button-secondary" onClick={() => setDialog(null)} type="button">{t("common.cancel")}</button>
              <button
                className="button button-primary"
                disabled={busy !== null}
                onClick={() => void recoverPendingResetCredit()}
                type="button"
              >
                {busy === "recover-reset-credit" ? <LoaderCircle className="spin" size={16} /> : <RefreshCw size={16} />}
                {t("recovery.originalRequest")}
              </button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "reset" && resetAccount ? (
        <Modal
          onClose={() => {
            setResetConfirmation(false);
            setDialog(null);
          }}
          t={t}
          title={t("reset.title", { name: resetAccount.label })}
        >
          <div className="reset-details">
            <p>
              {t("reset.available", { count: formatNumber(resetCredits?.available_count ?? 0, locale.formatLocale) })}
            </p>
            {resetCredits?.details_available ? (
              <ul className="credit-list">
                {resetCredits.usable_credits.map((credit, index) => (
                  <li key={String(credit.expires_at ?? "unknown") + "-" + index}>
                    <span>{t("reset.eligible", { number: formatNumber(index + 1, locale.formatLocale) })}</span>
                    <strong>{credit.expires_at ? t("reset.expires", { time: formatDateTimeWithRelative(credit.expires_at, locale.formatLocale) }) : t("reset.expiryUnknown")}</strong>
                  </li>
                ))}
              </ul>
            ) : (
              <div className="inline-warning"><CircleAlert size={17} />{t("reset.detailsUnavailable")}</div>
            )}
            {resetConfirmation ? (
              <div className="confirm-copy"><strong>{t("reset.question")}</strong><p>{t("reset.warning")}</p></div>
            ) : null}
            <div className="modal-actions">
              <button className="button button-secondary" onClick={() => setDialog(null)} type="button">{t("common.cancel")}</button>
              <button className="button button-danger" disabled={!resetCredits?.can_redeem || busy !== null} onClick={() => void redeemReset()} type="button">
                {busy?.startsWith("reset:") ? <LoaderCircle className="spin" size={16} /> : <Zap size={16} />}
                {resetConfirmation ? t("reset.useEarliest") : t("reset.use")}
              </button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "remove" && removeAccount ? (
        <Modal onClose={() => setDialog(null)} t={t} title={t("remove.title", { name: removeAccount.label })}>
          <div className="confirm-panel">
            <Trash2 size={26} />
            <h3>{t("remove.heading")}</h3>
            <p>{t("remove.body")}</p>
            <div className="modal-actions">
              <button className="button button-secondary" onClick={() => setDialog(null)} type="button">{t("common.cancel")}</button>
              <button className="button button-danger" disabled={busy !== null} onClick={() => void removeSavedAccount()} type="button"><Trash2 size={16} />{t("common.removeAccount")}</button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "wake" && wake ? (
        <Modal onClose={() => setDialog(null)} t={t} title={t("wake.title")}>
          <div className="wake-panel">
            <div className="wake-status">
              {wake.status === "running" ? <LoaderCircle className="spin" size={21} /> : <CircleCheck size={21} />}
              <div>
                <strong>{wake.status === "running" ? t("wake.running") : t("wake.status", { status: wakeStatusLabel(wake.status, t) })}</strong>
                <p>{wake.current_account_id ? t("wake.oneProcessing") : t("wake.perAccount")}</p>
              </div>
            </div>
            <ul className="wake-results">
              {wake.results.map((result, index) => (
                <li key={result.account_id + "-" + result.result + "-" + index}>
                  <span className={"wake-result-dot wake-" + result.result} />
                  <div>
                    <strong>{result.label}</strong>
                    <p>{wakeResultLabel(result.result, t)}</p>
                    <p className="wake-result-detail">{result.message}</p>
                  </div>
                </li>
              ))}
              {wake.status === "running" && wake.results.length === 0 ? <li className="wake-empty">{t("wake.preparing")}</li> : null}
            </ul>
            <div className="modal-actions">
              {wake.status === "running" ? (
                <button className="button button-secondary" onClick={() => void runVoidTask("cancel-wake", () => api.cancelWake(wake.id), false)} type="button">{t("wake.cancelRemaining")}</button>
              ) : null}
              <button className="button button-primary" onClick={() => setDialog(null)} type="button">{t("common.done")}</button>
            </div>
          </div>
        </Modal>
      ) : null}
    </main>
  );
}
