# GSwitch ownership map

This file records the durable authority boundaries that keep account operations safe.

## Authority

- The maintainer's current instruction owns product acceptance and scope.
- Current executable behavior is established by the code and the Codex runtime it interacts with.
- Durable product and architecture meaning lives in the smallest document routed by `docs/README.md`.
- External provider facts remain owned by Codex/OpenAI responses; GSwitch may project them but must not invent them.

## State ownership

| Concern | Canonical owner | GSwitch responsibility |
| --- | --- | --- |
| Effective live Codex identity | Codex runtime | Inspect, match, and verify before reporting active state |
| Live file-backed credential document | `CODEX_HOME` | Preserve the complete document and replace it only through the guarded switch transaction |
| Saved account profiles | GSwitch account store | Persist complete credentials plus minimum non-secret identity metadata |
| Credential-store policy | Codex configuration / managed policy | Read effective policy; never silently override it |
| OAuth/session refresh behavior | Codex | Use official Codex flows and preserve refreshed credentials |
| External Codex process state | Operating system runtime | Detect known Codex runtimes before live credential mutation |
| Quota, reset timing, reset credits | Codex/OpenAI provider state | Read and present; never infer missing provider facts |
| Pending switch recovery | GSwitch account store | Record and resolve enough state to avoid destructive ambiguity |

## Mutation rules

Credential-changing operations are Rust-owned and serialized.

The frontend does not:

- read or write credential documents;
- decide whether a credential replacement is safe;
- orchestrate the steps of a switch transaction;
- store secrets in browser storage.

A second durable writer, shadow credential store, proxy, daemon, or remote control path requires an explicit ownership decision before implementation.

## Unknown state

Unknown, stale, timed-out, or conflicting state remains unknown.

Do not turn an unreadable credential, ambiguous process result, failed verification, or missing provider response into a guessed account state.
