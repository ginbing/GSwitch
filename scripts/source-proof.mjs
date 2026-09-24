import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const mode = process.argv[2];

const commands = {
  frontend: [
    ["pnpm", ["run", "version:check"]],
    ["pnpm", ["test"]],
    ["pnpm", ["build"]],
  ],
  linux: [
    ["pnpm", ["run", "version:check"]],
    ["pnpm", ["build"]],
    ["cargo", ["fmt", "--manifest-path", "src-tauri/Cargo.toml", "--check"]],
    ["cargo", ["clippy", "--manifest-path", "src-tauri/Cargo.toml", "--all-targets", "--locked", "--", "-D", "warnings"]],
    ["cargo", ["test", "--manifest-path", "src-tauri/Cargo.toml", "--locked"]],
  ],
  platform: [
    ["cargo", ["check", "--manifest-path", "src-tauri/Cargo.toml", "--all-targets", "--locked"]],
  ],
};

if (!Object.hasOwn(commands, mode)) {
  console.error("Usage: node scripts/source-proof.mjs <frontend|linux|platform>");
  process.exit(2);
}

const base = process.env.GSWITCH_DIFF_BASE;
const head = process.env.GSWITCH_DIFF_HEAD;
const diffArgs = base && head ? ["diff", "--check", base, head] : ["diff", "--check", "HEAD"];
run("git", diffArgs);

for (const [command, args] of commands[mode]) run(command, args);

function run(command, args) {
  console.log(`\n> ${command} ${args.join(" ")}`);
  const windowsPnpm = process.platform === "win32" && command === "pnpm";
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    shell: windowsPnpm,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
