import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { AppWindow, Check, ChevronRight, CircleAlert, CircleCheck, Copy, FileJson, FolderOpen, KeyRound, LoaderCircle, RefreshCw, Upload } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "./api";
import { accountPrimaryName, planLabel } from "./AccountCard";
import { friendlyError, type Notice } from "./feedback";
import { formatNumber, type Translator } from "./i18n";
import { Modal } from "./Modal";
import type { AccountView, MigrationCandidate, MigrationPreview, OAuthFailureCode, OAuthLoginStart, OAuthLoginStatus } from "./types";

type AddMethod = "start" | "oauth" | "json" | "api-key" | "migration";
interface Input {
  t: Translator;
  formatLocale: string;
  notify: (notice: Notice) => void;
  onChanged: () => Promise<void>;
  onStartingSignIn: (id: string) => void;
  onAuthenticated: (account: AccountView, target: AccountView | null) => void;
  onShow: (visible: boolean) => void;
}
interface OAuthFlow extends OAuthLoginStart {
  status: OAuthLoginStatus;
}

function fileFilters(t: Translator) { return [{ name: t("file.accountExports"), extensions: ["json"] }]; }
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

interface AddAccountMethodsProps {
  busy: boolean;
  onSignIn: () => void;
  onImport: () => void;
  onSelectMethod: (method: "migration" | "json" | "api-key") => void;
  t: Translator;
}

export function AddAccountMethods({ busy, onSignIn, onImport, onSelectMethod, t }: AddAccountMethodsProps) {
  return (
    <div className="add-methods">
      <button className="add-method-card add-method-sign-in" disabled={busy} onClick={onSignIn} type="button">
        <span className="method-icon"><AppWindow size={22} /></span>
        <span><strong>{t("add.browserTitle")}</strong>{" "}<small>{t("add.browserDescription")}</small></span>
        <ChevronRight size={18} />
      </button>
      <button className="add-method-card" disabled={busy} onClick={() => onSelectMethod("migration")} type="button">
        <span className="method-icon"><Upload size={22} /></span>
        <span><strong>{t("add.localTitle")}</strong>{" "}<small>{t("add.localDescription")}</small></span>
        <ChevronRight size={18} />
      </button>
      <button className="add-method-card" disabled={busy} onClick={onImport} type="button">
        <span className="method-icon"><FolderOpen size={22} /></span>
        <span><strong>{t("add.fileTitle")}</strong>{" "}<small>{t("add.fileDescription")}</small></span>
        <ChevronRight size={18} />
      </button>
      <details className="add-other-methods">
        <summary>{t("add.otherMethods")}</summary>
        <div>
          <button className="add-method-card" disabled={busy} onClick={() => onSelectMethod("json")} type="button">
            <span className="method-icon"><FileJson size={22} /></span>
            <span><strong>{t("add.jsonTitle")}</strong>{" "}<small>{t("add.jsonDescription")}</small></span>
            <ChevronRight size={18} />
          </button>
          <button className="add-method-card" disabled={busy} onClick={() => onSelectMethod("api-key")} type="button">
            <span className="method-icon"><KeyRound size={22} /></span>
            <span><strong>{t("add.apiKeyTitle")}</strong>{" "}<small>{t("add.apiKeyDescription")}</small></span>
            <ChevronRight size={18} />
          </button>
        </div>
      </details>
    </div>
  );
}

export function FirstRun(props: AddAccountMethodsProps) {
  const { t } = props;
  return (
    <section className="first-run">
      <div className="first-run-primary">
        <span className="eyebrow">{t("firstRun.eyebrow")}</span>
        <h2>{t("firstRun.title")}</h2>
        <p>
          {t("firstRun.body")}
        </p>
        <AddAccountMethods {...props} />
      </div>
    </section>
  );
}

