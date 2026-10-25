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
credential document. Switch and reset-credit failures cross IPC as a small
serialized error code, not a Rust display string or raw provider error.

## State ownership

| State | Canonical owner | GSwitch responsibility |
| --- | --- | --- |
| Effective live Codex identity | Codex runtime | Inspect, match, and verify before reporting it |
| Live file-backed credential | `CODEX_HOME/auth.json` | Preserve the complete document and replace it only through the switch transaction |
| Credential-store policy | Effective Codex configuration or managed policy | Read and verify it; never silently override it |
| Saved accounts and recovery secrets | Rust-owned, owner-only `accounts.json` | Persist identity, display fields, quota summaries, active state, complete credential documents, reset IDs, idempotency keys, and rollback material in one atomic file |
| Account-store health | Rust-owned recovery state | Open a recovery-only workspace when the store cannot be read; never infer an empty library |
| OAuth and managed token refresh | Codex | Use official flows and preserve refreshed complete documents |
| Read-only account metadata | ChatGPT backend response | Match the returned workspace entry and cache only normalized fields |
| Quota and reset-credit facts | Codex/OpenAI response | Normalize and cache them without inventing missing values |
| External Codex process state | Operating system | Detect known or uninspectable runtimes before sensitive operations |
| Pending switch/reset recovery | GSwitch account store | Persist enough intent to resume safely and idempotently |
| Screen and dialog state | React component tree | Keep it transient, derived, and free of stored secrets; selected saved-account IDs are permitted, credentials and export paths are not |

Unknown, stale, timed-out, or conflicting state remains unknown. A cache is a
projection, not a second authority over runtime or provider facts.

The initial workspace snapshot names the saved account of a reset that needs
recovery and whether retrying can no longer help. The provider credit and
idempotency key remain in Rust-owned storage.

Interrupted credential saves use separate owner-only files in
`pending-credentials/`, so a recovery copy does not depend on the account file
being writable. The explicit recovery command returns only a recovered count;
Rust verifies each document's stable identity and the recorded account
generation before a store commit, then deletes the entry. Neither the entries
nor their references are IPC values.

## Rust module boundaries

Keep the backend flat and organized by concrete responsibility:

- `lib.rs`: Tauri setup, managed state, plugins, and command registration;
- `commands.rs`: thin IPC adapters, sanitized application snapshots, and the
  `spawn_blocking` scheduling boundary for filesystem, provider, process, and
  App Server work;
- `accounts.rs`: saved profiles, credential generations, operation
  serialization, store loading and migration, damaged-store recovery, pending
  credential recovery, and in-memory operation state;
- `app_server.rs`: lifecycle and protocol boundary for the official Codex App
  Server;
- `isolated_profile.rs`: short-lived isolated Codex homes, removed when their
  operation ends or, failing that, at the next start;
- `chatgpt.rs`: read-only ChatGPT quota and account-metadata HTTP boundary;
- `codex.rs`: `CODEX_HOME`, effective storage mode, and live auth-file access;
- `identity.rs`: credential classification, stable non-secret identity, and
  fingerprints used for comparisons;
- `intake.rs`: OAuth, bounded auth-document batch import, versioned portable export serialization, and API-key intake;
- `migration.rs`: one-shot, allowlisted local Codex/Cockpit discovery,
  read-only envelope decoding, sanitized previews, and revalidated handoff to
  `intake.rs`;
- `switching.rs`: live-account reconciliation, file-store enablement, switching,
  removal, and interrupted-switch recovery;
- `quota.rs`: quota normalization/cache, the managed refresh shared by switch,
  quota, and Wake, reset-credit selection, redemption, and redemption recovery;
- `wake.rs`: Wake policy, narrow Responses transport, sequential Wake All,
  cancellation, and per-account outcomes;
- `runtime.rs`: external Codex process detection;
- `owned_process.rs`: scoped helper process groups/jobs, bounded termination,
  and the fail-closed state after unconfirmed cleanup;
- `storage.rs`: versioned JSON persistence, pending credential files, and
  atomic owner-only writes;
- `vault.rs`: read-only access to the Stronghold vault of GSwitch 1.0.30 and
  earlier, used once to migrate it, and its removal;
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

`App` owns the workspace snapshot/account topology, current account, selection,
dialog entry points and notices. `AccountIntake.tsx` owns login/import/discovery
state, requests, errors, drag-and-drop registration and the add-account view.
`Wake.tsx` owns Wake starts, cancellation, polling and per-account outcomes.
`Update.tsx` owns the signed-update check schedule, the offered update and its
install, restart and notice.
Their hooks stay mounted independently of their dialogs. Polling is keyed by
operation ID and phase. Closing a pending login cancels it; an accepted login
continues saving. Wake continues when its details close.

`useAccountQuota` owns the transient quota/error view, request coalescing,
concurrency lanes, attempt timing, automatic checks and batch progress. Cache,
refresh and reset projections share one acceptance path: current membership and
credential revision, request identity and timestamps decide which results can
apply. Removed/superseded work cannot restore data or errors. Reset and re-login
invalidate earlier requests; post-login reads wait for the matching account
revision. A new revision, which Rust also issues when saved reset credits
change, supersedes requests and errors but leaves the last reading current.
This is a projection of Rust facts, not another persisted account store.

