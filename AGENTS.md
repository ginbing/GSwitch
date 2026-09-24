# AGENTS.md

GSwitch is a small, local-first Codex account utility. Keep both the product
and its implementation deliberately small.

## Authority by proposition

The maintainer's current request owns task scope, product acceptance, public
copy, visual taste, and semantic-ownership changes. A document, Issue, test,
review, or agent statement cannot create that acceptance by itself.

Resolve other claims by what they describe:

- **Current executable behavior:** observed Codex/runtime or operating-system
  behavior, then the code, configuration, schema, or generated artifact it
  actually consumes, then behavioral tests, then prose.
- **Durable product and architecture meaning:** the maintainer's current
  decision, then the smallest canonical document routed by `docs/README.md`,
  then source as evidence or inference.
- **Task procedure:** the current executable command, workflow, and official
  provider contract, with the matching local document explaining inputs,
  actions, proof, and stop points.
- **Dynamic operational state:** the live provider, OS, GitHub, or private
  evidence—not an undated repository assertion.

Existing patterns are preferences, not authority over those sources. Issues
own current shared work only; they are not product acceptance, a roadmap
archive, an engineering notebook, a PR-strategy ledger, or durable product
truth. Closed Issues and old PRs are historical evidence only.

When a durable decision changes, update its existing owner in the same change.
Do not create a second documentation index, strategy/status store, mutation
path, account mirror, or parallel source of truth.

## Working rules

- Inspect the current source and relevant runtime evidence, then make the
  smallest complete change. Preserve unrelated user edits and stage only
  task-owned files.
- Prefer direct behavior, explicit state, official Codex and Tauri interfaces,
  narrow dependencies, and one owner per durable fact. Do not add speculative
  compatibility paths, a provider framework, database, proxy/router, daemon,
  scheduler, automatic account rotation, or unnecessary UI structure.
- Rust owns credentials, filesystem mutation, process checks, Codex-runtime
  interaction, persistence, and safety-critical state transitions. The WebView
  is presentation: credential documents, reset-credit IDs, and raw provider
  payloads must not enter frontend state, browser storage, or logs. Secret form
  inputs are transient and cleared after submission.
- Keep the flat module structure unless a module has acquired two real,
  independent responsibilities. Do not add an abstraction merely because one
  might be useful later.
- When the maintainer asks for a fresh redesign, treat current UI, CSS,
  screenshots, generated concepts, Issues, PRs, and snapshot tests as evidence
  of existing behavior and content—not as visual direction. Do not infer or
  persist aesthetic preferences unless the maintainer accepts durable design
  guidance.
- Before wrapping or extending Codex, Tauri, or another pinned framework or
  provider, inspect current official documentation and installed source/types.
  Prefer documented extension points. A fork, monkey patch, vendored internal,
  or replacement for supported behavior needs separate maintainer approval.
- Never force-push, rewrite history, push a protected branch, or stage
  unrelated work. Use branch and PR metadata that describes the accepted
  product change.

## Documentation routing

Start from the task evidence. Read `docs/README.md` only when durable meaning
or a procedure is needed, then read the smallest matching owner.

- New capability or scope question: `PRODUCT.md`.
- Tauri/Rust/React boundaries or state ownership: `docs/architecture.md`.
- OAuth, import, switch, quota, reset, or Wake: `docs/workflows.md`.
- UI structure or visual behavior: `docs/design.md`.
- Credentials, processes, mutation, or recovery: `docs/security.md`.
- Tests or CI claims: `docs/testing.md`.
- Packaging, signing, versioning, or distribution: `docs/release.md`.

When a task exposes a reusable engineering decision, consult only the smallest
matching [Ginbing Playbook](https://github.com/ginbing/ginbing-playbook) guide
and verify it against GSwitch's local documents and current runtime behavior.
The Playbook is guidance, not local product authority.

Before nontrivial implementation, state a short plan, expected files, and
whether durable documentation changes. For work crossing credential state,
provider behavior, persistence, recovery, concurrency, or a second source of
truth, include the affected owner, mutation path, failure mode, and behavioral
scenarios in that plan. GSwitch has no cross-repository ownership map; do not
invent one just to satisfy a process rule.

## External and irreversible actions

Keep secrets out of chat, logs, committed files, provider error bodies, and
frontend state. Local code changes, inspection, and safe test doubles do not
authorize an external provider action.

Explicit maintainer confirmation naming the target and action is required for:

- redeeming a real reset credit or any other billable/irreversible provider
  action;
- deleting or overwriting user data outside GSwitch-owned storage;
- production/release signing, notarization, publication, deployment, backup,
  or restore actions.

OAuth browser sign-in, a user-selected credential import, and a GitHub branch
or PR action may proceed when the maintainer has directly requested that flow.
Never use a user's existing private Cockpit Tools storage as an import source:
only import files the user explicitly exports or selects.

## Issues and completion

Use an Issue only for a current bug, feature, or bounded task that benefits
from shared tracking. Before changing a selected Issue, read its current body,
comments, metadata, and relationships. When the maintainer names a task,
execute it without scanning the Issue queue.

Write GitHub Issue titles, bodies, and comments in English.

After implementation, reconcile the affected Issue against current behavior.
Close it only when its acceptance evidence is satisfied; otherwise retain only
the remaining gap and point durable facts to their canonical owner.

## Validation and handoff

Run focused checks while editing, then the smallest complete local proof for
the changed boundary before handoff. Credential, provider-state, storage,
recovery, and capacity mutations require regression coverage. Validate real
boundaries—provider protocol, filesystem behavior, process safety, and rendered
user flows—where relevant. Do not claim a check ran unless it did.

Use `docs/testing.md` for the current local and CI procedures. Read the active
GitHub ruleset for merge requirements; do not infer live required checks from
this file. CI green alone does not prove the product, provider, installer,
signing, or recovery claim.

After implementation, report changed behavior, actual validation, limitations,
commits, and any required user handoff. Once the accepted claim has the
required proof, stop unless a new change, failure, or unresolved risk changes
what must be proven.
