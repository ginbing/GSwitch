# GSwitch documentation

This is the sole router for durable GSwitch documentation.

Read only the smallest owner needed for the task:

| Need | Owner |
| --- | --- |
| Product purpose and anti-expansion boundary | [`PRODUCT.md`](../PRODUCT.md) |
| Runtime and state ownership | [`OWNERSHIP.md`](../OWNERSHIP.md) |
| User-visible account workflows | [`product/workflows.md`](./product/workflows.md) |
| Code structure and runtime architecture | [`engineering/architecture.md`](./engineering/architecture.md) |
| Desktop UI rules | [`engineering/frontend.md`](./engineering/frontend.md) |
| Credential and recovery safety | [`engineering/security.md`](./engineering/security.md) |
| Validation commands and proof boundaries | [`engineering/testing.md`](./engineering/testing.md) |
| Packaging and public-release gates | [`operations/release.md`](./operations/release.md) |

## Documentation model

Repository Markdown owns durable product and architecture meaning.

Code, configuration, tests, and Codex runtime behavior own exact executable facts.

Issues own current work only. They should not become a second source of truth for product scope, architecture, release policy, or engineering process. Closed Issues are historical context.

When a durable decision changes, edit the smallest existing owner. Delete obsolete prose instead of creating another strategy, roadmap, status, or documentation index.
