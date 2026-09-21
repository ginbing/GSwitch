# GSwitch

A simple and reliable Codex account switcher.

GSwitch is for people who use more than one Codex account and want an easy way
to manage them.

It gets the job done.

## What it does

- Keep multiple Codex accounts on your computer
- Show the active account
- Show quota, reset times, and available reset credits
- Change accounts safely
- Use the eligible reset credit that expires first, after confirmation
- Wake one account or all saved accounts

## Install

Release builds are published on [GitHub Releases](https://github.com/ginbing/GSwitch/releases).
When a release is available, choose the file for your computer:

- **Windows:** the NSIS `setup.exe` installer
- **macOS:** the DMG for Apple Silicon or Intel, matching your Mac
- **Linux:** the AppImage or Debian (`.deb`) package

On macOS, GSwitch is ad-hoc signed rather than Apple-notarized. macOS may ask
you to allow it manually in Privacy & Security.

Codex must be installed before you use account actions in GSwitch.

For the current Windows trust status, signing roles, and release order, see the
[Code signing policy](./docs/code-signing-policy.md).

### Windows PowerShell helper

Downloading the installer is the normal Windows path. If you prefer PowerShell,
download [install.ps1](./install.ps1) and run it from a normal,
non-administrator PowerShell window:

```powershell
.\install.ps1
```

The helper downloads and starts the same `setup.exe` from the latest public
release. It does not build GSwitch or install development tools.

## Uninstall

- **Windows:** open Settings → Apps → Installed apps, find GSwitch, and choose
  Uninstall.
- **macOS:** move GSwitch to the Trash.
- **Linux:** delete the AppImage, or remove the Debian package with your
  distribution's package manager.

## Updates

After installation, GSwitch checks GitHub Releases for signed updates and can
install supported updates from inside the app.

GSwitch has no analytics or telemetry. It contacts Codex/OpenAI for account
operations you request and GitHub Releases for signed update checks and
downloads.

## Build from source

To develop GSwitch, install the Tauri prerequisites for your platform, then:

```bash
git clone https://github.com/ginbing/GSwitch.git
cd GSwitch
pnpm install
pnpm tauri dev
```

Development details are in [docs/README.md](./docs/README.md).

GSwitch is an independent project and is not affiliated with OpenAI.

## License

AGPL-3.0.
