import { spawnSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

// Chooses the source-proof jobs for a pull request or a main push. Only paths
// known to be documentation or repository metadata select nothing; unknown
// paths and failed change detection select every job. A main push narrows the
// selection only when the previous main commit's push run passed, so a green
// main run always covers the whole tree, not just the latest diff.
const frontendOnly = [/^src\//, /^index\.html$/, /^tsconfig[^/]*\.json$/, /^vitest\.config\.[^/]+$/, /^release-notes\.json$/];
const noProof = [/^docs\//, /\.md$/, /^LICENSE$/, /^assets\/readme-[^/]+$/, /^demo\//, /^\.gitignore$/];

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

export function changedScope({ event, base, head, basePassed = () => false }) {
  if (event === "schedule") return { ...noScope };
  if (!base || !head || /^0+$/.test(base)) return { ...fullScope };
  if (event === "push" && !basePassed(base)) {
    console.error(`No passing main CI run for ${base}; selecting every source job.`);
    return { ...fullScope };
  }
  // Renames are listed as a deletion and an addition, so both sides count.
  const diff = spawnSync("git", ["diff", "--no-renames", "--name-only", base, head], { encoding: "utf8" });
  if (diff.status !== 0) {
    console.error("Change detection failed; selecting every source job.");
    return { ...fullScope };
  }
  return scopeFor(diff.stdout.split(/\r?\n/));
}

// A successful CI run for a push of that commit; a scheduled run selects no
// source job, so it does not count. Any API failure counts as not passed.
function mainRunPassed(sha) {
  const query = `repos/${process.env.GITHUB_REPOSITORY}/actions/workflows/ci.yml/runs?head_sha=${sha}&event=push&status=success&per_page=1`;
  const result = spawnSync("gh", ["api", query, "--jq", ".total_count"], { encoding: "utf8" });
  return result.status === 0 && Number(result.stdout.trim()) > 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const scope = changedScope({
    event: process.env.EVENT_NAME,
    base: process.env.BASE_SHA,
    head: process.env.HEAD_SHA,
    basePassed: mainRunPassed,
  });
  const lines = Object.entries(scope).map(([job, selected]) => `${job}=${selected}`).join("\n");
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${lines}\n`);
  console.log(lines);
}
