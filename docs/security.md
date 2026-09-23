# Security and recovery

GSwitch handles password-equivalent credential material. These invariants are
product behavior, not optional implementation polish.

## Secret boundary

- Complete saved credential documents, individual reset-credit IDs,
  idempotency keys, rollback auth, and protected recovery copies stay in the
  Rust-only Stronghold vault and are never returned to React. `accounts.json`
  holds only non-secret metadata plus opaque secret references and generations.
- The Stronghold snapshot is encrypted with a random root key held by the
  native platform credential manager (Keychain, Windows Credential Manager, or
  Secret Service). GSwitch never exposes a Stronghold command/capability to the
  WebView, writes a root key beside the snapshot, or falls back to plaintext
  when protected storage is unavailable.
- Preserve the complete Codex-generated document rather than rebuilding known
  token fields; unknown future fields must survive a round trip.
- A pasted auth document or API key may exist only in its transient input until
  submission and must be cleared immediately afterward.
- Selected or dropped auth files are read in Rust. A batch is capped at 64
  files and 64 MiB in aggregate, retains the 10 MiB per-file limit, and parses
  all readable files before sequential credential validation. File contents,
  absolute paths, and individual failure details are not placed in WebView
  storage or returned in the aggregate result.
- Import support for Cockpit Tools, Sub2API, and CPA applies to files the user
  explicitly selects or drops. GSwitch does not inspect another application's
  account storage automatically. After the user explicitly starts **Find on
  this computer**, it may read only the documented Official Codex profile
  and Cockpit allowlist, including the existing secure-storage key needed to
  decode a supported Codex detail. The read is bounded and strictly
  read-only: no key creation, rotation, repair, source rewrite, watcher,
  scheduler, or generic recursive search is allowed. A changed source
  identity invalidates the preview before normal intake runs. When a portable
  Cockpit export or local record includes non-Codex account metadata, GSwitch
  drops password, 2FA, note, phone, tag, group, provider, and mail-setting
  fields.
- Do not write credentials, raw provider payloads, reset-credit IDs, or account
  secrets to logs, telemetry, issue-report output, or user-facing errors.
- The production WebView CSP permits bundled local assets and Tauri IPC only.
  It does not grant browser-network, shell, filesystem, or path-opening access.
  OAuth URLs are opened by a Rust command after the corresponding in-memory
  login is checked.
- Credential-bearing files use atomic replacement and restrictive permissions
  where the operating system and filesystem support them.
- Portable export is explicit credential egress: Rust validates selected saved
  IDs, opens the native save dialog, and writes only after the user accepts the
  unencrypted-export warning. The WebView receives neither credential content
  nor destination path. The versioned format excludes account operational
  state, reset/recovery IDs, provider payloads, paths, and source metadata.

The GSwitch metadata store and encrypted vault live under the application's
config directory. The versioned JSON store owns a handful of profile projections,
not credential material or relational data. Do not add a database or export
subsystem without a concrete accepted need.

## Process boundary

A running Codex process may retain old credentials in memory or write state
after a file replacement. Before any live credential mutation, GSwitch detects
known Codex Desktop, CLI, App Server, and IDE-owned processes. A relevant process
whose command line cannot be inspected is unsafe, not absent.

The switch operation fails before any provider request and tells the user to
quit Codex. GSwitch does not silently kill, restart, or manage external Codex
processes. Switching checks again before an authentication-only managed refresh
and immediately before replacement to narrow both race windows.

GSwitch-owned isolated App Server children are scoped to their operation and
excluded only from that operation's external-process check. They are terminated
when the operation ends.

Saving the current file-backed ChatGPT account uses the live access-token
snapshot for a Rust-only read-only account check. It matches the returned
workspace to the locally derived stable identity, rereads the live credential
before saving, and revalidates once if Codex has written a newer document for
the same identity. A changed identity or another change aborts. The action
never rewrites live `auth.json` or starts an isolated managed refresh, so it
remains available while Codex is running. Copying a live credential into an
isolated profile would not preserve refresh-token ownership if that profile
requested a managed refresh. API-key intake retains its existing isolated
non-refreshing check; switching and other live credential mutations retain the
external-process guard.

## Storage and concurrency

Credential-affecting operations are serialized by an in-process mutex and a
cross-process file lock. A new secret generation is durably written and
readable in the vault before metadata points at it; metadata commits before an
old reachable generation can be retired. Persist metadata successfully before
changing its in-memory projection. Startup hydration and legacy migration hold
the same cross-process operation lock through their metadata commit.

Atomic writes protect against partial files; they do not by themselves prove
that the data being written is current. Every mutation must also verify identity
and expected prior state.

## Switching invariants

Before replacing live credentials:

1. the effective store must be confirmed as file-backed and unmanaged;
2. no external Codex runtime may be active or uninspectable, and that check must
   happen before target-network validation;
