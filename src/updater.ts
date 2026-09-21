import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";

// Keep the native updater boundary small and mockable. The updater plugin
// downloads and verifies signed release artifacts itself; no update metadata
// or installer payload crosses the account-management API.
export type AvailableUpdate = Update;
export type UpdateProgressEvent = DownloadEvent;

export const updater = {
  check,
  relaunch,
};
