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

import { api } from "./api";
import type {
  AccountView,
  LiveAccountView,
  OAuthLoginStart,
  OAuthLoginStatus,
  QuotaView,
  QuotaWindow,
  RuntimeInfo,
  WakeOperationView,
} from "./types";

type Dialog =
  | "add"
  | "settings"
  | "enable-switching"
  | "recover-switch"
  | "reset"
  | "remove"
  | "wake"
  | null;

type AddMethod = "start" | "oauth" | "json" | "api-key";

interface Notice {
  kind: "success" | "error" | "info";
  text: string;
}

interface OAuthFlow extends OAuthLoginStart {
  status: OAuthLoginStatus;
}

const FILE_FILTERS = [{ name: "Account exports", extensions: ["json"] }];

function friendlyError(error: unknown, fallback = "The action did not complete.") {
  const message = String(error);
  if (/Codex is running|external Codex/i.test(message)) {
    return "GSwitch did not make a change. Quit the other Codex session, then try again.";
  }
  if (/recovery/i.test(message)) {
    return "GSwitch did not make a change. Resolve the protected recovery step before continuing.";
  }
  if (/file-backed|file store|required/i.test(message)) {
    return "GSwitch did not make a change. Enable file-backed switching before trying again.";
  }
  if (/No supported|not valid JSON|Invalid auth/i.test(message)) {
    return "That account file could not be imported. Choose a complete supported export and try again.";
  }
  return fallback + " Your current Codex account was not changed.";
}

function formatDate(timestamp?: number) {
  if (!timestamp) {
    return "Unknown";
  }
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(timestamp * 1000));
}

function primaryWindow(quota: QuotaView | undefined, kind: "five_hour" | "weekly") {
  return quota?.snapshot?.buckets
    .filter((bucket) => bucket.kind === "codex")
    .flatMap((bucket) => bucket.windows)
    .find((window) => window.kind === kind);
}

function accountPlan(account: AccountView) {
  return account.kind === "api_key" ? "API key" : account.plan_type || "ChatGPT";
}

function Modal({
  title,
  children,
  onClose,
  wide = false,
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
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
          <button aria-label={"Close " + title} className="icon-button" onClick={onClose} type="button">
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
}: {
  label: string;
  window?: QuotaWindow;
  status?: QuotaView["status"];
}) {
  const known = status === "fresh" || status === "stale";
  const remaining = window?.remaining_percent;
  const value = typeof remaining === "number" ? Math.max(0, Math.min(100, remaining)) : undefined;

  return (
    <div className="quota-meter">
      <div className="quota-heading">
        <span>{label}</span>
        <strong>{value === undefined || !known ? "—" : Math.round(value) + "%"}</strong>
      </div>
      <div
        aria-label={label + " remaining"}
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
          ? "Not available"
          : status === "stale"
            ? "Last result is stale"
            : window?.resets_at
              ? "Resets " + formatDate(window.resets_at)
              : "Reset time unavailable"}
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
  onRemove,
}: {
  account: AccountView;
  quota?: QuotaView;
  active: boolean;
  busy: boolean;
  onSwitch: () => void;
  onWake: () => void;
  onRefresh: () => void;
  onReset: () => void;
  onRemove: () => void;
}) {
  const credits = quota?.snapshot?.reset_credits;
  const isApiKey = account.kind === "api_key";

  return (
    <article className={"account-card" + (active ? " account-active" : "")}>
      <div className="account-card-head">
        <div className="account-identity">
          <div className="account-avatar" aria-hidden="true">
            {account.label.slice(0, 1).toUpperCase()}
          </div>
          <div className="account-copy">
            <h3>{account.label}</h3>
            <p>{account.email || (isApiKey ? "Stored locally" : "Verified Codex account")}</p>
          </div>
        </div>
        <details className="card-menu">
          <summary aria-label={"More actions for " + account.label}>
            <MoreHorizontal size={18} />
          </summary>
          <div className="card-menu-popover">
            <button disabled={active || busy} onClick={onRemove} type="button">
              <Trash2 size={15} />
              Remove account
            </button>
          </div>
        </details>
      </div>

      <div className="account-badges">
        {active ? <span className="badge badge-active"><Check size={13} /> Active</span> : null}
        <span className="badge">{accountPlan(account)}</span>
        {quota?.status === "stale" ? <span className="badge badge-muted">Stale</span> : null}
      </div>

      {isApiKey ? (
        <div className="api-key-state">
          <KeyRound size={18} />
          <div>
            <strong>Saved without a billable check</strong>
            <p>Subscription quota, reset credits, and Wake do not apply to API-key accounts.</p>
          </div>
        </div>
      ) : (
        <>
          <div className="quota-pair">
            <QuotaMeter label="5-hour" status={quota?.status} window={primaryWindow(quota, "five_hour")} />
            <QuotaMeter label="Weekly" status={quota?.status} window={primaryWindow(quota, "weekly")} />
          </div>
          <div className="credit-row">
            <span>
              Reset credits
              <strong>{credits ? credits.available_count : "—"}</strong>
            </span>
            {credits?.nearest_expiry ? <small>Earliest {formatDate(credits.nearest_expiry)}</small> : null}
            {credits?.available_count && credits.available_count > 0 ? (
              <button className="text-button" onClick={onReset} type="button">
                Details
                <ChevronRight size={15} />
              </button>
            ) : null}
          </div>
        </>
      )}

      <div className="card-footer">
        <button
          aria-label={"Refresh " + account.label}
          className="icon-button"
          disabled={busy || isApiKey}
          onClick={onRefresh}
          type="button"
        >
          <RefreshCw className={busy ? "spin" : ""} size={17} />
        </button>
        <div className="card-footer-actions">
          {!isApiKey ? (
            <button className="button button-secondary" disabled={busy} onClick={onWake} type="button">
              <Zap size={15} />
              Wake
            </button>
          ) : null}
          <button className="button button-primary" disabled={busy || active} onClick={onSwitch} type="button">
            {busy ? <LoaderCircle className="spin" size={15} /> : <ArrowRightLeft size={15} />}
            {active ? "Current" : "Switch"}
          </button>
        </div>
      </div>
    </article>
  );
}

