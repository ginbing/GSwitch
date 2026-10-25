import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { changedScope, fullScope, noScope, scopeFor } from "./ci-scope.mjs";

const frontendScope = { ...noScope, frontend: true };

// A temporary repository with a frontend file, for real git diffs.
async function withRepo(run) {
  const root = await mkdtemp(join(tmpdir(), "gswitch-ci-scope-"));
  const git = (...args) => {
    const result = spawnSync("git", ["-C", root, ...args], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout.trim();
  };
  try {
    git("init", "-q");
    git("config", "user.email", "test@example.com");
    git("config", "user.name", "Test");
    git("config", "commit.gpgsign", "false");
    await mkdir(join(root, "src"));
    await writeFile(join(root, "src", "App.tsx"), "export const app = 'a long enough body to be detected as a rename';\n");
    git("add", ".");
    git("commit", "-q", "-m", "base");
    await run(root, git);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

test("documentation and repository metadata select no source job", () => {
  assert.deepEqual(scopeFor(["README.md", "docs/testing.md", "AGENTS.md", "LICENSE", ".github/PULL_REQUEST_TEMPLATE.md",
    "assets/readme-demo.png", "demo/api.ts", ".gitignore", ""]), noScope);
});

test("frontend sources select only the frontend job", () => {
  assert.deepEqual(scopeFor(["src/App.tsx", "src/i18n.ts", "index.html", "tsconfig.json", "vitest.config.ts", "release-notes.json"]), frontendScope);
  assert.deepEqual(scopeFor(["src/notes.md"]), frontendScope);
});

test("Rust, build, workflow, script, installer and unknown inputs select every job", () => {
  for (const path of ["src-tauri/src/accounts.rs", "src-tauri/README.md", "rust-toolchain.toml", "package.json", "pnpm-lock.yaml",
    ".node-version", "vite.config.ts", ".github/workflows/ci.yml", ".github/renovate.jsonc", "scripts/ci-scope.mjs",
    "install.ps1", "assets/gswitch-icon.svg", ".gitattributes", "something-new.toml"]) {
    assert.deepEqual(scopeFor([path]), fullScope, path);
  }
  assert.deepEqual(scopeFor(["docs/release.md", "src/App.tsx", "src-tauri/Cargo.lock"]), fullScope);
});

test("schedules select nothing, and new branches or failed detection select everything", () => {
  const passed = () => true;
  assert.deepEqual(changedScope({ event: "schedule", base: "a", head: "b" }), noScope);
  assert.deepEqual(changedScope({ event: "push", base: "0000000000000000000000000000000000000000", head: "HEAD", basePassed: passed }), fullScope);
  assert.deepEqual(changedScope({ event: "pull_request", base: "", head: "HEAD" }), fullScope);
  assert.deepEqual(changedScope({ event: "push", base: "not-a-commit", head: "HEAD", basePassed: passed }), fullScope);
});

test("a main push narrows the selection only after the previous main run passed", async () => {
  await withRepo(async (root, git) => {
    const base = git("rev-parse", "HEAD");
    await writeFile(join(root, "README.md"), "docs\n");
    git("add", ".");
    git("commit", "-q", "-m", "docs");
    const head = git("rev-parse", "HEAD");
    const cwd = process.cwd();
    process.chdir(root);
    try {
      const asked = [];
      const passed = (sha) => { asked.push(sha); return true; };
      assert.deepEqual(changedScope({ event: "push", base, head, basePassed: passed }), noScope);
      assert.deepEqual(asked, [base]);
      assert.deepEqual(changedScope({ event: "push", base, head, basePassed: () => false }), fullScope);
      assert.deepEqual(changedScope({ event: "push", base, head }), fullScope);
      assert.deepEqual(changedScope({ event: "pull_request", base, head, basePassed: () => assert.fail("not asked") }), noScope);
    } finally {
      process.chdir(cwd);
    }
  });
});

test("a rename out of the frontend still selects the frontend job", async () => {
  await withRepo(async (root, git) => {
    const base = git("rev-parse", "HEAD");
    git("mv", "src/App.tsx", "docs-App.tsx.md");
    git("commit", "-q", "-m", "move");
    const head = git("rev-parse", "HEAD");
    const script = resolve("scripts/ci-scope.mjs");
    const result = spawnSync(process.execPath, [script], {
      cwd: root, encoding: "utf8",
      env: { ...process.env, EVENT_NAME: "pull_request", BASE_SHA: base, HEAD_SHA: head, GITHUB_OUTPUT: join(root, "output") },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(join(root, "output"), "utf8"), "frontend=true\nrust=false\nplatform=false\n");
  });
});
