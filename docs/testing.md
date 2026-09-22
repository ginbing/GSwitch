# Testing

Tests prove specific behavior at a specific revision. They do not replace the
product boundary, architecture ownership, real provider behavior, or release
acceptance.

## Local validation

Frontend feedback:

```bash
pnpm test
pnpm build
```

Rust feedback:

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
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

GitHub Actions runs:

- version synchronization, frontend tests, and a production frontend build on
  Linux;
- version synchronization, Rust formatting, Clippy, tests, and a standard
  platform bundle on Windows and macOS;
- frontend, Rust, no-bundle Tauri, AppImage, and Debian-package checks on an
  Ubuntu 22.04 Linux runner.

The workflow uploads the Windows NSIS installer, macOS DMG, and Linux AppImage
plus Debian package from its exact revision. They are CI artifacts only: they
are not signed, notarized, tested through an interactive installer, or released.

The CI workflow has read-only repository permissions. A passing CI run proves
that the checked-in revision passed those commands on those runners; it does
not publish or release anything.

CI also runs GitHub dependency review on pull requests (moderate-or-higher
advisories in runtime, development, or unknown scopes) and RustSec's
`cargo-audit` against `src-tauri/Cargo.lock`. The dependency check deliberately
does not impose a license policy. Both checks use a read-only token; the
release workflow grants write access only to its draft-upload job. External
Actions use full commit SHAs with version comments. Rust and Node versions are
owned by `rust-toolchain.toml` and `.node-version`; pnpm is owned by
`package.json`'s `packageManager`.

GitHub CodeQL default setup, rather than a repository workflow, scans Actions,
JavaScript/TypeScript, and Rust. Its effective language list and the
repository's SHA-pinning policy must be read from GitHub when making a claim
about current protection; these settings are not inferred from this document.

## Required regression areas

Credential and state work should cover the exact affected boundary, including
as applicable:

- complete credential-document preservation, including unknown fields;
- stable identity matching and workspace separation;
- reauthentication replacing the correct saved profile;
- single-operation and cross-process serialization;
- failed persistence leaving the in-memory owner unchanged;
- malformed or unreadable state failing closed without leaking contents;
- a damaged account store opening a recovery-only workspace, preserving its
  source on explicit reset, and leaving unrelated live Codex files untouched;
- external or uninspectable Codex runtimes blocking sensitive mutation;
- current live credentials being reconciled before replacement;
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
- structured switch-error codes producing account-specific frontend guidance
  without parsing Rust strings or exposing credential/provider details;
- successful switching reloading the account view with the active account first;
- delayed filesystem, provider, process, and App Server command paths running
  through the asynchronous blocking-work boundary rather than the Tauri main
  thread;
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
  no-retry sent-but-unconfirmed outcome, and sequential queue continuation;
- frontend clearing secret inputs, confirming reset consumption, and disabling
  conflicting actions;
- browser-login cancellation, selected-file import boundaries, account-space
  empty state, quota stale state, and visible Wake progress;
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
