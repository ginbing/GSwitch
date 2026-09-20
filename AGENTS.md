# AGENTS.md

GSwitch is a small local Codex account utility. Keep both the product and the
implementation small.

## Authority

The maintainer's current request owns task scope and product acceptance.
Current executable behavior is established by the Codex/runtime behavior being
used, then the consumed source and configuration, then behavioral tests, then
prose. Durable product and architecture meaning belongs in the smallest owner
routed by `docs/README.md`.

Issues own current work only. They are not product acceptance, durable product
or architecture authority, a roadmap archive, an engineering notebook, or a PR
strategy ledger. Closed Issues and old PR discussions are historical evidence.
Do not reconstruct current GSwitch policy from Issue history when code and
committed docs answer the question.

When a durable decision changes, update its owning document in the same change.
Do not create a second docs index, strategy file, status layer, or truth store.

## Documentation routing

Start with the task evidence. Read `docs/README.md` only when durable meaning or
a procedure is needed, then read the smallest matching owner.

- New capability or scope question: `PRODUCT.md`.
- Tauri/Rust/React boundaries or state ownership: `docs/architecture.md`.
- OAuth, import, switch, quota, reset, or Wake: `docs/workflows.md`.
- UI structure or visual behavior: `docs/design.md`.
- Credentials, processes, mutation, or recovery: `docs/security.md`.
- Tests or CI claims: `docs/testing.md`.
- Packaging, signing, versioning, or distribution: `docs/release.md`.

## Working rules

- Inspect current source and make the smallest complete change. Preserve
  unrelated user edits.
- Prefer direct behavior, explicit state, official Codex and Tauri interfaces,
  narrow dependencies, and one owner per durable fact.
- Rust owns credentials, filesystem mutation, process checks, Codex runtime
  interaction, persistence, and safety-critical state transitions.
- The WebView is presentation. Stored credential documents, reset-credit IDs,
  and raw provider payloads must not enter frontend state, browser storage, or
  logs. User-entered secret fields must be transient and cleared after use.
- Prefer the flat module structure. Split only when a module has acquired two
  real responsibilities.
- Do not add speculative layers, provider frameworks, a database, proxy/router
  behavior, a background daemon, automatic process killing, or unnecessary UI
  structure.
- Before nontrivial implementation, state the short plan, expected files, and
  whether durable docs change.

## Issues and completion

Use an Issue only for a current bug, feature, or bounded task that benefits from
shared tracking. Keep its body focused on the remaining outcome and acceptance
evidence; link to canonical docs instead of copying the product or architecture
contract into it.

After implementation, reconcile the affected Issue against current behavior.
Close it when the tracked work is satisfied; if work remains, leave only the
current gap and point durable facts to their repository owner.

## Validation

Run the smallest relevant proof while editing and the repository CI path before
merge. Credential, state, and capacity mutations require regression coverage.
Never claim a platform, provider, installer, or recovery path was tested unless
that exact check ran. See `docs/testing.md`.
