import { LoaderCircle, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, type UpdateDelivery } from "./api";
import type { AppLanguage, Translator } from "./i18n";
import { updater, type AvailableUpdate } from "./updater";

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
// A stalled check counts as failed so the schedule keeps running. The timeout
// applies to the check request only, not to the download.
const UPDATE_CHECK_TIMEOUT_MS = 30 * 1000;
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

// Owns the signed-update schedule and the offered update for the whole run.
export function useUpdate() {
  const [pending, setPending] = useState<PendingUpdate | null>(null);
  const [phase, setPhase] = useState<UpdatePhase>("available");
  const [progress, setProgress] = useState<number | null>(null);
  const dismissedVersions = useRef(new Set<string>());
  const checkInFlight = useRef(false);
  const checksActive = useRef(false);
  const checkTimer = useRef<number | undefined>(undefined);
  const lastCheck = useRef(0);
  const pendingRef = useRef<PendingUpdate | null>(null);
  const phaseRef = useRef<UpdatePhase>("available");

  useEffect(() => { pendingRef.current = pending; }, [pending]);
  useEffect(() => { phaseRef.current = phase; }, [phase]);

  const check = useCallback(async () => {
    if (!("__TAURI_INTERNALS__" in window) || checkInFlight.current) {
      return;
    }
    checkInFlight.current = true;
    lastCheck.current = Date.now();
    let nextCheck = UPDATE_RETRY_MS;
    try {
      const update = await updater.check({ timeout: UPDATE_CHECK_TIMEOUT_MS });
      const delivery = update ? await api.updateDelivery() : null;
      // Read after the awaits: the user may have started installing meanwhile.
      const current = pendingRef.current;
      const installing = ["downloading", "installing", "ready"].includes(phaseRef.current);
      if (update && delivery
        && !dismissedVersions.current.has(update.version)
        && current?.update.version !== update.version
        && !(current && installing)) {
        pendingRef.current = { update, delivery };
        setPending(pendingRef.current);
        setPhase("available");
        setProgress(null);
      }
      nextCheck = UPDATE_CHECK_INTERVAL_MS;
    } catch {
      // A failed background check stays quiet and is retried soon. A
      // user-initiated install reports an actionable retry state instead.
    } finally {
      checkInFlight.current = false;
      window.clearTimeout(checkTimer.current);
      if (checksActive.current) {
        checkTimer.current = window.setTimeout(() => void check(), nextCheck);
      }
    }
  }, []);

  useEffect(() => {
    checksActive.current = true;
    void check();
    const checkIfStale = () => {
      if (document.visibilityState !== "hidden" && Date.now() - lastCheck.current >= UPDATE_FOCUS_CHECK_MS) {
        void check();
      }
    };
    window.addEventListener("focus", checkIfStale);
    document.addEventListener("visibilitychange", checkIfStale);
    return () => {
      checksActive.current = false;
      window.clearTimeout(checkTimer.current);
      window.removeEventListener("focus", checkIfStale);
      document.removeEventListener("visibilitychange", checkIfStale);
    };
  }, [check]);

  const dismiss = () => {
    if (pending) {
      dismissedVersions.current.add(pending.update.version);
    }
    setPending(null);
    setPhase("available");
    setProgress(null);
  };

  const install = async () => {
    if (!pending) {
      return;
    }
    if (pending.delivery === "release_download") {
      try {
        await api.openLatestRelease();
      } catch {
        setPhase("error");
      }
      return;
    }

    setPhase("downloading");
    setProgress(null);
    let expectedBytes: number | undefined;
    let downloadedBytes = 0;
    try {
      await pending.update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          expectedBytes = event.data.contentLength;
        } else if (event.event === "Progress") {
          downloadedBytes += event.data.chunkLength;
          if (expectedBytes && expectedBytes > 0) {
            setProgress(Math.min(100, Math.round((downloadedBytes / expectedBytes) * 100)));
          }
        }
      });
      setPhase(pending.delivery === "installer_exits" ? "installing" : "ready");
    } catch {
      setPhase("error");
    }
  };

  const restart = async () => {
    try {
      await updater.relaunch();
    } catch {
      setPhase("error");
    }
  };

  return { pending, phase, progress, dismiss, install, restart };
}

export function UpdateNotice({ flow, language, t }: {
  flow: ReturnType<typeof useUpdate>;
  language: AppLanguage;
  t: Translator;
}) {
  const { pending, phase, progress } = flow;
  if (!pending) {
    return null;
  }
  const notes = updateNotes(pending.update, language);
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
          <button className="button button-quiet" onClick={flow.dismiss} type="button">{t("update.later")}</button>
        ) : null}
        <button
          className="button button-primary"
          disabled={downloading}
          onClick={phase === "ready" ? () => void flow.restart() : () => void flow.install()}
          type="button"
        >
          {downloading ? <LoaderCircle className="spin" size={15} /> : <RefreshCw size={15} />}
          {primaryLabel}
        </button>
      </div>
    </section>
  );
}