export function useAccountIntake(input: Input) {
  const { t, formatLocale } = input;
  const locale = { formatLocale };
  const inputs = useRef(input);
  useLayoutEffect(() => { inputs.current = input; });
  const [busy, setBusy] = useState<string | null>(null);
  const running = useRef(false);
  const setNotice = useCallback((notice: Notice) => inputs.current.notify(notice), []);
  const setDialog = useCallback((dialog: "add" | null) => inputs.current.onShow(dialog === "add"), []);
  const runTask = useCallback(async <T,>(key: string, task: () => Promise<T>, reload = true): Promise<T | undefined> => {
    if (running.current) return;
    running.current = true;
    setBusy(key);
    try {
      const result = await task();
      if (reload) await inputs.current.onChanged();
      return result;
    } catch (error) {
      setNotice({ kind: "error", text: friendlyError(inputs.current.t, error) });
    } finally { running.current = false; setBusy(null); }
  }, [setNotice]);
  const runVoidTask = useCallback(async (key: string, task: () => Promise<void>, reload = true) =>
    (await runTask(key, async () => { await task(); return true; }, reload)) === true, [runTask]);
  const [addMethod, setAddMethod] = useState<AddMethod>("start");
  const [migrationPreview, setMigrationPreview] = useState<MigrationPreview | null>(null);
  const [migrationRoot, setMigrationRoot] = useState<string | undefined>();
  const [migrationSelection, setMigrationSelection] = useState<string[]>([]);
  const [migrationScanned, setMigrationScanned] = useState(false);
  const [oauth, setOauth] = useState<OAuthFlow | null>(null);
  const [oauthStatusUnavailable, setOauthStatusUnavailable] = useState(false);
  const [oauthTarget, setOauthTarget] = useState<AccountView | null>(null);
  const [label, setLabel] = useState("");
  const jsonRef = useRef<HTMLTextAreaElement>(null);
  const apiKeyRef = useRef<HTMLInputElement>(null);
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
    let closed = false;
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
        if (closed) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // File selection remains available if drag-and-drop cannot register.
      });
    return () => { closed = true; unlisten?.(); };
  }, [importPaths]);

  const oauthLoginId = oauth?.login_id;
  const oauthPhase = oauth?.status.status;
  useEffect(() => {
    if (!oauthLoginId || (oauthPhase !== "pending" && oauthPhase !== "finishing")) {
      return;
    }
    let closed = false;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const status = await api.oauthStatus(oauthLoginId);
        if (closed) {
          return;
        }
        setOauthStatusUnavailable(false);
        setOauth((current) =>
          current?.login_id === oauthLoginId ? { ...current, status } : current,
        );
        if (status.status === "complete") {
          inputs.current.onAuthenticated(status.account, oauthTarget);
          if (!status.account.needs_apply) {
            setNotice({ kind: "success", text: t(oauthTarget ? "notice.reauthenticatedAccount" : "notice.importedAccount", { name: accountPrimaryName(status.account) }) });
          }
          void inputs.current.onChanged();
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
  }, [oauthLoginId, oauthPhase, oauthTarget, t]);

  const startOAuth = async (target?: AccountView) => {
    if (oauthPhase === "finishing") {
      setAddMethod("oauth"); setDialog("add"); return;
    }
    if (target) inputs.current.onStartingSignIn(target.id);
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
    if (oauth?.status.status === "pending") void api.cancelOAuth(oauth.login_id);
    setDialog(null);
  };

  const openAddDialog = (method: AddMethod = "start") => {
    if (oauthPhase === "finishing") {
      setAddMethod("oauth"); setDialog("add"); return;
    }
    setOauth(null);
    setOauthStatusUnavailable(false);
    setOauthTarget(null);
    setAddMethod(method);
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

  const openBrowser = async () => {
    if (!oauth) return;
    try { await api.openOAuth(oauth.login_id); }
    catch { setNotice({ kind: "error", text: t("oauth.openFailed") }); }
  };
  return { busy, addMethod, setAddMethod, migrationPreview, migrationRoot, migrationSelection, setMigrationSelection, migrationScanned, oauth, oauthTarget, oauthStatusUnavailable, label, setLabel, jsonRef, apiKeyRef,
    startOAuth, openAddDialog, closeAddDialog, chooseImportFile, chooseMigrationFolder, scanMigration, confirmMigration, cancelOAuth, retryOAuthSave, copyOAuthUrl, submitJson, submitApiKey, openBrowser };
}

export function AccountIntakeDialog({ flow, busy, onCopyEmail, t }: {
  flow: ReturnType<typeof useAccountIntake>; busy: string | null; onCopyEmail: (account: AccountView) => Promise<void>; t: Translator;
}) {
  const { addMethod, setAddMethod, migrationPreview, migrationRoot, migrationSelection, setMigrationSelection, migrationScanned, oauth, oauthTarget, oauthStatusUnavailable, label, setLabel, jsonRef, apiKeyRef,
    startOAuth, openAddDialog, closeAddDialog, chooseImportFile, chooseMigrationFolder, scanMigration, confirmMigration, cancelOAuth, retryOAuthSave, copyOAuthUrl, submitJson, submitApiKey } = flow;
  const copyAccountEmail = onCopyEmail;
  const runningAny = (...keys: string[]) => busy !== null && keys.includes(busy);
  return (
        <Modal dismissible={!runningAny("import", "migration-scan", "migration-import", "oauth", "oauth-cancel", "oauth-retry", "import-json", "import-key")} onClose={closeAddDialog} t={t} title={t(oauthTarget ? "oauth.reauthenticateTitle" : "add.title", oauthTarget ? { name: accountPrimaryName(oauthTarget) } : undefined)} wide>
          {addMethod === "start" ? (
            <AddAccountMethods
              busy={busy !== null}
              onSignIn={() => void startOAuth()}
              onImport={() => void chooseImportFile()}
              onSelectMethod={openAddDialog}
              t={t}
            />
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
                    <button className="button button-primary" onClick={() => void flow.openBrowser()} type="button"><AppWindow size={16} />{t("oauth.openBrowser")}</button>
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
  );
}
