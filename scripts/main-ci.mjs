import { spawnSync } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";
import { pathToFileURL } from "node:url";

// Waits for the CI run that proves a release commit: ci.yml run for a push of
// that exact commit to main. Pull request and weekly schedule runs report a
// `CI gate` on the same commit, but a schedule run selects no source job, so
// only the main push run counts. Its conclusion already reflects the source
// jobs its change scope selected; skipped jobs outside that scope do not fail it.

export function mainPushRun(runs, sha) {
  return runs
    .filter((run) => run.path === ".github/workflows/ci.yml" && run.event === "push"
      && run.head_branch === "main" && run.head_sha === sha)
    .sort((a, b) => b.id - a.id)[0];
}

// done=false keeps waiting; done=true ends with passed or not.
export function mainCiState(runs, sha) {
  const run = mainPushRun(runs, sha);
  if (!run) return { done: false, message: `No main push CI run for ${sha} yet.` };
  if (run.status !== "completed") return { done: false, message: `Main push CI run ${run.html_url} is ${run.status}.` };
  return { done: true, passed: run.conclusion === "success", message: `Main push CI run ${run.html_url} finished: ${run.conclusion}.` };
}

export async function waitForMainCi({ sha, listRuns, budgetMs, intervalMs = 30_000, now = Date.now, wait = sleep, log = console.log }) {
  const deadline = now() + budgetMs;
  let last;
  for (;;) {
    let state;
    try {
      state = mainCiState(await listRuns(sha), sha);
    } catch (error) {
      state = { done: false, message: `Could not list CI runs: ${error.message}` };
    }
    if (state.message !== last) log(state.message);
    last = state.message;
    if (state.done) return state.passed;
    if (now() >= deadline) {
      log(`No completed main push CI run for ${sha} within ${budgetMs / 60_000} minutes.`);
      return false;
    }
    await wait(intervalMs);
  }
}

export function mainPushRunsQuery(repository, sha) {
  return `repos/${repository}/actions/workflows/ci.yml/runs?event=push&branch=main&head_sha=${sha}&per_page=100`;
}

function listMainPushRuns(sha) {
  const query = mainPushRunsQuery(process.env.GITHUB_REPOSITORY, sha);
  const result = spawnSync("gh", ["api", query], { encoding: "utf8", timeout: 60_000 });
  if (result.status !== 0) throw new Error(result.stderr?.trim() || `gh api exited with ${result.status}`);
  return JSON.parse(result.stdout).workflow_runs;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [sha, minutes] = process.argv.slice(2);
  if (!sha || !(Number(minutes) > 0)) {
    console.error("Usage: node scripts/main-ci.mjs <commit-sha> <budget-minutes>");
    process.exit(2);
  }
  const passed = await waitForMainCi({ sha, listRuns: listMainPushRuns, budgetMs: Number(minutes) * 60_000 });
  process.exit(passed ? 0 : 1);
}
