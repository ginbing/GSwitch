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
- target identity confirmation before switch success;
- interrupted-switch and protected-credential recovery;
- quota bucket normalization, zero remaining, and stale-cache labeling;
- a quota projection failure leaving the saved-account workspace and Switch
  action available;
- provider refresh before reset-credit selection;
- earliest eligible unexpired reset-credit selection and idempotent outcomes;
- a pending reset being surfaced as a boolean-only recovery prompt and replayed
  only after a new explicit user action;
- Wake model allowlisting, low reasoning effort, quota guards, and confirmation;
- frontend clearing secret inputs, confirming reset consumption, and disabling
  conflicting actions;
- browser-login cancellation, selected-file import boundaries, account-space
  empty state, quota stale state, and visible Wake progress.
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
