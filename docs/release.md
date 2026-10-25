# Release

This document owns the release policy and distribution procedure. A maintainer
request to publish a change supplies release authorization under `AGENTS.md`;
this document supplies the checks and sequence, not another approval step.

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

The helper requires the selected GitHub asset's `sha256:<64 hex digits>` digest
and verifies the downloaded bytes before starting the installer. Missing,
unsupported or malformed digests, unreadable downloads and mismatches stop
installation. Temporary downloads are removed on both success and failure.
This verifies consistency with the trusted release metadata; it is not
Authenticode signing or independent publisher authentication.

macOS artifacts use Tauri ad-hoc signing (`bundle.macOS.signingIdentity` is
`"-"`). They do not use an Apple Developer certificate or notarization. macOS
may require a user to allow GSwitch manually in Privacy & Security.

The current Windows installer is not Authenticode-signed. The release workflow
does not import a Windows certificate or PFX file. Stage A prepares a possible
SignPath Foundation application; application, approval, and future SignPath
configuration are external TBD work. Linux artifacts are distributed as the
AppImage and Debian files produced by the workflow; no RPM, Snap, Flatpak, AUR
package, or Linux repository is provided.

Microsoft Store/MSIX is intentionally deferred. It is a separate distribution
and update decision and is not part of the current GitHub Releases path.

## Maintainer Windows preview

When a desktop change needs maintainer interaction before merge, build a local
NSIS preview from a committed source revision and provide the installer as an
opt-in feedback path. Ordinary PR CI does not build installers. Record the
source commit, preview version, and installer location in the PR or its tracking
Issue. If feedback leads to another change, commit it and increase the preview
suffix before rebuilding.

From the repository root in PowerShell 7, derive a unique preview version from
the next patch version, then build. Product pull requests leave the version
files at the last release, so a suffix on the current version would sort below
the installed release and be offered that release as an update:

```powershell
$dirty = git status --porcelain
if ($dirty) { throw 'Build the preview from a clean, committed source revision.' }
$major, $minor, $patch = (node -p "require('./src-tauri/tauri.conf.json').version").Split('.')
$previewVersion = "$major.$minor.$([int]$patch + 1)-rc.1"
New-Item -ItemType Directory -Force -Path 'src-tauri/target' | Out-Null
$previewConfig = Join-Path (Resolve-Path 'src-tauri/target').Path "tauri-$previewVersion.json"
Set-Content -LiteralPath $previewConfig -Value "{`"version`":`"$previewVersion`"}" -NoNewline -Encoding utf8
pnpm tauri build --config $previewConfig --bundles nsis
$previewInstaller = "src-tauri/target/release/bundle/nsis/GSwitch_${previewVersion}_x64-setup.exe"
Get-Item -LiteralPath $previewInstaller
git rev-parse HEAD
```

The configuration overlay and installer stay under the ignored `src-tauri/target`
directory. The preview is unsigned and is not a tag, GitHub release, or update
candidate. It uses the same per-user GSwitch account storage as the installed
product, so the maintainer can exercise their own saved accounts. Do not include
production updater-signing keys in a local preview build.

CI artifacts are build evidence, not a public release, signing result, or proof
of interactive installation behavior. Linux builds use Ubuntu 22.04 with
Tauri's WebKitGTK 4.1 prerequisites to produce the AppImage and Debian package.

Desktop launchers on Linux and macOS do not inherit shell startup files.
GSwitch uses `GSWITCH_CODEX_BIN` when it is explicitly set and recognizes the
official Codex installer's default `$HOME/.local/bin/codex` location. A custom
Codex location must be in the graphical session's `PATH` or set with
`GSWITCH_CODEX_BIN`.

## Real-account acceptance

Real-account checks provide optional evidence of how the provider and installed
Codex behave. They are not a release prerequisite: the maintainer may release
without performing them, without a waiver or additional approval. Applicable
automated checks, package and updater-signature verification, provenance, and
draft review remain required.

When the maintainer chooses to test with real accounts, the agent can build the
[maintainer Windows preview](#maintainer-windows-preview) from a committed
revision and hand it over. Only the maintainer uses real accounts. With at
least two saved ChatGPT accounts, useful checks include:

1. launch shows the current account and loads each card's quota;
2. Switch to another saved account with Codex closed, then switch back;
3. Refresh one account, then Refresh all;
4. Wake one account and read its result;
5. **Sign in again** from a card menu replaces that saved login, and the
   current account's renewed login is applied or offered as **Apply**;
6. an account whose saved sign-in no longer works shows **Sign in required**
   and recovers through **Sign in again**, when such an account is available.

Reset-credit redemption is not part of this list; it keeps its own explicit
confirmation. If checks are performed, record the tested commit and actual
results in the release pull request, including any defect found. An omitted or
unanswered optional check does not stop publication. Never describe an
unperformed flow as validated or rewrite historical validation claims.

## Release workflow

Pushing a tag named `v<version>` starts the sole release workflow. The tag must
point to the exact current merged `main` commit and match
`src-tauri/tauri.conf.json`. Tag it as soon as the version pull request
merges; do not wait for that commit's CI run first.
[Real-account checks](#real-account-acceptance) are optional. The workflow
checks the version and release-asset tooling, then builds all release bundles
locally through the official Tauri CLI while the `main` CI run proves the same
commit's source checks. It does not attest or draft anything until that
commit's `CI gate` passes; CodeQL results are not awaited. It smoke checks the
Windows install and uninstall,
both macOS DMGs and updater archives, and both Linux package payloads before
signing the final updater bytes. The workflow verifies signatures against the
configured public key, builds a static `latest.json` from those signatures,
checks the exact asset set, and only then creates a draft.

The repository's active tag ruleset restricts updates and deletion of existing
`refs/tags/v*` tags, with no bypass actors or creation restriction. Create each
new version once; do not retarget or delete an existing release tag to repair a
failed build. Read the live ruleset when verifying this operational guarantee.

The draft also contains a CycloneDX SBOM generated by Syft 1.52.0 from the
frozen pnpm and locked Cargo dependency graphs. Artifact transfers use GitHub
Actions' immutable artifact path and built-in digest validation. GitHub artifact
attestations bind each exact asset to the tagged source commit and attach the
SBOM to the packages. The final workflow job downloads the draft assets and
verifies their exact set, manifest, and provenance again.

The release notes come from `release-notes.json` at the tagged commit: one to
twelve short single-line notes each in English (`en`) and Simplified Chinese
(`zh-CN`), for the version in `src-tauri/tauri.conf.json`. `latest.json` carries
the English notes in `notes` and both languages in `localized_notes`, and the
workflow writes the draft body from the same notes plus download links and a
changelog link from the previous published release. Verification fails when
`latest.json` notes differ from that file, so the notes are never edited after
the build. For an authorized release, the agent reviews the exact assets and
notes before publishing the draft. Missing Apple Developer credentials cannot
block either macOS build.

See the repository's [Code signing policy](code-signing-policy.md) for the
current roles, privacy statement, SignPath application facts, and future
Windows signing order.

The updater requires `TAURI_UPDATER_PUBLIC_KEY` as a repository variable and
the matching `TAURI_SIGNING_PRIVATE_KEY` secret. If the private key is
passphrase-protected, the workflow also needs
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. These maintainer-owned values must not be
generated, rotated, copied into source, or printed by repository tooling.

## Signed in-app updates

GSwitch uses the official Tauri updater. It checks the `latest.json` release
manifest after startup, six hours after each successful check, when the window
returns to the foreground at least an hour after the last check, and five
minutes after a failed check. Checks stay quiet; **Later** hides a version for
the rest of that run. The prompt shows that release's notes in the UI language,
falling back to English, as plain text: `latest.json` is not signed, so the
notes are never interpreted as markup or links. The native updater verifies the
downloaded update artifact's signature before installation.
This verifies the update source; it is not a guarantee about what an installed
version may do with account data.

The updater enables TLS and system proxy support explicitly. Its optional
legacy ZIP package support is disabled: GSwitch publishes the standard NSIS
executable and AppImage directly, and macOS keeps its native tar.gz updater
bundle. No GSwitch release uses the older Windows ZIP updater format.

Windows uses a passive NSIS installer and exits while the installer runs. macOS
and AppImage installations request a normal relaunch. Debian packages are not
self-replaced; when an update is available, GSwitch opens the verified release
page instead.

Each release draft must contain `latest.json` and signatures for the Windows
NSIS installer, two macOS updater archives, AppImage, and Debian package. The
Debian package is a download fallback, not an in-app updater artifact.

To check the provenance of a downloaded asset, use the exact merged commit and
tag recorded for the release:

```bash
gh attestation verify GSwitch_<version>_x64-setup.exe -R ginbing/GSwitch \
  --source-digest <main-commit-sha> --source-ref refs/tags/v<version> \
  --signer-workflow ginbing/GSwitch/.github/workflows/release.yml
```

The draft remains unpublished until its complete assets and notes pass review.

When a trusted Windows signing path exists, the release order remains: build
the Windows bundle from the release commit, Authenticode-sign the final
installer through the approved SignPath path, generate the Tauri updater
signature from those final bytes, generate `latest.json`, and then publish the
reviewed draft assets. The updater signature must not be created before
Authenticode changes the installer.

## Permissions and privacy

The main window receives only the capabilities its current UI uses. Filesystem,
process, credential, and Codex operations stay behind Rust commands; the
WebView does not receive broad shell, filesystem, process, or network access.

Default release posture:

- no analytics or telemetry;
- no crash-reporting service without a separate accepted privacy decision;
- no listening port, localhost API, or remote-control surface;
- outbound traffic only for user-visible Codex/OpenAI operations and signed
  GSwitch update checks.

## Versioning

`src-tauri/tauri.conf.json` is the version source. Keep the JavaScript package,
Cargo package, and Cargo lock metadata synchronized:

```bash
pnpm run version:sync
pnpm run version:check
```

Make the version change its own commit and pull request, after the product
changes it releases have merged. The same pull request rewrites
`release-notes.json` for the new version; `version:check` fails until it does.
A product pull request does not touch the version files or the release notes.

After an authorized version change is merged, tag that exact `main` commit as
the matching `v<version>`. The workflow waits for the commit's `CI gate` and
then creates a draft; review its assets and notes before publishing it.
