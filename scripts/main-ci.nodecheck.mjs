import assert from "node:assert/strict";
import test from "node:test";
import { mainPushRunsQuery, waitForMainCi } from "./main-ci.mjs";

const sha = "9608764ed85f6d0355be961b4b8a2584d12b56fc";
const budgetMs = 55 * 60_000;

// Shaped like the Actions workflow-runs API; on 2026-11-02 this commit had a
// passing push run and a later passing schedule run that skipped every source job.
function run(id, event, status, conclusion, overrides = {}) {
  return {
    id, event, status, conclusion, head_branch: "main", head_sha: sha, path: ".github/workflows/ci.yml",
    html_url: `https://github.com/ginbing/GSwitch/actions/runs/${id}`, ...overrides,
  };
}
const push = (status, conclusion) => run(41190994114, "push", status, conclusion);
const schedule = (conclusion) => run(41444609521, "schedule", "completed", conclusion);

// Replays API responses on a fake clock: each poll returns the next response,
// repeating the last one, and waiting only advances the clock.
async function release(...responses) {
  let time = 0;
  const polls = [];
  const log = [];
  const passed = await waitForMainCi({
    sha,
    budgetMs,
    listRuns: (asked) => {
      assert.equal(asked, sha);
      polls.push(time);
      const response = responses[Math.min(polls.length, responses.length) - 1];
      if (response instanceof Error) throw response;
      return response;
    },
    now: () => time,
    wait: async (ms) => { time += ms; },
    log: (message) => log.push(message),
  });
  return { passed, polls, log };
}

test("the release asks only for main push runs of the tagged commit", () => {
  assert.equal(mainPushRunsQuery("ginbing/GSwitch", sha),
    `repos/ginbing/GSwitch/actions/workflows/ci.yml/runs?event=push&branch=main&head_sha=${sha}&per_page=100`);
});

test("a failed main push run rejects the release even when the schedule run passed", async () => {
  const { passed, polls, log } = await release([schedule("success"), push("completed", "failure")]);
  assert.equal(passed, false);
  assert.equal(polls.length, 1);
  assert.match(log.at(-1), /41190994114 finished: failure/);
});

test("a cancelled main push run rejects the release", async () => {
  assert.equal((await release([push("completed", "cancelled")])).passed, false);
});

test("an unfinished main push run keeps the release waiting until it finishes", async () => {
  const { passed, polls, log } = await release(
    [schedule("success"), push("queued", null)],
    [schedule("success"), push("in_progress", null)],
    [schedule("success"), push("in_progress", null)],
    [schedule("success"), push("completed", "success")],
  );
  assert.equal(passed, true);
  assert.deepEqual(polls, [0, 30_000, 60_000, 90_000]);
  assert.deepEqual(log.map((line) => line.replace(/^.*runs\/\d+ /, "")), ["is queued.", "is in_progress.", "finished: success."]);
});

test("schedule and pull request runs alone never pass, and the release fails at the wait budget", async () => {
  const prRun = run(41190000000, "pull_request", "completed", "success", { head_branch: "fix/example" });
  const { passed, polls, log } = await release([schedule("success"), prRun]);
  assert.equal(passed, false);
  assert.equal(polls.at(-1), budgetMs);
  assert.equal(polls.length, budgetMs / 30_000 + 1);
  assert.deepEqual(log, [`No main push CI run for ${sha} yet.`, `No completed main push CI run for ${sha} within 55 minutes.`]);
});

test("a run for another commit, branch or workflow does not count", async () => {
  const others = [
    run(1, "push", "completed", "success", { head_sha: "0".repeat(40) }),
    run(2, "push", "completed", "success", { head_branch: "release/1.0.35" }),
    run(3, "push", "completed", "success", { path: ".github/workflows/release.yml" }),
  ];
  assert.equal((await release(others)).passed, false);
});

test("a passed main push run passes the release even when the schedule run failed", async () => {
  const { passed, polls } = await release([schedule("failure"), push("completed", "success")]);
  assert.equal(passed, true);
  assert.equal(polls.length, 1);
});

test("a docs-only main push run passes on its own conclusion", async () => {
  // Its change scope selected no source job, so they were skipped and the run succeeded.
  assert.equal((await release([push("completed", "success")])).passed, true);
});

test("the newest main push run of the commit decides", async () => {
  const older = run(41190994000, "push", "completed", "success");
  assert.equal((await release([older, push("completed", "failure")])).passed, false);
  assert.equal((await release([push("completed", "failure"), run(41190995000, "push", "completed", "success")])).passed, true);
});

test("an API failure is retried within the wait budget", async () => {
  const { passed, polls, log } = await release(new Error("HTTP 502"), [push("completed", "success")]);
  assert.equal(passed, true);
  assert.equal(polls.length, 2);
  assert.equal(log[0], "Could not list CI runs: HTTP 502");
});
