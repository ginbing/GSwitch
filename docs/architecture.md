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
credential document.

## State ownership

| State | Canonical owner | GSwitch responsibility |
| --- | --- | --- |
| Effective live Codex identity | Codex runtime | Inspect, match, and verify before reporting it |
| Live file-backed credential | `CODEX_HOME/auth.json` | Preserve the complete document and replace it only through the switch transaction |
| Credential-store policy | Effective Codex configuration or managed policy | Read and verify it; never silently override it |
| Saved account profiles | Rust-owned GSwitch account store | Persist credentials plus minimum identity and cached provider projections |
| Account-store health | Rust-owned recovery state | Open a recovery-only workspace when the store cannot be read; never infer an empty library |
| OAuth and token refresh | Codex | Use official flows and preserve refreshed complete documents |
| Quota and reset-credit facts | Codex/OpenAI response | Normalize and cache them without inventing missing values |
| External Codex process state | Operating system | Detect known or uninspectable runtimes before sensitive operations |
| Pending switch/reset recovery | GSwitch account store | Persist enough intent to resume safely and idempotently |
| Screen and dialog state | React component tree | Keep it transient, derived, and free of stored secrets |

Unknown, stale, timed-out, or conflicting state remains unknown. A cache is a
projection, not a second authority over runtime or provider facts.

The initial workspace snapshot may reveal only a boolean that reset-credit
recovery needs attention. Its account, provider credit, and idempotency key
remain in Rust-owned storage.

## Rust module boundaries

Keep the backend flat and organized by concrete responsibility:

- `lib.rs`: Tauri setup, managed state, plugins, and command registration;
- `commands.rs`: thin IPC adapters and sanitized application snapshots;
- `accounts.rs`: saved profiles, operation serialization, store coordination,
  damaged-store recovery, pending credential recovery, and in-memory operation
  state;
- `app_server.rs`: lifecycle and protocol boundary for the official Codex App
  Server;
- `chatgpt.rs`: read-only ChatGPT quota and account-metadata HTTP boundary;
- `codex.rs`: `CODEX_HOME`, effective storage mode, and live auth-file access;
- `identity.rs`: credential classification, stable non-secret identity, and
  fingerprints used for comparisons;
- `intake.rs`: OAuth, auth-document import, and API-key intake;
- `switching.rs`: live-account reconciliation, file-store enablement, switching,
  removal, and interrupted-switch recovery;
- `quota.rs`: quota normalization/cache, reset-credit selection, redemption,
  and redemption recovery;
- `wake.rs`: Wake policy, isolated execution, sequential Wake All, cancellation,
  and per-account outcomes;
- `runtime.rs`: external Codex process detection;
- `storage.rs`: versioned JSON persistence and atomic/private writes;
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

The native dialog is used only to choose an account-export path. Rust reads that
path and returns a sanitized import result; raw file bytes and credential
documents do not cross the WebView boundary. OAuth links are short-lived,
user-visible links associated with an in-memory login session.

The WebView may persist its selected display language only. Language selection
is not account state and must not share storage with credentials, provider data,
or recovery records.

## Isolated Codex profiles

OAuth, imported-account validation, quota fallback, reset redemption, and Wake
may run an official Codex App Server in a short-lived GSwitch-owned
`CODEX_HOME`. This isolates the operation from the user's live Codex identity.
Refreshed credentials are accepted only after the returned document still
matches the expected account identity. Ordinary quota and account metadata
reads use the small read-only ChatGPT HTTP boundary first; that path never
starts App Server or writes a credential.

## Mutation boundary

Operations that may persist or refresh credentials share one Rust-owned
in-process mutex and cross-process file lock. Live switching adds external Codex
process checks because a GSwitch lock has no authority over another Codex
process.

A switch is one backend transaction, not a frontend sequence or file-copy
shortcut. See [workflows.md](./workflows.md) for its semantics and
[security.md](./security.md) for its invariants.

## Dependency rule

Use official current Codex and Tauri interfaces where they exist. New
dependencies and abstractions require a present product, platform, or safety
need. Do not add infrastructure because a future feature might use it.
