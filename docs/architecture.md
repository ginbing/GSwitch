# Architecture

GSwitch is a local Tauri 2 desktop application. Its architecture is deliberately
smaller than a general account-management platform.

## Runtime boundary

- Rust is the trusted application boundary. It owns credentials, Codex App
  Server interaction, filesystem mutation, process checks, persistence, and
  safety-critical state transitions.
- React and TypeScript own presentation, transient form state, and direct user
  interaction.
- Vite builds the frontend; local CSS defines the small desktop design system.
- The WebView calls explicit Tauri commands. GSwitch has no local HTTP API,
  listening port, daemon, or remote-control surface.

The frontend asks for user-level actions such as add, switch, refresh, redeem,
or Wake. It does not orchestrate their internal steps or receive the stored
credential document. Switch failures cross IPC as a small serialized error code,
not a Rust display string or raw provider error.

## State ownership

| State | Canonical owner | GSwitch responsibility |
| --- | --- | --- |
| Effective live Codex identity | Codex runtime | Inspect, match, and verify before reporting it |
| Live file-backed credential | `CODEX_HOME/auth.json` | Preserve the complete document and replace it only through the switch transaction |
| Credential-store policy | Effective Codex configuration or managed policy | Read and verify it; never silently override it |
| Saved account metadata | Rust-owned `accounts.json` | Persist identity, display fields, quota summaries, active state, and opaque credential references only |
| Saved credential and recovery secrets | Rust-owned Stronghold vault | Preserve complete credential documents, reset IDs, idempotency keys, and rollback material behind a native-credential-manager-protected root key |
| Account-store health | Rust-owned recovery state | Open a recovery-only workspace when the store cannot be read; never infer an empty library |
| OAuth and managed token refresh | Codex | Use official flows and preserve refreshed complete documents |
| Read-only account metadata | ChatGPT backend response | Match the returned workspace entry and cache only normalized fields |
| Quota and reset-credit facts | Codex/OpenAI response | Normalize and cache them without inventing missing values |
| External Codex process state | Operating system | Detect known or uninspectable runtimes before sensitive operations |
| Pending switch/reset recovery | GSwitch account store | Persist enough intent to resume safely and idempotently |
| Screen and dialog state | React component tree | Keep it transient, derived, and free of stored secrets; selected saved-account IDs are permitted, credentials and export paths are not |

Unknown, stale, timed-out, or conflicting state remains unknown. A cache is a
projection, not a second authority over runtime or provider facts.

The initial workspace snapshot may reveal only a boolean that reset-credit
recovery needs attention. Its account, provider credit, and idempotency key
remain in Rust-owned storage.

Interrupted credential saves use a separate encrypted pending index. The
explicit recovery command returns only a recovered count; Rust verifies each
document's stable identity and the recorded account generation before a
metadata commit, then retires the protected entry. Neither the index nor its
contents is an IPC value.

## Rust module boundaries

Keep the backend flat and organized by concrete responsibility:

- `lib.rs`: Tauri setup, managed state, plugins, and command registration;
- `commands.rs`: thin IPC adapters, sanitized application snapshots, and the
  `spawn_blocking` scheduling boundary for filesystem, provider, process, and
  App Server work;
- `accounts.rs`: saved-profile metadata, secret-generation transactions,
  operation serialization, store coordination,
  damaged-store recovery, pending credential recovery, and in-memory operation
  state;
- `app_server.rs`: lifecycle and protocol boundary for the official Codex App
  Server;
- `chatgpt.rs`: read-only ChatGPT quota and account-metadata HTTP boundary;
- `codex.rs`: `CODEX_HOME`, effective storage mode, and live auth-file access;
- `cli_update.rs`: version inspection and explicit handoff to the installed
  Codex CLI's official update command;
- `identity.rs`: credential classification, stable non-secret identity, and
  fingerprints used for comparisons;
- `intake.rs`: OAuth, bounded auth-document batch import, versioned portable export serialization, and API-key intake;
- `migration.rs`: one-shot, allowlisted local Codex/Cockpit discovery,
  read-only envelope decoding, sanitized previews, and revalidated handoff to
  `intake.rs`;
- `switching.rs`: live-account reconciliation, file-store enablement, switching,
  removal, and interrupted-switch recovery;
