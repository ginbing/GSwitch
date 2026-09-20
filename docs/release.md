# Release

This document owns durable release gates, not a release calendar, release
status, or deployment authorization.

## Supported targets

The v1 desktop targets are Windows and macOS. Linux is not a release requirement.
Keep portable code where Tauri makes that natural, but do not add platform work
or CI for an unaccepted target.

Use standard Tauri 2 bundles:

- Windows: NSIS installer;
- macOS: application bundle distributed through a DMG.

Production Windows builds use the GUI subsystem. Launching GSwitch must open
only its application window; GSwitch-owned Codex helper processes also run
without creating visible console windows.

The repository's current Tauri configuration is authoritative for which bundles
and architectures a particular revision actually produces.

## Signing and distribution

A public macOS build requires a Developer ID signing identity and Apple
notarization credentials. Verify the stapled/notarized artifact on a clean
machine before publication.

Windows release artifacts should be Authenticode signed when a code-signing
certificate is configured. An unsigned build is a development or explicitly
identified preview artifact, not evidence of a trusted public installer.

Signing identities, passwords, tokens, and notarization material are release
secrets. Keep them out of source, logs, Issues, and generated support output.

GSwitch has no auto-updater in v1. Do not add one until signing ownership and
release operations are stable. If Tauri's updater is later accepted, update
artifacts must use its signed-update mechanism; signature verification is not an
optional convenience.

## Permissions and privacy

Grant the main window only the capabilities used by its current UI. Filesystem,
process, credential, and Codex operations stay behind Rust commands; do not grant
the WebView broad shell, filesystem, process, or network authority.

Default release posture:

- no analytics or telemetry;
- no crash-reporting service without a separate accepted privacy decision;
- no listening port, localhost API, or remote-control surface;
- outbound traffic only for user-visible Codex/OpenAI operations and an
  explicitly accepted future update check.

## Versioning

Use SemVer. `src-tauri/tauri.conf.json` is the application version source; keep
JavaScript and Cargo package metadata synchronized during release preparation.
Run the repository's version-sync command when it is present, then review the
resulting diff and lockfile before committing.

Do not add channels, a release train, or a compatibility matrix until a real
distribution need requires one.

## Public-release gate

A public candidate requires:

- an exact source revision with passing repository CI;
- frontend and Rust tests plus no-bundle Tauri builds on Windows and macOS;
- bundled artifacts produced from that same candidate revision;
- clean-install smoke tests on each advertised platform and architecture;
- account intake, live identity, confirmed switch, quota, reset-credit, Wake,
  and recovery behavior matching the advertised feature set;
- unsupported credential-store behavior that fails without changing the live
  login;
- no unresolved credential-loss or secret-exposure regression;
- review of effective Tauri capabilities and network behavior;
- signed/notarized packaging where required for the distribution claim;
- accurate release notes for capabilities, prerequisites, and limitations.

A merge, passing source check, or successful local build does not by itself
produce or authorize a public release.
