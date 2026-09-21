# Release

This document owns durable release gates, not a release calendar, release
status, or deployment authorization.

## Supported targets

The v1 desktop targets are Windows, macOS, and Linux. A platform is supported
only after its matching packaged build and clean-install checks have run for the
specific release candidate.

Use standard Tauri 2 bundles:

- Windows: NSIS installer;
- macOS: application bundle distributed through a DMG.
- Linux: AppImage and Debian package.

Production Windows builds use the GUI subsystem. Launching GSwitch must open
only its application window; GSwitch-owned Codex helper processes also run
without creating visible console windows.

The Windows NSIS installer uses the versioned GSwitch application icon plus a
small branded header and welcome/finish sidebar. It offers English and
Simplified Chinese according to the operating-system language, installs for the
current user without an administrator prompt, and places its shortcut in a
GSwitch Start menu folder. These assets are presentation only; installation
must not collect account data or alter Codex credentials.

`src-tauri/tauri.conf.json` is the product version source and enables standard
bundles. Platform-specific Tauri configuration selects NSIS on Windows, DMG on
macOS, and AppImage plus Debian package on Linux. The repository's current
Tauri configuration is authoritative for which bundles and architectures a
particular revision actually produces.

The CI path builds these platform bundles from the exact checked revision
and keeps them as unsigned CI artifacts. Artifacts are evidence for a build;
they are not a public release, signing result, clean-install result, or
publication authorization.

Linux builds use the Tauri-supported AppImage and Debian formats. Build them on
an Ubuntu 22.04 baseline, which supplies Tauri's required WebKitGTK 4.1
development packages without unnecessarily raising the AppImage's glibc floor.
The project does not publish an RPM, Snap, Flatpak, AUR package, or a Linux
repository without a separate product decision.

Desktop launchers on Linux and macOS do not inherit shell startup files. GSwitch
uses `GSWITCH_CODEX_BIN` when the user explicitly sets it and recognizes the
official Codex installer's default `$HOME/.local/bin/codex` location. A custom
Codex location outside those paths must be placed in the graphical session's
`PATH` or provided through `GSWITCH_CODEX_BIN`.

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

Use SemVer. `src-tauri/tauri.conf.json` is the application version source. Keep
the JavaScript package, Cargo package, and Cargo lock metadata synchronized by
running:

```bash
pnpm run version:sync
pnpm run version:check
```

Review the resulting diff and lockfile before committing.

Do not add channels, a release train, or a compatibility matrix until a real
distribution need requires one.

## Public-release gate

A public candidate requires:

- an exact source revision with passing repository CI;
- frontend and Rust tests plus no-bundle Tauri builds on Windows, macOS, and
  Linux;
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