`AccountCard` owns presentation and its menu/tooltip interactions, and reports
intent through callbacks. Shared popover behavior remains a local component.

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
after checking the saved credential generation. Wake requests likewise hold no
lock; Wake waits for it only to run an isolated refresh or to mark the rejected
credential generation as needing sign-in. React keeps point-operation state local: a single-account quota refresh
replaces only that account's quota projection, while a full workspace reload is
reserved for initial state or a real topology change. When a reload reads saved
projections, a newer saved projection replaces the in-memory one and an older
one does not.

`accounts.json` version 5 keeps each complete credential document beside its
account record, as Codex keeps its own `auth.json`: plain JSON in an owner-only
file. Every change is one atomic replacement, and a changed credential advances
its account's credential generation. On Unix the file is `0600` in a `0700`
directory; on Windows it keeps the per-user permissions of the application
config directory. The live Codex `auth.json` remains a distinct file-backed
projection, never proof of the saved account.

Version-4 stores from GSwitch 1.0.30 and earlier reference a Stronghold
snapshot whose root key is held by the platform credential manager. At startup,
under the operation lock, GSwitch reads every referenced record and pending
entry from one snapshot session, verifies each credential's identity, writes
the version-5 file and pending files, rereads both, and only then retires the
snapshot: it is renamed, its root key is deleted, and the renamed file is
deleted last, so a cleanup interrupted after the rename finishes at a later
start; a snapshot that cannot be renamed stays in place. A vault that cannot
be read leaves both files unchanged and the library unavailable until a later
start. A snapshot that was not migrated this way, such as one beside a store
reset after a failed migration, is kept unread. Stores from before the vault
already keep credentials inline and are verified and rewritten as version 5.
Removing Stronghold, `keyring` and the two patched crates below ends
this direct migration path.

## Isolated Codex profiles

OAuth, authentication-only switch/import/quota fallback, and reset redemption
may run an official Codex App Server in a short-lived GSwitch-owned
`CODEX_HOME`. This isolates the operation from the user's live Codex identity.
Each profile is deleted when its operation ends; the next start removes any
profile that a forced stop or a still-open file left behind. Refreshed
credentials are accepted only after the returned document still matches the
expected account identity. Ordinary switch validation, quota,
account metadata, and Wake use the small Rust-owned ChatGPT HTTP boundary first.
A valid switch snapshot updates only normalized non-secret metadata; it neither
refreshes nor rewrites credentials. API-key switching does no provider request.

## Mutation boundary

Operations that may persist or refresh credentials share one Rust-owned
in-process mutex and cross-process file lock. Only one GSwitch process runs at a
time: each process loads the account store into memory once, so a second launch
brings the running window forward and exits instead of keeping its own copy.
Live switching adds external Codex
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

Keep Tauri's JavaScript API and Rust crate on the same minor release, and paired
plugin packages on the same exact version, following the official
[dependency update contract](https://v2.tauri.app/develop/updating-dependencies/#sync-npm-packages-and-cargo-crates-versions).
The Tauri CLI build validates the resolved pairs; do not bypass its mismatch check.

The Cargo patch for `libsodium-sys-stable` preserves the published 1.24.0
source (upstream revision `b17dd40bd8ff878a308b79423efc0a086ee2e7d3`,
crates.io checksum `72b04bf6da2c98b727af37ab62cb505f4d751b975b034a9b9ad491d333b0564e`).
The library and build source files are unchanged; the build dependency's ZIP
version requirement moves from 8.5 to 9 because released upstream versions
still require a vulnerable ZIP major. Upstream integration tests and examples
are omitted with their manifest targets because GSwitch consumes this package
only as a library. The original signed libsodium archives and their verification
code stay byte-for-byte intact; no vault format or credential path changes. Cargo's
lock file owns the resolved dependency versions. Remove this patch when a
compatible upstream release uses a fixed ZIP dependency, or with Stronghold.

The maintainer-approved `iota-crypto` patch retains the published 0.23.2 source
(upstream `8c13125541e762206e2dc57b8bfde89c7f6ce8e3`, crate SHA-256
`98a38db844c910d78825e173c083f2ef416b69cb091bba8ac1055763c6db065b`).
Its k256 dependency moves to 0.14 so the locked graph can use crypto-bigint
0.7.6 or newer. Only the secp256k1 and SLIP10 API call sites adapt to the new
upstream names, return types, and RNG trait; key generation and cryptographic
arithmetic remain upstream implementations. The RNG bridge preserves the old
caller interface and forwards bytes without changing the entropy source.
Stronghold, snapshot encryption, credential metadata, and key derivation are
unchanged. Published upstream vectors and a synthetic snapshot generated with
unmodified Stronghold verify the boundary. Remove this patch when a compatible
published IOTA release adopts the fixed dependency chain, or with Stronghold.
