# Testing

Tests prove specific behavior at a specific revision. They do not replace the
product boundary, architecture ownership, real provider behavior, or release
acceptance.

## Local validation

Frontend source proof:

```bash
pnpm run ci:source frontend
```

Full source proof (run on Linux to match CI):

```bash
pnpm run ci:source linux
```

Desktop compilation without producing an installer:

```bash
pnpm tauri build --no-bundle
```

Release configuration checks:

```bash
pnpm run version:check
```

Linux distribution bundles, on a Linux host with the current Tauri prerequisites:

```bash
pnpm run bundle:linux
```

Run focused tests while editing, then the full relevant path before handoff.

## CI coverage

The CI workflow always starts and detects changed paths inside the run, so a
path filter cannot leave a required workflow pending. Frontend changes call the
repository-owned `ci:source` command for the version check, UI tests, and
production frontend build. Rust changes call its Linux mode for version,
frontend build, Rust formatting, Clippy, and the full Rust test suite. Frontend
only changes do not start the Rust or platform workers. Changes to application
or build inputs also compile all Rust targets on Windows and macOS; those jobs
do not repeat tests or create installer bundles. Node and Cargo dependencies
use runner caches, and newer runs cancel older runs for the same pull request
or branch.

`CI gate` reports whether the scoped source jobs and security checks succeeded.
It runs even when a scoped source job is skipped and fails if scope detection
fails or a required job for the changed paths did not pass. During a required
check migration, keep the old job names available until the new gate passes on
the pull request, then update the GitHub ruleset and read it back before
merging.

Ordinary CI does not create or upload installers. The release workflow builds
and smoke-checks final platform packages before upload; CI success alone does
not prove interactive installation, platform signing, notarization, or public
release readiness.

The CI workflow has read-only repository permissions. A passing CI run proves
that the checked-in revision passed those commands on those runners; it does
not publish or release anything.

The `main` ruleset is the authority for current required checks, strict update
requirements, and resolved review conversations. Read the live ruleset when
changing or reporting those requirements; this document describes workflow
behavior and migration steps only.

CI runs GitHub dependency review on every pull request (moderate-or-higher
advisories in runtime, development, or unknown scopes) and RustSec's
`cargo-audit` on pull requests and main pushes, with a weekly scheduled audit.
The dependency check deliberately does not impose a license policy. Both checks
use a read-only token; the release workflow grants write access only to its
draft-upload job. External Actions use full commit SHAs with version comments.
Rust and Node versions are owned by `rust-toolchain.toml` and `.node-version`;
pnpm is owned by `package.json`'s `packageManager`.

Renovate's [Dependency Dashboard](https://github.com/ginbing/GSwitch/issues/110)
is the intake queue for routine npm, Cargo, and GitHub Actions updates. A
routine update creates a branch only after dashboard approval. Renovate opens
security remediation PRs immediately, without automerge; GitHub Dependabot
Alerts remain a signal but Dependabot Security Updates PRs are disabled.

GitHub CodeQL default setup, rather than a repository workflow, scans Actions,
JavaScript/TypeScript, and Rust. Its effective language list and the
repository's SHA-pinning policy must be read from GitHub when making a claim
about current protection; these settings are not inferred from this document.

## Required regression areas

Credential and state work should cover the exact affected boundary, including
as applicable:

- complete credential-document preservation, including unknown fields;
- encrypted-vault round-trip of unknown credential fields, metadata JSON with
  no token, reset ID, idempotency key, rollback auth, or recovery credential;
- native credential storage unavailable, missing secret references, and
  incomplete legacy migration failing closed without an empty library or
  plaintext fallback;
- version-3 inline-store migration, restart-safe vault hydration, stable
  identity preservation, unversioned legacy migration, pending-recovery import,
  and same-identity token rotation advancing generation without duplicate
  profiles or retaining the retired secret generation;
- simulated interrupted migration, vault-write failure preserving the only
  legacy source, metadata-commit failure preserving the old readable secret,
  concurrent startup migration waiting on the operation lock, missing vault
  references, and inline switch recovery failing closed;
- protected switch rollback and reset-credit material round-tripping through
  the vault without appearing in `accounts.json`, plus saved-account removal
  leaving the live Codex file untouched;
- isolated App Server profile cleanup through unconditional scope cleanup;
- OAuth App Server-before-profile teardown and explicit cleanup errors,
  including a Windows-exclusive `auth.json` handle fixture;
- encrypted pending-credential replay after restart, generation mismatch
  refusal, intended-identity mismatch refusal, failed-commit retry, missing
  queue material fail-closed, and count-only IPC;
- stable identity matching and workspace separation;
- reauthentication replacing the correct saved profile;
- single-operation and cross-process serialization;
- failed persistence leaving the in-memory owner unchanged;
- malformed or unreadable state failing closed without leaking contents;
- a damaged account store opening a recovery-only workspace, preserving its
  source on explicit reset, and leaving unrelated live Codex files untouched;
