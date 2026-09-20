# GSwitch documentation

This is the sole router for durable GSwitch documentation. Read only the
smallest owner needed for the task.

| Need | Canonical owner |
| --- | --- |
| Product purpose and anti-expansion boundary | [`PRODUCT.md`](../PRODUCT.md) |
| Runtime architecture, module boundaries, and state ownership | [`architecture.md`](./architecture.md) |
| OAuth, import, switching, quota, reset-credit, and Wake semantics | [`workflows.md`](./workflows.md) |
| Single-window UI and anti-overdesign rules | [`design.md`](./design.md) |
| Credential, process, mutation, and recovery invariants | [`security.md`](./security.md) |
| Test commands, CI, and proof boundaries | [`testing.md`](./testing.md) |
| Windows/macOS packaging, signing, and release gates | [`release.md`](./release.md) |

## Documentation model

- Repository Markdown owns durable product and architecture meaning.
- Source, configuration, tests, generated artifacts, and observed Codex behavior
  own exact executable facts for a particular revision.
- Live provider and operating-system state stays with its provider. Do not turn
  a dynamic observation into an undated repository claim.
- Issues own current bugs, features, and bounded tasks only.

When a durable decision changes, edit the smallest existing owner. Delete
obsolete prose because Git retains history. Do not create another roadmap,
strategy document, status ledger, or documentation index.
