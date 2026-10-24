import { AppWindow, ArrowRightLeft, Check, ChevronRight, CircleAlert, Copy, KeyRound, LoaderCircle, MoreHorizontal, RefreshCw, Trash2, Zap } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { formatCompactExpiry, formatDateTime, formatDateTimeWithRelative, formatNumber, formatPercent, formatQuotaResetTime, formatRelativeTime, type Translator } from "./i18n";
import type { AccountView, QuotaRefreshFailureCode, QuotaView, QuotaWindow } from "./types";
import { PopoverMenu } from "./PopoverMenu";
export type AccountBusyAction = "refresh" | "wake" | "switch";

export function accountPrimaryName(account: AccountView) {
  if (account.kind === "chat_gpt" && account.email?.trim()) {
    return account.email.trim();
  }
  return account.label;
}

export function accountSecondaryName(account: AccountView, primary: string, t: Translator) {
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

export function planLabel(plan: string, t: Translator) {
  const key = ({
    free: "plan.free",
    plus: "plan.plus",
    pro: "plan.pro",
    team: "plan.team",
  } as const)[plan.trim().toLowerCase() as "free" | "plus" | "pro" | "team"];
  return key ? t(key) : t("plan.other");
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

function QuotaResetTime({ timestamp, t, formatLocale }: {
  timestamp: number;
  t: Translator;
  formatLocale: string;
}) {
  const [nowMs, setNowMs] = useState(Date.now);
  useLayoutEffect(() => {
    setNowMs(Date.now());
    const interval = window.setInterval(() => setNowMs(Date.now()), 60_000);
    return () => window.clearInterval(interval);
  }, [timestamp]);

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

export function AccountCard({
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
            {!isApiKey ? <button disabled={controlsBusy} onClick={onReauthenticate} type="button"><AppWindow size={15} />{t("account.signInAgain")}</button> : null}
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
              disabled={globalBusy || (busyAction !== undefined && busyAction !== "refresh")}
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
            {busyAction === "switch" ? <LoaderCircle className="spin" size={15} /> : signInRequired ? <AppWindow size={15} /> : <ArrowRightLeft size={15} />}
            {signInRequired ? t("account.signInAgain") : active && account.needs_apply ? t("account.apply") : active ? t("common.current") : t("common.switch")}
          </button>
        </div>
      </div> : null}
    </article>
  );
}