- external or uninspectable Codex runtimes blocking sensitive mutation;
- current live credentials being reconciled before replacement;
- Add current account using a read-only ChatGPT access-token snapshot without
  managed refresh or live-file writes, preserving unknown credential fields,
  matching account/workspace identity, revalidating one same-identity newer
  live document, and rejecting identity changes or another update; an external
  Codex process must not block this read/save path, while API-key behavior stays
  on its existing non-refreshing path;
- switch process rejection happening before target-network validation and the
  process/fingerprint checks running again before live replacement;
- a valid ChatGPT switch snapshot using one read-only account check without
  managed refresh or credential rewrite;
- one 401/403 switch fallback through an isolated managed refresh, with all
  rate-limit, network, TLS, timeout, 5xx, and malformed-response failures
  refusing that fallback;
- API-key switching performing local structure and stable-identity checks with
  no provider request;
- target identity mismatch and live-fingerprint races preventing mutation;
- post-write identity confirmation using only a local `auth.json` reread;
- verification failure restoring the previous credential only when the live
  file still equals GSwitch's write, otherwise preserving pending recovery;
- interrupted-switch and protected-credential recovery;
- saved-account removal protecting the live identity, allowing removal of a
  non-current profile while Codex runs, leaving live `auth.json` untouched, and
  keeping lock and persistence failures actionable in the confirmation;
- structured switch-error codes producing account-specific frontend guidance
  without parsing Rust strings or exposing credential/provider details;
- successful switching reloading the account view with the active account first;
- delayed filesystem, provider, process, and App Server command paths running
  through the asynchronous blocking-work boundary rather than the Tauri main
  thread;
- quota requests across startup and manual refresh sharing one serial queue,
  same-account refresh coalescing, six-account continuation after partial
  failure, and stale or unavailable state on each failed card;
- a one-account quota refresh updating its own projection without rebuilding
  the workspace, and that account's busy state leaving unrelated cards usable;
- quota bucket normalization, zero remaining, and stale-cache labeling;
- a quota projection failure leaving the saved-account workspace and Switch
  action available;
- provider refresh before reset-credit selection;
- earliest eligible unexpired reset-credit selection and idempotent outcomes;
- a pending reset being surfaced as a boolean-only recovery prompt and replayed
  only after a new explicit user action;
- Wake's fixed minimal Responses payload, quota guards, active-token reread,
  externally owned no-refresh path, inactive authentication-only refresh,
  no-retry sent-but-unconfirmed outcome, sequential queue continuation, saved
  email/workspace identity, localized outcome, and background progress actions;
- frontend clearing secret inputs, confirming reset consumption, and disabling
  conflicting actions;
- browser-login cancellation, selected-file import boundaries, account-space
  empty state, quota stale state, and visible Wake progress in English and
  Simplified Chinese at the minimum and default window sizes in light and dark
  themes;
- local migration allowlisting, no startup scan, plaintext and supported
  encrypted Cockpit records, missing/bad key and envelope fail-closed behavior,
  source immutability, sanitized previews, existing-identity suppression,
  preview-confirm identity revalidation, bounded custom roots, mixed source
  states, and handoff through normal intake;
- multi-file import picker and drop paths, one-file compatibility, cross-file
  identity deduplication, saved-identity duplicates, mixed parse outcomes,
  batch file/byte limits, sequential validation, sanitized aggregate results,
  and successful persistence when post-import quota refresh fails.
- versioned portable export structure, intentional omissions, empty/stale
  selections, cancellation before write, Unix private permissions, version-1
  import round-trip, unknown-version rejection, saved-identity deduplication,
  and aggregate-only command output;
- Add Account import-first hierarchy, account selection controls, selected Wake,
  unencrypted-export confirmation, and result feedback without credential data.
- the toolbar's visible ready indicator beside a separately truncatable account
  label, plus the canonical SVG and generated native/installer icon dimensions.
- system-language selection, unsupported-locale fallback, immediate manual
  language selection, language preference persistence, locale-aware reset-time
  formatting, and the main account flow in Simplified Chinese.

Prefer behavior assertions over broad snapshots. Mock protocol payloads prove
normalization and local policy; they do not prove the current remote provider.

## What CI does not prove

Standard CI does not by itself prove:

- a real OAuth login, installed Codex App Server, or live provider response;
- process detection against every Codex/IDE version;
- a signed or notarized installer;
- clean install, upgrade, uninstall, or OS security-dialog behavior;
- actual Linux desktop launch, OAuth browser handoff, Codex credential-store
  behavior, or package-manager integration outside the Linux runner;
- the exact permissions of a packaged artifact;
- public-release readiness.

Those claims require the matching integration or release check. Never report a
platform, provider, installer, or recovery path as validated unless that exact
proof ran.