3. the current live identity must be saved or the live profile must be empty;
4. the newest live credential must be reconciled into its saved profile;
5. a ChatGPT target must pass a read-only account check with its saved snapshot,
   or an API-key target must pass local structure and stable-identity checks;
6. a ChatGPT 401 or 403 may use one isolated managed refresh after another
   external-process check, but rate limits, transport, TLS, timeout, 5xx, and
   parse failures must not enter that fallback;
7. any refreshed credential must still match the saved identity before it is
   persisted;
8. pending recovery intent must be durably stored;
9. the external process state and live credential fingerprint must still match
   the preflight observations.

After replacement, success requires a local reread of `auth.json` to derive the
requested identity. That check does not start App Server or make a provider
request. Rollback is allowed only if the live file still matches the credential
GSwitch wrote. Otherwise fail closed and require recovery. A stale saved refresh
token must never overwrite a newer live token.

Changing Codex credential-store policy is explicit. Never extract keyring or
ephemeral credentials, bypass managed policy, or assume that writing
`auth.json` changes the effective identity when another store is authoritative.

## Isolated-operation invariants

OAuth, authentication-only switch/import/quota fallback, and reset redemption
use short-lived GSwitch-owned Codex profiles. Ordinary ChatGPT switch and import
validation use the Rust-only read-only backend client first. A valid switch
snapshot may update normalized non-secret metadata but must not refresh or
rewrite the saved credential. Before persisting any refreshed credential,
verify that its account kind and identity are unchanged. Delete the isolated
profile on every exit. If a refreshed credential cannot be committed to
metadata, any recovery copy must first be committed to the encrypted vault;
never leave a plaintext isolated `auth.json` as a fallback.

Ordinary quota refresh is a read-only provider projection. When Codex is
running, GSwitch rereads the file-backed live credential immediately before
the request and uses that token snapshot when its identity matches the target.
If the read gets an authentication response, it rereads once and retries only
when the same identity has a newer credential. It never writes the live file or
uses App Server for that active path. A running Codex instance on another saved
identity can still be read through the target's saved snapshot. Only an
authentication failure for an inactive account may enter the managed isolated
refresh path; 429, transport, TLS, DNS, timeout, parse, and server failures do
not.

Wake uses a Rust-owned, direct ChatGPT Codex Responses request with one
access-token snapshot. A matching running Codex identity remains eligible, but
retains sole refresh-token ownership: GSwitch rereads its live file-backed token
once before sending and never writes live `auth.json` or starts a second App
Server. A safely inactive identity may use one isolated refresh only after an
authentication failure; an unidentifiable running process is never a reason to
skip Wake, but prevents that refresh fallback. The one standard-tier text
request has no tools, project or file context, stored response, reset credit, or
Reserve use. Once it may have reached the provider, GSwitch reports uncertainty
instead of retrying.

## Recovery invariants

- A pending switch records the target, expected identity, previous active
  profile, transaction stage, and an opaque reference to protected previous
  live auth.
- A pending reset records the account, start time, and an opaque reference to
  the exact protected credit and idempotency key so retry cannot intentionally
  double-consume.
- The WebView receives only whether reset recovery is pending. It cannot read
  the recorded account, provider credit, or idempotency key.
- Ambiguous external changes are preserved, not overwritten.
- Switch errors returned to the WebView contain only a structured code for
  Codex-open, sign-in-required, file-store-required, credentials-changed,
  recovery-required, or verification-failed. Detailed provider, filesystem,
  and recovery errors stay in Rust.
- A failed account-store write must not silently discard a credential refreshed
  by Codex: keep a recovery copy in the encrypted vault when possible. If that
  write also fails, remove the isolated profile rather than retain plaintext
  `auth.json` as a fallback.
- An explicit pending-credential recovery reads the encrypted index only in
  Rust. It derives the document identity again, accepts an existing account
  only at the recorded generation, and refuses to overwrite a newer secret.
  It commits metadata before retiring the queue entry, so interruption can be
  retried. A document that fails the intended identity check remains protected
  but cannot be applied as another account. IPC receives only a count, never a
  credential or vault reference.
- OAuth ends its App Server process before attempting to delete the isolated
  profile. Failure to remove that plaintext profile is reported as failure,
  not hidden by its destructor.
- A damaged metadata store, unavailable/locked vault, missing metadata secret
  reference, or incomplete legacy migration blocks mutation. None becomes an
  empty library or plaintext fallback. Metadata reset preserves the damaged
  file and never changes `CODEX_HOME/auth.json`. The recovery-only UI disables
  account intake, switching, quota refresh, reset credits, and Wake until the
  user explicitly confirms that GSwitch-only reset.
- Removing a saved account never removes the live Codex login.

Unknown, missing, stale, timed-out, or conflicting state remains unknown. Never
turn it into invented success.