function FirstRun({ onImport, onAdd }: { onImport: () => void; onAdd: () => void }) {
  return (
    <section className="first-run">
      <div className="first-run-primary">
        <span className="eyebrow">GET STARTED</span>
        <h2>Bring your Codex accounts into one calm workspace.</h2>
        <p>
          Import an export you choose, or add an account directly. GSwitch never reads another
          app&apos;s private account storage.
        </p>
        <div className="first-run-actions">
          <button className="button button-primary" onClick={onImport} type="button">
            <Upload size={16} />
            Import an export
          </button>
          <button className="button button-secondary" onClick={onAdd} type="button">
            <Plus size={16} />
            Add manually
          </button>
        </div>
      </div>
      <div className="first-run-methods">
        <div><Globe2 size={18} /><span>Browser sign-in</span></div>
        <div><FileJson size={18} /><span>Codex auth.json or supported export</span></div>
        <div><KeyRound size={18} /><span>API key</span></div>
      </div>
    </section>
  );
}

export default function App() {
  const [accounts, setAccounts] = useState<AccountView[]>([]);
  const [quotas, setQuotas] = useState<Record<string, QuotaView>>({});
  const [live, setLive] = useState<LiveAccountView | null>(null);
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [addMethod, setAddMethod] = useState<AddMethod>("start");
  const [notice, setNotice] = useState<Notice | null>(null);
  const [oauth, setOauth] = useState<OAuthFlow | null>(null);
  const [wake, setWake] = useState<WakeOperationView | null>(null);
  const [resetAccount, setResetAccount] = useState<AccountView | null>(null);
  const [removeAccount, setRemoveAccount] = useState<AccountView | null>(null);
  const [resetConfirmation, setResetConfirmation] = useState(false);
  const jsonRef = useRef<HTMLTextAreaElement>(null);
  const apiKeyRef = useRef<HTMLInputElement>(null);
  const [label, setLabel] = useState("");

  const loadSnapshot = useCallback(async () => {
    setLoading(true);
    try {
      const initial = await Promise.all([api.listAccounts(), api.liveAccount(), api.runtimeInfo()]);
      const nextAccounts = initial[0];
      const quotaPairs = await Promise.all(
        nextAccounts
          .filter((account) => account.kind === "chat_gpt")
          .map(async (account) => [account.id, await api.accountQuota(account.id)] as const),
      );
      setAccounts(nextAccounts);
      setLive(initial[1]);
      setRuntime(initial[2]);
      setQuotas(Object.fromEntries(quotaPairs));
    } catch (error) {
      setNotice({
        kind: "error",
        text: friendlyError(error, "GSwitch could not read the current account state."),
      });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadSnapshot();
  }, [loadSnapshot]);

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
        setNotice({ kind: "error", text: friendlyError(error) });
        return undefined;
      } finally {
        setBusy(null);
      }
    },
    [loadSnapshot],
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
        setNotice({ kind: "error", text: friendlyError(error) });
        return false;
      } finally {
        setBusy(null);
      }
    },
    [loadSnapshot],
  );

  const importPath = useCallback(
    async (path: string) => {
      const result = await runTask("import", () => api.importAuthFile(path));
      if (result) {
        const suffix = result.imported.length === 1 ? "" : "s";
        const skippedSuffix = result.skipped_count
          ? " " + result.skipped_count + " unsupported item" + (result.skipped_count === 1 ? "" : "s") + " skipped."
          : "";
        setNotice({
          kind: "success",
          text: String(result.imported.length) + " account" + suffix + " imported safely." + skippedSuffix,
        });
        setDialog(null);
      }
    },
    [runTask],
  );

  const chooseImportFile = useCallback(async () => {
    try {
      const selected = await open({
        title: "Choose a Codex account export",
        filters: FILE_FILTERS,
        multiple: false,
      });
      if (typeof selected === "string") {
        await importPath(selected);
      }
    } catch (error) {
      setNotice({ kind: "error", text: friendlyError(error, "GSwitch could not open the file picker.") });
    }
  }, [importPath]);

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
        const path = event.payload.paths[0];
        if (path) {
          void importPath(path);
        }
      })
      .then((stop) => {
        unlisten = stop;
      })
      .catch(() => {
        // File selection remains available if drag-and-drop cannot register.
      });
    return () => unlisten?.();
  }, [importPath]);

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
          setNotice({ kind: "success", text: status.account.label + " was added safely." });
          setDialog(null);
          void loadSnapshot();
        } else if (status.status === "pending") {
          timer = window.setTimeout(poll, 800);
        }
      } catch {
        if (!closed) {
          setOauth((current) =>
            current?.login_id === oauth.login_id
              ? { ...current, status: { status: "failed", message: "Unable to read sign-in result." } }
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
  }, [loadSnapshot, oauth]);

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
          setNotice({ kind: "error", text: "Wake stopped before GSwitch could read its final result." });
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
  }, [loadSnapshot, wake]);

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
      setNotice({ kind: "info", text: "Your sign-in link is ready. Copy it and open it in your preferred browser." });
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

  const copyOAuthUrl = async () => {
    if (!oauth) {
      return;
    }
    try {
      await navigator.clipboard.writeText(oauth.auth_url);
      setNotice({ kind: "success", text: "Sign-in link copied." });
    } catch {
      setNotice({ kind: "info", text: "Select the sign-in link below and copy it manually." });
    }
  };

  const submitJson = async (event: FormEvent) => {
    event.preventDefault();
    const rawJson = jsonRef.current?.value ?? "";
    if (jsonRef.current) {
      jsonRef.current.value = "";
    }
    if (!rawJson.trim()) {
      setNotice({ kind: "error", text: "Paste a complete auth JSON document before adding it." });
      return;
    }
    const result = await runTask("import-json", () => api.importAuthJson(rawJson, label || undefined));
    if (result) {
      setLabel("");
      setDialog(null);
      setNotice({ kind: "success", text: result.label + " was imported safely." });
    }
  };

  const submitApiKey = async (event: FormEvent) => {
    event.preventDefault();
    const apiKey = apiKeyRef.current?.value ?? "";
    if (apiKeyRef.current) {
      apiKeyRef.current.value = "";
    }
    if (!apiKey.trim()) {
      setNotice({ kind: "error", text: "Enter an API key before adding it." });
      return;
    }
    const result = await runTask("import-key", () => api.importApiKey(apiKey, label || undefined));
    if (result) {
      setLabel("");
      setDialog(null);
      setNotice({ kind: "success", text: result.label + " was saved without a billable check." });
    }
  };

  const refreshAll = async () => {
    await runVoidTask(
      "refresh-all",
      async () => {
        for (const account of accounts) {
          if (account.kind === "chat_gpt") {
            await api.refreshAccountQuota(account.id);
          }
        }
      },
      true,
    );
  };

  const switchAccount = async (account: AccountView) => {
    const result = await runTask("switch:" + account.id, () => api.switchAccount(account.id));
    if (result) {
      setNotice({ kind: "success", text: account.label + " is now the active Codex account." });
    }
  };

  const refreshAccount = async (account: AccountView) => {
    const result = await runTask("refresh:" + account.id, () => api.refreshAccountQuota(account.id));
    if (result) {
      setNotice({ kind: "success", text: account.label + " quota was refreshed." });
    }
  };

  const startWake = async (accountId?: string, model?: string) => {
    const key = accountId ? "wake:" + accountId : "wake-all";
    const result = await runTask(
      key,
      () => accountId ? api.startWake(accountId, model) : api.startWakeAll(),
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
          ? result.refresh_warning || "The earliest eligible reset credit was used."
          : "No reset credit was used.",
      });
      setResetAccount(null);
      setResetConfirmation(false);
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
      setNotice({ kind: "success", text: removeAccount.label + " was removed from GSwitch." });
      setRemoveAccount(null);
      setDialog(null);
    }
  };

  const currentActiveId = live?.account?.id;
  const liveAccountLabel = live?.account?.label || "Current Codex account";
  const chatGptAccounts = accounts.filter((account) => account.kind === "chat_gpt");
  const resetCredits = resetAccount ? quotas[resetAccount.id]?.snapshot?.reset_credits : undefined;

  const safetyNotice = useMemo(() => {
    if (!live) {
      return null;
    }
    if (live.status === "unknown_account") {
      return {
        icon: <ShieldAlert size={20} />,
        title: "This Codex account is not saved",
        body: "Save it before switching so its current credentials are never overwritten.",
        action: "Save current account",
        onAction: () =>
          void runTask("save-current", api.saveCurrentAccount).then((account) => {
            if (account) {
              setNotice({ kind: "success", text: account.label + " was saved safely." });
            }
          }),
      };
    }
    if (live.status === "file_store_required") {
      return {
        icon: <ShieldAlert size={20} />,
        title: "Switching needs file-backed Codex credentials",
        body: "GSwitch will not try to extract credentials from keychain, auto, or ephemeral storage.",
        action: "Enable account switching",
        onAction: () => setDialog("enable-switching"),
      };
    }
    if (live.status === "recovery_required") {
      return {
        icon: <CircleAlert size={20} />,
        title: "A protected switch needs recovery",
        body: "No credential change will run until the previous switch is safely resolved.",
        action: "Review recovery",
        onAction: () => setDialog("recover-switch"),
      };
    }
    return null;
  }, [live, runTask]);

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
          <button className="button button-quiet" disabled={loading || busy !== null} onClick={() => void refreshAll()} type="button">
            <RefreshCw className={busy === "refresh-all" ? "spin" : ""} size={16} />
            Refresh
          </button>
          <button
            className="button button-secondary"
            disabled={loading || busy !== null || chatGptAccounts.length === 0}
            onClick={() => void startWake()}
            type="button"
          >
            <Zap size={16} />
            Wake all
          </button>
          <button
            className="button button-primary"
            disabled={loading || busy !== null}
            onClick={() => {
              setOauth(null);
              setAddMethod("start");
              setDialog("add");
            }}
            type="button"
          >
            <Plus size={16} />
            Add account
          </button>
          <button aria-label="Open settings" className="icon-button" onClick={() => setDialog("settings")} type="button">
            <Settings size={18} />
          </button>
        </div>
      </header>

      <div className="content">
        {notice ? (
          <div className={"toast toast-" + notice.kind} role="status">
            {notice.kind === "success" ? <CircleCheck size={17} /> : <CircleAlert size={17} />}
            <span>{notice.text}</span>
            <button aria-label="Dismiss message" onClick={() => setNotice(null)} type="button"><X size={15} /></button>
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
              <span className="eyebrow">ACCOUNTS</span>
              <h2 id="accounts-heading">
                {loading ? "Loading workspace" : String(accounts.length) + " saved account" + (accounts.length === 1 ? "" : "s")}
              </h2>
            </div>
            <p>Switch an account only when you are ready to change Codex.</p>
          </div>

          {loading ? (
            <div className="loading-state"><LoaderCircle className="spin" size={24} />Reading local account state…</div>
          ) : accounts.length ? (
            <div className="account-grid">
              {accounts.map((account) => (
                <AccountCard
                  account={account}
                  active={account.active || account.id === currentActiveId}
                  busy={busy !== null}
                  key={account.id}
                  onRefresh={() => void refreshAccount(account)}
                  onRemove={() => {
                    setRemoveAccount(account);
                    setDialog("remove");
                  }}
                  onReset={() => {
                    setResetAccount(account);
                    setResetConfirmation(false);
                    setDialog("reset");
                  }}
                  onSwitch={() => void switchAccount(account)}
                  onWake={() => void startWake(account.id)}
                  quota={quotas[account.id]}
                />
              ))}
            </div>
          ) : (
            <FirstRun
              onAdd={() => {
                setOauth(null);
                setAddMethod("start");
                setDialog("add");
              }}
              onImport={() => void chooseImportFile()}
            />
          )}
        </section>
      </div>

      {dialog === "add" ? (
        <Modal onClose={closeAddDialog} title="Add a Codex account" wide>
          {addMethod === "start" ? (
            <div className="add-methods">
              <button className="add-method-card" onClick={() => void startOAuth()} type="button">
                <span className="method-icon"><Globe2 size={22} /></span>
                <span><strong>Sign in with your browser</strong><small>Use the official Codex sign-in flow.</small></span>
                <ChevronRight size={18} />
              </button>
              <button className="add-method-card" onClick={() => setAddMethod("json")} type="button">
                <span className="method-icon"><FileJson size={22} /></span>
                <span><strong>Paste auth JSON</strong><small>Submit a complete Codex auth document.</small></span>
                <ChevronRight size={18} />
              </button>
              <button className="add-method-card" onClick={() => void chooseImportFile()} type="button">
                <span className="method-icon"><FolderOpen size={22} /></span>
                <span><strong>Choose an export file</strong><small>Codex auth.json, Cockpit, Sub2API, or CPA export.</small></span>
                <ChevronRight size={18} />
              </button>
              <button className="add-method-card" onClick={() => setAddMethod("api-key")} type="button">
                <span className="method-icon"><KeyRound size={22} /></span>
                <span><strong>Add an API key</strong><small>Saved without a billable validation request.</small></span>
                <ChevronRight size={18} />
              </button>
              <p className="dialog-footnote">
                GSwitch reads only the file you select. It does not access Cockpit Tools&apos; private storage.
              </p>
            </div>
          ) : null}

          {addMethod === "oauth" && oauth ? (
            <div className="oauth-flow">
              {oauth.status.status === "pending" ? (
                <>
                  <div className="oauth-hero">
                    <LoaderCircle className="spin" size={26} />
                    <div><h3>Finish sign-in in your browser</h3><p>GSwitch adds the verified account when Codex confirms the sign-in.</p></div>
                  </div>
                  <label className="field-label" htmlFor="oauth-link">Sign-in link</label>
                  <div className="copy-field">
                    <input id="oauth-link" readOnly value={oauth.auth_url} />
                    <button aria-label="Copy sign-in link" className="icon-button" onClick={() => void copyOAuthUrl()} type="button"><Copy size={17} /></button>
                  </div>
                  <div className="modal-actions">
                    <button className="button button-secondary" onClick={() => void cancelOAuth()} type="button">Cancel sign-in</button>
                    <button className="button button-primary" onClick={() => void api.openOAuth(oauth.login_id)} type="button"><Globe2 size={16} />Open browser</button>
                  </div>
                </>
              ) : oauth.status.status === "complete" ? (
                <div className="outcome-panel">
                  <CircleCheck size={26} />
                  <h3>{oauth.status.account.label} was added</h3>
                  <button className="button button-primary" onClick={closeAddDialog} type="button">Done</button>
                </div>
              ) : (
                <div className="outcome-panel">
                  <CircleAlert size={26} />
                  <h3>{oauth.status.status === "cancelled" ? "Sign-in cancelled" : "Sign-in did not complete"}</h3>
                  <p>{oauth.status.status === "failed" ? "Try the official browser sign-in again." : "No account was changed."}</p>
                  <button className="button button-primary" onClick={() => void startOAuth()} type="button">Try again</button>
                </div>
              )}
            </div>
          ) : null}

          {addMethod === "json" ? (
            <form className="credential-form" onSubmit={submitJson}>
              <button className="back-link" onClick={() => setAddMethod("start")} type="button">← All methods</button>
              <h3>Paste a complete auth document</h3>
              <p>It is submitted directly to Rust and cleared from this field immediately.</p>
              <label className="field-label" htmlFor="account-label">Display name <span>optional</span></label>
              <input id="account-label" onChange={(event) => setLabel(event.target.value)} value={label} />
              <label className="field-label" htmlFor="auth-json">auth.json</label>
              <textarea id="auth-json" ref={jsonRef} required spellCheck={false} />
              <div className="modal-actions">
                <button className="button button-secondary" onClick={() => setAddMethod("start")} type="button">Cancel</button>
                <button className="button button-primary" disabled={busy !== null} type="submit">
                  {busy === "import-json" ? <LoaderCircle className="spin" size={16} /> : <FileJson size={16} />}
                  Add account
                </button>
              </div>
            </form>
          ) : null}

          {addMethod === "api-key" ? (
            <form className="credential-form" onSubmit={submitApiKey}>
              <button className="back-link" onClick={() => setAddMethod("start")} type="button">← All methods</button>
              <h3>Add an API-key account</h3>
              <p>GSwitch does not use the key for an unrequested billable validation call.</p>
              <label className="field-label" htmlFor="api-label">Display name <span>optional</span></label>
              <input id="api-label" onChange={(event) => setLabel(event.target.value)} value={label} />
              <label className="field-label" htmlFor="api-key">API key</label>
              <input autoComplete="off" id="api-key" ref={apiKeyRef} required spellCheck={false} type="password" />
              <div className="modal-actions">
                <button className="button button-secondary" onClick={() => setAddMethod("start")} type="button">Cancel</button>
                <button className="button button-primary" disabled={busy !== null} type="submit">
                  {busy === "import-key" ? <LoaderCircle className="spin" size={16} /> : <KeyRound size={16} />}
                  Add account
                </button>
              </div>
            </form>
          ) : null}
        </Modal>
      ) : null}

      {dialog === "settings" ? (
        <Modal onClose={() => setDialog(null)} title="Settings">
          <div className="settings-list">
            <div><span>Credential store</span><strong>{runtime?.credential_store || "Checking…"}</strong></div>
            <div><span>Account records</span><strong>Stored locally</strong></div>
            <p>Credentials, provider responses, and reset-credit identifiers never enter this window.</p>
          </div>
        </Modal>
      ) : null}

      {dialog === "enable-switching" ? (
        <Modal onClose={() => setDialog(null)} title="Enable account switching">
          <div className="confirm-panel">
            <ShieldAlert size={26} />
            <h3>Prepare file-backed credentials</h3>
            <p>GSwitch asks Codex to use its supported file-backed credential store. It will not extract credentials from a keychain or private store.</p>
            <div className="modal-actions">
              <button className="button button-secondary" onClick={() => setDialog(null)} type="button">Cancel</button>
              <button
                className="button button-primary"
                disabled={busy !== null}
                onClick={() =>
                  void runTask("enable-switching", api.enableAccountSwitching).then((enabled) => {
                    if (enabled) {
                      setDialog(null);
                      setNotice({ kind: "success", text: "Account switching is ready." });
                    }
                  })
                }
                type="button"
              >
                Enable switching
              </button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "recover-switch" ? (
        <Modal onClose={() => setDialog(null)} title="Recover protected switch">
          <div className="confirm-panel">
            <CircleAlert size={26} />
            <h3>Resolve the incomplete switch first</h3>
            <p>GSwitch restores credentials only when the live file still matches its own previous write. If another program changed it, recovery stops safely.</p>
            <div className="modal-actions">
              <button className="button button-secondary" onClick={() => setDialog(null)} type="button">Cancel</button>
              <button
                className="button button-primary"
                disabled={busy !== null}
                onClick={() =>
                  void runVoidTask("recover-switch", api.recoverPendingSwitch).then((recovered) => {
                    if (recovered) {
                      setDialog(null);
                      setNotice({ kind: "success", text: "The protected switch was recovered." });
                    }
                  })
                }
                type="button"
              >
                Recover safely
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
          title={"Reset credits · " + resetAccount.label}
        >
          <div className="reset-details">
            <p>
              {resetCredits?.available_count ?? 0} available. GSwitch refreshes this account, selects the earliest eligible credit in Rust, and never exposes a credit ID here.
            </p>
            {resetCredits?.details_available ? (
              <ul className="credit-list">
                {resetCredits.usable_credits.map((credit, index) => (
                  <li key={String(credit.expires_at ?? "unknown") + "-" + index}>
                    <span>Eligible credit {index + 1}</span>
                    <strong>{credit.expires_at ? "Expires " + formatDate(credit.expires_at) : "Expiry unknown"}</strong>
                  </li>
                ))}
              </ul>
            ) : (
              <div className="inline-warning"><CircleAlert size={17} />Details are unavailable, so GSwitch cannot safely redeem a reset credit.</div>
            )}
            {resetConfirmation ? (
              <div className="confirm-copy"><strong>Use the earliest eligible reset credit?</strong><p>This consumes one provider credit and cannot be undone.</p></div>
            ) : null}
            <div className="modal-actions">
              <button className="button button-secondary" onClick={() => setDialog(null)} type="button">Cancel</button>
              <button className="button button-danger" disabled={!resetCredits?.can_redeem || busy !== null} onClick={() => void redeemReset()} type="button">
                {busy?.startsWith("reset:") ? <LoaderCircle className="spin" size={16} /> : <Zap size={16} />}
                {resetConfirmation ? "Use earliest credit" : "Use reset credit"}
              </button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "remove" && removeAccount ? (
        <Modal onClose={() => setDialog(null)} title={"Remove " + removeAccount.label + "?"}>
          <div className="confirm-panel">
            <Trash2 size={26} />
            <h3>Remove this saved account from GSwitch?</h3>
            <p>This does not log out or change the live Codex account.</p>
            <div className="modal-actions">
              <button className="button button-secondary" onClick={() => setDialog(null)} type="button">Cancel</button>
              <button className="button button-danger" disabled={busy !== null} onClick={() => void removeSavedAccount()} type="button"><Trash2 size={16} />Remove account</button>
            </div>
          </div>
        </Modal>
      ) : null}

      {dialog === "wake" && wake ? (
        <Modal onClose={() => setDialog(null)} title="Wake">
          <div className="wake-panel">
            <div className="wake-status">
              {wake.status === "running" ? <LoaderCircle className="spin" size={21} /> : <CircleCheck size={21} />}
              <div>
                <strong>{wake.status === "running" ? "Wake is running safely" : "Wake " + wake.status}</strong>
                <p>{wake.current_account_id ? "One isolated account is being processed. The live Codex credential is untouched." : "Each result is shown separately; failed or skipped accounts do not stop the rest."}</p>
              </div>
            </div>
            <ul className="wake-results">
              {wake.results.map((result, index) => (
                <li key={result.account_id + "-" + result.result + "-" + index}>
                  <span className={"wake-result-dot wake-" + result.result} />
                  <div>
                    <strong>{result.label}</strong>
                    <p>{result.message}</p>
                    {result.result === "needs_model_selection" && result.available_models?.length ? (
                      <div className="model-choices">
                        {result.available_models.map((model) => (
                          <button className="button button-secondary" key={model} onClick={() => void startWake(result.account_id, model)} type="button">Use {model}</button>
                        ))}
                      </div>
                    ) : null}
                  </div>
                </li>
              ))}
              {wake.status === "running" && wake.results.length === 0 ? <li className="wake-empty">Preparing an isolated Codex session…</li> : null}
            </ul>
            <div className="modal-actions">
              {wake.status === "running" ? (
                <button className="button button-secondary" onClick={() => void runVoidTask("cancel-wake", () => api.cancelWake(wake.id), false)} type="button">Cancel remaining</button>
              ) : null}
              <button className="button button-primary" onClick={() => setDialog(null)} type="button">Done</button>
            </div>
          </div>
        </Modal>
      ) : null}
    </main>
  );
}
