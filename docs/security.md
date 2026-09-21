# Security and recovery

GSwitch handles password-equivalent credential material. These invariants are
product behavior, not optional implementation polish.

## Secret boundary

- Complete saved credential documents and individual reset-credit IDs stay in
  Rust-owned storage and are never returned to React.
- Preserve the complete Codex-generated document rather than rebuilding known
  token fields; unknown future fields must survive a round trip.
- A pasted auth document or API key may exist only in its transient input until
  submission and must be cleared immediately afterward.
- A selected or dropped auth file is read in Rust. Its contents are not placed
  in WebView storage.
- Import support for Cockpit Tools, Sub2API, and CPA applies only to a file the
  user explicitly selects or drops. GSwitch never scans their private storage.
  When a portable Cockpit export includes non-Codex account metadata, it drops
  password, 2FA, note, phone, tag, group, provider, and mail-setting fields.
- Do not write credentials, raw provider payloads, reset-credit IDs, or account
  secrets to logs, telemetry, issue-report output, or user-facing errors.
- Credential-bearing files use atomic replacement and restrictive permissions
  where the operating system and filesystem support them.

The GSwitch account store lives under the application's config directory. It is
a small versioned JSON store because the product owns a handful of local
profiles, not relational data. Do not add a database or export subsystem without
a concrete accepted need.

## Process boundary

A running Codex process may retain old credentials in memory or write state
after a file replacement. Before any live credential mutation, GSwitch detects
known Codex Desktop, CLI, App Server, and IDE-owned processes. A relevant process
whose command line cannot be inspected is unsafe, not absent.

The operation fails before live mutation and tells the user to quit Codex.
GSwitch does not silently kill, restart, or manage external Codex processes.
Switching checks again immediately before replacement to narrow the race window.

GSwitch-owned isolated App Server children are scoped to their operation and
excluded only from that operation's external-process check. They are terminated
when the operation ends.

## Storage and concurrency

Credential-affecting operations are serialized by an in-process mutex and a
cross-process file lock. Persist the store successfully before changing its
in-memory projection.

Atomic writes protect against partial files; they do not by themselves prove
that the data being written is current. Every mutation must also verify identity
and expected prior state.

## Switching invariants

Before replacing live credentials:

1. the effective store must be confirmed as file-backed and unmanaged;
2. no external Codex runtime may be active or uninspectable;
3. the current live identity must be saved or the live profile must be empty;
4. the newest live credential must be reconciled into its saved profile;
5. the target must validate in isolation and still match its saved identity;
6. pending recovery intent must be durably stored;
7. the live credential fingerprint must still match the preflight observation.

After replacement, success requires Codex to confirm the requested identity.
Rollback is allowed only if the live file still matches the credential GSwitch
wrote. Otherwise fail closed and require recovery. A stale saved refresh token
must never overwrite a newer live token.

Changing Codex credential-store policy is explicit. Never extract keyring or
ephemeral credentials, bypass managed policy, or assume that writing
`auth.json` changes the effective identity when another store is authoritative.

## Isolated-operation invariants

OAuth, import validation, quota, reset redemption, and Wake use short-lived
GSwitch-owned Codex profiles. Before persisting any refreshed credential, verify
that its account kind and identity are unchanged. Delete the isolated profile
after success unless it must be retained as a last-resort protected recovery
copy.

Wake also uses an empty workspace, read-only sandbox, no approvals, and an
ephemeral thread. It does not load the user's project, MCP servers, Skills, or
normal Codex configuration. Its minimal instruction asks Codex not to inspect
files or use tools; it never spends a reset credit or Reserve.

## Recovery invariants

- A pending switch records the target, expected identity, previous live
  credential, previous active profile, and transaction stage.
- A pending reset records the account, exact credit, idempotency key, and start
  time so retry cannot intentionally double-consume.
- The WebView receives only whether reset recovery is pending. It cannot read
  the recorded account, provider credit, or idempotency key.
- Ambiguous external changes are preserved, not overwritten.
- A failed account-store write must not silently discard a credential refreshed
  by Codex; retain a protected recovery copy.
- A corrupt GSwitch store blocks mutation. Reset preserves the damaged file and
  never changes `CODEX_HOME/auth.json`. The recovery-only UI disables account
  intake, switching, quota refresh, reset credits, and Wake until the user
  explicitly confirms that GSwitch-only reset.
- Removing a saved account never removes the live Codex login.

Unknown, missing, stale, timed-out, or conflicting state remains unknown. Never
turn it into invented success.
