# Contributing to GSwitch

GSwitch is deliberately small: a local Codex account utility, not an account
platform. Start with the [product boundary](PRODUCT.md), then use the smallest
relevant document from [the documentation router](docs/README.md).

## A compatible change

- Keep the change focused on understanding account state, importing or safely
  switching an account, reset credits, Wake, or the minimum UI and recovery
  needed for those jobs.
- Prefer an explicit, narrow behavior over a new service, daemon, provider
  layer, scheduler, database, or management surface.
- Keep credentials, complete auth documents, API keys, reset-credit IDs, and
  raw provider payloads in Rust-owned boundaries. Never add them to WebView
  state, fixtures, logs, issues, or commits.
- Do not read another application's private account storage. Import only a file
  that the user explicitly selected or exported.
- Make one small pull request per independently reviewable behavior. Explain
  the user-visible result and the local proof in its description.

## Before opening a pull request

Run the focused checks for the files you changed, followed by the applicable
commands in [Testing](docs/testing.md). For a frontend or desktop change, that
usually includes:

```bash
pnpm test
pnpm build
pnpm tauri build --no-bundle
```

For Rust changes, also run the formatting and locked test commands listed in
that document. Test real behavior, not source shape. Do not perform OAuth,
Wake, reset-credit redemption, or another billable/irreversible provider action
with a real account merely to prove a pull request.

If a proposal changes the product boundary or a durable safety rule, discuss it
with the maintainer before expanding the implementation.

## Sensitive reports

Do not use Issues or pull requests for a security vulnerability, credential,
auth document, API key, reset-credit ID, or a reproducible path that exposes
one. Follow [SECURITY.md](SECURITY.md) instead.
