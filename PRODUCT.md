# GSwitch product

GSwitch is a local desktop utility for people who personally use multiple Codex accounts.

Its durable product rule is:

> **GSwitch is a Codex switcher that stays a switcher.**

The product should make a small set of account jobs fast, clear, and reliable without requiring the user to understand credential files, refresh-token rotation, process state, or Codex runtime internals.

## Product purpose

GSwitch exists to help a user:

- understand which Codex account is active;
- understand useful account capacity and reset timing;
- switch the effective Codex identity safely;
- move accounts between machines without unnecessary re-login;
- use expiring reset capacity intentionally when supported;
- explicitly Wake one or more eligible accounts when useful.

Exact executable behavior belongs to the current code and tests. This document owns product purpose and boundary.

## Experience

- Keep the main workflow short and obvious.
- Prefer one direct action over a management surface.
- Confirm real state instead of optimistically reporting success.
- Treat account capacity as operational state, not analytics.
- Keep technical credential details out of the normal user flow.
- Make destructive or capacity-consuming actions explicit.

## Intake

Useful account intake is part of the switcher:

- official ChatGPT OAuth;
- complete Codex auth-document import for migration;
- API-key intake where Codex supports it.

Several intake paths still end in one local saved-account model. They do not justify a provider platform.

## Boundaries

GSwitch does not become:

- a reverse proxy or relay;
- a request router or traffic balancer;
- an automatic account rotator;
- an account pool or team-sharing service;
- a multi-instance orchestration platform;
- a local or remote API gateway;
- a third-party provider marketplace;
- an MCP, Skills, or session-management platform;
- a cloud account control plane;
- a background scheduler that mutates account capacity without an explicit user action.

A capability belongs only when it directly reduces the time, steps, or uncertainty involved in understanding account state, switching an account, migrating an account, using an expiring reset, or explicitly waking an account.

If a proposal requires a new platform layer, routing system, background service, or separate management surface, it requires a new product decision rather than an incidental implementation change.
