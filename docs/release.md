# Release

This document owns the release policy and distribution procedure. It does not
authorize a tag, draft, or publication.

## Current release policy

GSwitch's first public release was version `1.0.0`. The maintainer waived
additional platform acceptance testing for that release. Do not describe an
unperformed clean install, interactive flow, or platform behavior as validated.

An **available build** is an artifact produced by the release workflow. A
**validated behavior** is a claim backed by the matching recorded check. Those
are different claims.

## Release artifacts

The release workflow produces standard Tauri 2 bundles:

- Windows: current-user NSIS `setup.exe` installer;
- macOS: Apple Silicon and Intel DMGs;
- Linux: AppImage and Debian package.

`src-tauri/tauri.conf.json` is the product version source and defines the
enabled bundles. Windows uses the GUI subsystem, so launching GSwitch opens its
application window without a console; GSwitch-owned Codex helper processes also
run without visible console windows.

The Windows NSIS installer installs for the current user without an
administrator prompt and places its shortcut in a GSwitch Start menu folder. It
offers English and Simplified Chinese according to the operating-system
language. Installation does not collect account data or alter Codex credentials.

The normal Windows download is the NSIS installer. `install.ps1` is an optional
helper that downloads and starts that same installer; it is not a package
manager or a source build path.

macOS artifacts use Tauri ad-hoc signing (`bundle.macOS.signingIdentity` is
`"-"`). They do not use an Apple Developer certificate or notarization. macOS
may require a user to allow GSwitch manually in Privacy & Security.

Windows Authenticode signing remains optional: it is used only when the
configured Windows certificate is available. Linux artifacts are distributed as
the AppImage and Debian files produced by the workflow; no RPM, Snap, Flatpak,
AUR package, or Linux repository is provided.

CI artifacts are build evidence, not a public release, signing result, or proof
of interactive installation behavior. Linux builds use Ubuntu 22.04 with
Tauri's WebKitGTK 4.1 prerequisites to produce the AppImage and Debian package.

Desktop launchers on Linux and macOS do not inherit shell startup files.
GSwitch uses `GSWITCH_CODEX_BIN` when it is explicitly set and recognizes the
official Codex installer's default `$HOME/.local/bin/codex` location. A custom
Codex location must be in the graphical session's `PATH` or set with
`GSWITCH_CODEX_BIN`.

## Release workflow

Pushing a tag named `v<version>` starts the sole release workflow. The tag must
match `src-tauri/tauri.conf.json`. The workflow runs the version, frontend, and
Rust checks, builds all release artifacts, signs updater artifacts with the
maintainer-provided Tauri updater key, and creates one GitHub Release draft.

The draft uses GitHub-generated release notes. It remains a draft until a
maintainer reviews the exact assets and user-facing notes, then explicitly
publishes it. Missing Apple Developer credentials cannot block either macOS
build.

The updater requires `TAURI_UPDATER_PUBLIC_KEY` as a repository variable and
the matching `TAURI_SIGNING_PRIVATE_KEY` secret. If the private key is
passphrase-protected, the workflow also needs
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. These maintainer-owned values must not be
generated, rotated, copied into source, or printed by repository tooling.

## Signed in-app updates

GSwitch uses the official Tauri updater. It checks the signed `latest.json`
release asset once after startup and then at most once every six hours. The
native updater verifies every update signature before installation; it never
reads, exports, or changes a Codex account.

Windows uses a passive NSIS installer and exits while the installer runs. macOS
and AppImage installations request a normal relaunch. Debian packages are not
self-replaced; when an update is available, GSwitch opens the verified release
page instead.

Each release draft must contain `latest.json` and four updater signatures:
Windows NSIS, two macOS archives, and the AppImage. The Debian package is a
download fallback, not an in-app updater artifact.

## Permissions and privacy

The main window receives only the capabilities its current UI uses. Filesystem,
process, credential, and Codex operations stay behind Rust commands; the
WebView does not receive broad shell, filesystem, process, or network access.

Default release posture:

- no analytics or telemetry;
- no crash-reporting service without a separate accepted privacy decision;
- no listening port, localhost API, or remote-control surface;
- outbound traffic only for user-visible Codex/OpenAI operations and signed
  update checks.

## Versioning

`src-tauri/tauri.conf.json` is the version source. Keep the JavaScript package,
Cargo package, and Cargo lock metadata synchronized:

```bash
pnpm run version:sync
pnpm run version:check
```

After a version change is merged, a maintainer can tag the chosen `main`
revision as the matching `v<version>`. The workflow creates a draft; publishing
that draft is a separate maintainer decision.
