# Product workflows

This document owns durable user-visible workflow semantics. Current code and tests own exact executable behavior.

## Add an account

GSwitch may accept:

- official ChatGPT OAuth;
- complete Codex auth-document import;
- API-key login where Codex supports it.

OAuth should use the official Codex-managed login flow rather than a parallel OAuth implementation.

Imported auth documents are migration input. Preserve the complete Codex-generated document and validate it before adding it to the saved account store.

Account intake must not unexpectedly replace the user's current live Codex identity.

## Save an unknown live account

If Codex is signed in to an identity GSwitch does not know, treat that state as user data.

Do not overwrite it.

Offer to save the current account before a switch may replace the live credential.

## Enable switching

OAuth and credential storage are separate concerns.

When the effective Codex credential backend cannot support deterministic file-backed switching, GSwitch must not pretend that replacing `auth.json` changes the effective account.

Any supported migration to file-backed switching is explicit. Managed or unsupported policy fails closed.

## Switch account

The visible action is **Switch**.

The durable behavior is:

1. serialize credential mutation;
2. refuse live replacement while an external Codex runtime is active;
3. preserve the newest live credential for the current saved account;
4. validate the target profile;
5. record recovery state before replacing live credentials;
6. atomically replace the live credential;
7. verify the effective target identity through Codex;
8. report success only after verification.

If the user has Codex open, ask them to quit it and retry. Do not silently kill or restart Codex.

## Remove account

Removing a saved account must not log out the active Codex identity.

The currently active saved profile is not removable until another identity is active.

## Capacity and reset state

Where Codex exposes account capacity, GSwitch may show only the state needed to make an account decision, such as short-window usage, weekly usage, and reset timing.

Do not turn capacity into an analytics product.

When reset credits are supported, show their useful expiry state and require an explicit user action before consuming one. Prefer the earliest-expiring usable credit after refreshing provider state immediately before redemption.

Do not add automatic redemption or a capacity scheduler.

## Wake

Wake is an explicit account utility action: perform the smallest suitable Codex request needed to start an account's usage window.

Wake may operate on one account or all eligible accounts.

It is not:

- traffic routing;
- account rotation;
- background keep-warm;
- scheduled automation.

A Wake operation should avoid replacing the user's live working identity when an isolated Codex profile can perform the action safely.
