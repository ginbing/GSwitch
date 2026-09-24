# GSwitch product

GSwitch is a local desktop utility for people who personally use multiple Codex
accounts.

Its durable product rule is:

> **GSwitch is a Codex switcher that stays a switcher.**

The product makes a small set of account jobs fast, clear, and reliable without
requiring the user to understand credential files, token refresh, process state,
or Codex runtime internals.

## Product outcome

A user should be able to open GSwitch, understand the active account and useful
capacity state, perform one explicit account action, and return to Codex.

GSwitch may:

- save the user's own Codex accounts locally;
- show the effective active identity;
- show useful five-hour, weekly, reset, and reset-credit state supplied by Codex;
- switch the effective Codex identity safely;
- import a complete Codex auth document to move an account between machines;
- consume an eligible reset credit only after an explicit user action;
- Wake one or more eligible accounts only after an explicit user action;
- expose the minimum settings and recovery actions required by those jobs.
- show the Codex CLI that GSwitch uses and request its official update when a
  local CLI problem prevents those jobs.

Multiple intake paths still end in one local saved-account model. They do not
create a general provider platform.

## Product behavior

- Confirm real runtime state instead of reporting optimistic success.
- Treat unknown or stale state as unknown or stale.
- Treat account capacity as operational state, not analytics.
- Keep technical credential details out of the normal user flow.
- Make destructive or capacity-consuming actions explicit.
- Prefer one direct action over a new management surface.

## Anti-expansion boundary

GSwitch does not become:

- a reverse proxy, relay, request router, or traffic balancer;
- an automatic account rotator, account pool, or team-sharing service;
- a multi-instance orchestrator, daemon, local API, or remote-control service;
- an API gateway or third-party provider aggregator;
- an MCP, Skills, thread, or session-management platform;
- a cloud account control plane;
- a background scheduler that mutates account state or capacity;
- a telemetry or account-analytics product.

A capability belongs only when it directly reduces the time, steps, or
uncertainty involved in understanding account state, switching or migrating an
account, using expiring reset capacity, or explicitly waking an account.

If a proposal needs a platform layer, routing system, background service,
database, or separate management surface, it requires a new product decision.
Do not introduce its infrastructure incidentally.

Exact executable behavior belongs to current code and tests. This document owns
the durable product purpose and boundary, not a roadmap or release status.
