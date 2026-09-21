# GSwitch

A simple and reliable Codex account switcher.

GSwitch is for people who use more than one Codex account and want an easy way
to manage them. It shows the account Codex is using, the information needed to
make a decision, and the actions needed to change accounts safely.

It gets the job done.

## What it does

- Keep multiple Codex accounts on your computer
- Show the active account
- Show quota, reset times, and available reset credits
- Change accounts safely
- Use the eligible reset credit that expires first, after confirmation
- Wake one account or all saved accounts

> [!NOTE]
> GSwitch is under development. There are no public release builds yet.

## Platforms

Windows, macOS, and Linux.

## Run from source

Install Codex before using account actions. Install the Tauri prerequisites for
your platform, then run:

```bash
git clone https://github.com/ginbing/GSwitch.git
cd GSwitch

pnpm install --frozen-lockfile
pnpm tauri dev
```

Development notes are in [docs/README.md](./docs/README.md).

## License

AGPL-3.0.
