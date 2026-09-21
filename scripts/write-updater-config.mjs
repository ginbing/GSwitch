import { writeFile } from "node:fs/promises";
import { resolve } from "node:path";

const publicKey = process.env.TAURI_UPDATER_PUBLIC_KEY?.trim();

if (!publicKey) {
  throw new Error("TAURI_UPDATER_PUBLIC_KEY is required to prepare signed updater artifacts.");
}

const config = {
  bundle: {
    createUpdaterArtifacts: true,
  },
  plugins: {
    updater: {
      pubkey: publicKey,
      endpoints: ["https://github.com/ginbing/GSwitch/releases/latest/download/latest.json"],
      windows: {
        installMode: "passive",
      },
    },
  },
};

await writeFile(
  resolve("src-tauri/tauri.updater.conf.json"),
  `${JSON.stringify(config, null, 2)}\n`,
  { encoding: "utf8", mode: 0o600 },
);