- `quota.rs`: quota normalization/cache, reset-credit selection, redemption,
  and redemption recovery;
- `wake.rs`: Wake policy, narrow Responses transport, sequential Wake All,
  cancellation, and per-account outcomes;
- `runtime.rs`: external Codex process detection;
- `storage.rs`: versioned JSON persistence and atomic/private writes;
- `vault.rs`: Rust-only Stronghold snapshot and native root-key boundary for
  credential and recovery secrets;
- `types.rs`: backend state and sanitized serializable view models.

Split a module only after it has acquired two real responsibilities. Do not add
`services/`, `repositories/`, `domain/`, `providers/`, dependency injection,
SQLite, or an ORM for hypothetical growth.

## Frontend boundary

The application remains a one-screen React app with a small typed Tauri adapter.
Use React's normal state model: one owner for each state value, derived data
instead of copies, props for controlled child views, and local state for dialogs
and forms. A global store, router, component framework, or generic API client
requires a demonstrated current need.

The native dialog is used to choose account-export paths or, after an explicit
user action, one documented Cockpit data folder. Rust reads selected import
paths and owns portable export lookup, serialization, destination, and write;
it returns only sanitized import summaries, local-account previews, and export
counts. Raw file bytes, source records, paths, keys, and credential documents
do not cross the WebView boundary. File-count, per-file, and aggregate-size
limits plus identity deduplication belong to Rust rather than React. Local
migration is a one-shot command with no startup scan, watcher, scheduler, or
background job. OAuth links are short-lived, user-visible links associated with
an in-memory login session.

The WebView may persist its selected display language only. Language selection
is not account state and must not share storage with credentials, provider data,
or recovery records.

Potentially blocking Rust commands are asynchronous and move their existing
synchronous domain operation to Tauri's blocking worker pool. This keeps the
window able to repaint, scroll, and accept unrelated input without weakening
the Rust operation mutex or cross-process lock. Automatic quota reads release
that lock during network waiting and reacquire it only to commit a projection
after checking the saved credential generation. Startup reads account secrets
from one Stronghold session instead of reopening the snapshot for every card.
React keeps point-operation state local: a single-account quota refresh
replaces only that account's quota projection, while a full workspace reload is
reserved for initial state or a real topology change.

`accounts.json` is metadata, not a vault. Its version-4 account records carry
an opaque credential reference and generation; complete documents and
credential-adjacent recovery values are loaded only inside Rust from the
Stronghold snapshot. The snapshot is encrypted with a random root key held by
the platform credential manager. Vault writes commit before metadata advances
to a new generation; a failed metadata commit can leave only unreachable
encrypted material, never a metadata reference to an unwritten secret. Legacy
inline stores migrate atomically after every saved secret is readable and still
derives its recorded identity. The live Codex `auth.json` remains a distinct
file-backed projection, never proof of vault state.

## Isolated Codex profiles

OAuth, authentication-only switch/import/quota fallback, and reset redemption
may run an official Codex App Server in a short-lived GSwitch-owned
`CODEX_HOME`. This isolates the operation from the user's live Codex identity.
Refreshed credentials are accepted only after the returned document still
matches the expected account identity. Ordinary switch validation, quota,
account metadata, and Wake use the small Rust-owned ChatGPT HTTP boundary first.
A valid switch snapshot updates only normalized non-secret metadata; it neither
refreshes nor rewrites credentials. API-key switching does no provider request.

## Mutation boundary

Operations that may persist or refresh credentials share one Rust-owned
in-process mutex and cross-process file lock. Live switching adds external Codex
process checks because a GSwitch lock has no authority over another Codex
process. The first check precedes provider work; a second check plus the original
live fingerprint gates the atomic `auth.json` replacement.

A switch is one backend transaction, not a frontend sequence or file-copy
shortcut. It validates a saved snapshot first, permits one isolated managed
refresh only after a ChatGPT 401 or 403, and verifies the write by locally
rereading identity without starting another App Server. Pending state, guarded
rollback, and recovery remain inside the same transaction. See
[workflows.md](./workflows.md) for its semantics and [security.md](./security.md)
for its invariants.

## Dependency rule

Use official current Codex and Tauri interfaces where they exist. New
dependencies and abstractions require a present product, platform, or safety
need. Do not add infrastructure because a future feature might use it.
