export const updater = {
  check: async () => null,
  relaunch: async () => undefined,
} as typeof import("../src/updater").updater;
export type AvailableUpdate = import("../src/updater").AvailableUpdate;
