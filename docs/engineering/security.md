# Security and recovery

GSwitch handles credential material. Safety rules are product behavior, not implementation polish.

## Secret boundary

Credential documents remain Rust-owned.

Do not:

- expose complete credentials to React;
- store them in localStorage or sessionStorage;
- print token contents in logs or user-facing errors;
- reconstruct only selected fields when the complete Codex document can be preserved.

Local credential-bearing files should use private permissions where the platform supports them.

## Process boundary

A running Codex runtime may retain old credentials in memory or write state after a file replacement.

Before replacing the live credential, detect known external Codex runtimes and fail before mutation when one is active.

v1 does not silently kill or restart Codex.

A second process check immediately before replacement protects against races between validation and mutation.

## Reconciliation

Before switching away from the current live account, preserve the newest live credential into the matching saved profile.

A stale saved profile must not overwrite a newer live refresh token.

Unknown live accounts are user data. Do not overwrite them merely because GSwitch cannot match them.

## Atomic writes

Credential and account-store writes must fail without leaving a partially written document.

In-memory state changes only after durable persistence succeeds.

## Recovery

A switch must record enough state before live replacement to distinguish:

- target verified;
- previous state still present;
- ambiguous external modification.

If state changed outside the known switch, fail closed instead of forcing either credential.

Resetting GSwitch state must never delete or damage the live Codex credential.

## Credential-store policy

Do not silently change Codex credential-store policy.

Managed, keyring-only, ephemeral, unreadable, or otherwise unsupported effective state must remain untouched unless a supported explicit migration is available.

## External state

Timeouts, missing responses, failed validation, and ambiguous provider outcomes remain unknown. Do not turn them into invented success.
