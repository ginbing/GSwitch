import { spawnSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

// Chooses the source-proof jobs for a pull request or a main push. Only paths
// known to be documentation or repository metadata select nothing; unknown
// paths and failed change detection select every job.
const frontendOnly = [/^src\//, /^index\.html$/, /^tsconfig[^/]*\.json$/, /^vitest\.config\.[^/]+$/, /^release-notes\.json$/];
const noProof = [/^docs\//, /\.md$/, /^LICENSE$/, /^assets\/readme-[^/]+$/, /^demo\//, /^\.gitignore$/, /^\.gitattributes$/];

export const noScope = Object.freeze({ frontend: false, rust: false, platform: false });
export const fullScope = Object.freeze({ frontend: true, rust: true, platform: true });

function classify(path) {
  if (path.startsWith("src-tauri/") || path === "rust-toolchain.toml") return fullScope;
  if (frontendOnly.some((pattern) => pattern.test(path))) return { ...noScope, frontend: true };
  if (noProof.some((pattern) => pattern.test(path))) return noScope;
  return fullScope;
}

export function scopeFor(paths) {
  const scope = { ...noScope };
  for (const path of paths) {
    if (!path) continue;
    for (const [job, selected] of Object.entries(classify(path))) scope[job] ||= selected;
  }
  return scope;
}

export function changedScope({ event, base, head }) {
  if (event === "schedule") return { ...noScope };
  if (!base || !head || /^0+$/.test(base)) return { ...fullScope };
  // Renames are listed as a deletion and an addition, so both sides count.
  const diff = spawnSync("git", ["diff", "--no-renames", "--name-only", base, head], { encoding: "utf8" });
  if (diff.status !== 0) {
    console.error("Change detection failed; selecting every source job.");
    return { ...fullScope };
  }
  return scopeFor(diff.stdout.split(/\r?\n/));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const scope = changedScope({ event: process.env.EVENT_NAME, base: process.env.BASE_SHA, head: process.env.HEAD_SHA });
  const lines = Object.entries(scope).map(([job, selected]) => `${job}=${selected}`).join("\n");
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${lines}\n`);
  console.log(lines);
}
