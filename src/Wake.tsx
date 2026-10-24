import { CircleAlert, Info, LoaderCircle } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "./api";
import { accountPrimaryName, accountSecondaryName } from "./AccountCard";
import { friendlyError, type Notice } from "./feedback";
import type { Translator } from "./i18n";
import { Modal } from "./Modal";
import type { AccountView, WakeOperationView, WakeStart } from "./types";

interface Input { t: Translator; notify: (notice: Notice) => void; onChanged: () => Promise<void>; onRefreshed: (id: string) => void; onShow: (visible: boolean) => void }
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

export function useWake(input: Input) {
  const inputs = useRef(input);
  useLayoutEffect(() => { inputs.current = input; });
  const [wake, setWake] = useState<WakeOperationView | null>(null);
  const [wakeStatusUnavailable, setWakeStatusUnavailable] = useState(false);
  const [starting, setStarting] = useState(false);
  const [startingAccount, setStartingAccount] = useState<string>();
  const [cancelling, setCancelling] = useState(false);
  const running = useRef(false);
  const setDialog = useCallback((dialog: "wake" | null) => inputs.current.onShow(dialog === "wake"), []);
  const begin = async (task: () => Promise<WakeStart>, accountId?: string) => {
    if (running.current) return;
    running.current = true; setStarting(true); setStartingAccount(accountId);
    try { return await task(); }
    catch (error) { inputs.current.notify({ kind: "error", text: friendlyError(inputs.current.t, error) }); }
    finally { running.current = false; setStarting(false); setStartingAccount(undefined); }
  };
  const wakeOperationId = wake?.id;
  const wakePhase = wake?.status;
  useEffect(() => {
    if (!wakeOperationId || wakePhase !== "running") {
      return;
    }
    let closed = false;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const next = await api.wakeOperation(wakeOperationId);
        if (closed) {
          return;
        }
        setWakeStatusUnavailable(false);
        setWake(next);
        if (next.status === "running") {
          timer = window.setTimeout(poll, 850);
        } else {
          await inputs.current.onChanged();
          for (const result of next.results) {
            inputs.current.onRefreshed(result.account_id);
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
  }, [wakeOperationId, wakePhase]);

  const startWake = async (accountId?: string, alternateModel = false) => {
    if (wake?.status === "running") {
      setDialog("wake");
      return;
    }
    const result = accountId
      ? await begin(() => api.startWake(accountId, alternateModel), accountId)
      : await begin(api.startWakeAll);
    if (result) {
      setWake({ id: result.operation_id, status: "running", alternate_model: alternateModel, results: [] });
      setWakeStatusUnavailable(false);
      if (accountId) setDialog("wake");
    }
  };

  const startSelectedWake = async (selectedChatGptIds: string[]) => {
    const result = await begin(() => api.startWakeSelected(selectedChatGptIds));
    if (result) {
      setWake({ id: result.operation_id, status: "running", alternate_model: false, results: [] });
      setWakeStatusUnavailable(false);
    }
  };

  const cancel = async () => {
    if (!wake || cancelling) return;
    setCancelling(true);
    try { await api.cancelWake(wake.id); }
    catch (error) { inputs.current.notify({ kind: "error", text: friendlyError(inputs.current.t, error) }); }
    finally { setCancelling(false); }
  };
  const close = () => { setDialog(null); if (wake?.status !== "running") setWake(null); };
  return { wake, wakeStatusUnavailable, startingAccount, busy: starting && !startingAccount ? "wake-all" : cancelling ? "cancel-wake" : null, startWake, startSelectedWake, cancel, close };
}

export function WakeDialog({ flow, accounts, busy, startOAuth, t }: {
  flow: ReturnType<typeof useWake>; accounts: AccountView[]; busy: string | null; startOAuth: (account: AccountView) => Promise<void>; t: Translator;
}) {
  const { wake, wakeStatusUnavailable, startWake } = flow;
  if (!wake) return null;
  return (
        <Modal onClose={() => { flow.close(); }} t={t} title={t("wake.title")}>
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
                <>
                  <button className="button button-secondary" disabled={busy !== null} onClick={() => void flow.cancel()} type="button">{t("wake.cancelRemaining")}</button>
                  <button className="button button-primary" onClick={flow.close} type="button">{t("wake.runInBackground")}</button>
                </>
              ) : null}
              {wake.status !== "running" ? <button className="button button-primary" disabled={busy !== null} onClick={() => {
                flow.close();
              }} type="button">{t("common.done")}</button> : null}
            </div>
          </div>
        </Modal>
  );
}
