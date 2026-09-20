# AGENTS.md

GSwitch is a small local Codex account utility. Keep both the product and the implementation small.

## Authority

The maintainer's current request owns task scope and product acceptance.

For durable meaning, start at `docs/README.md` and read only the smallest matching owner. For current behavior, inspect the executable source and tests.

Open Issues are temporary work records, not product or architecture authority. Closed Issues and old PR discussion are historical context. Do not reconstruct the product roadmap or engineering strategy from Issue history when current code and committed docs already answer the question.

When a reusable decision changes, update its owning document in the same PR.

## Working rules

Prefer:

- direct behavior;
- explicit state;
- safe credential handling;
- small reviewable changes;
- official current Codex and Tauri interfaces;
- one source of truth for each durable fact.

Avoid:

- speculative abstractions;
- provider frameworks;
- proxy/router/account-pool behavior;
- hidden credential mutation;
- automatic process killing;
- unnecessary UI structure;
- duplicate documentation.

Read `PRODUCT.md` before adding a capability and `OWNERSHIP.md` before adding a new state owner or mutation path.

## Engineering boundary

Rust owns credentials, filesystem mutation, process checks, Codex runtime interaction, persistence, and safety-critical state transitions.

The WebView is UI. Never move credential documents into frontend state, browser storage, logs, or a local web API.

Prefer the current flat module structure. Split a module only when it has acquired two real responsibilities.

## Issues and PRs

Use Issues only for current bugs, features, or bounded work that benefits from shared tracking.

Do not use Issues as a permanent product specification, architecture notebook, roadmap archive, or PR ledger.

After implementation, reconcile the affected Issue against current behavior and close it when the tracked work is satisfied.

## Validation

Before merging a code change, run the smallest relevant proof plus the repository CI path. Credential and state mutations require regression coverage.

See `docs/engineering/testing.md`.
