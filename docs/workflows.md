# Workflows

This document owns durable user-visible workflow semantics. Source, tests, and
the Codex runtime own exact command names, payload fields, and behavior at a
particular revision.

## Account intake

All intake methods produce the same local saved-account model without changing
the user's live Codex identity.

### ChatGPT OAuth

1. Create an isolated GSwitch-owned `CODEX_HOME`.
2. Ask the official Codex App Server to start ChatGPT login.
3. Open or copy the returned HTTPS authorization URL.
4. Wait for the matching completion event, then confirm the account through
   Codex and read the complete resulting credential document.
5. Persist the verified profile only after its identity is known.

Cancellation and timeout end the isolated login. OAuth is the default login
experience; GSwitch does not implement a parallel OAuth protocol.

### JSON or file import

A complete Codex auth document is migration input, not an editable GSwitch
schema. Preserve unknown fields, validate it through an isolated official Codex
runtime, and require the validated identity to match the imported document.

Pasted JSON is transient form input and is cleared after submission. A selected
or dropped file is read by Rust; its contents are not returned to the WebView.

### API key

Use the official Codex login surface in an isolated profile. The key field is
transient and cleared after submission. API-key accounts are stored as
unverified for subscription behavior: GSwitch must not make a hidden billable
request merely to validate them, and subscription quota, reset credits, and
Wake do not apply.

## First launch and live identity

GSwitch inspects the effective Codex credential-store policy and current live
identity. If the current identity is unknown to GSwitch, it is user data: show
it as an unsaved current account and offer to save it before any replacement.

Switching requires an effective file-backed credential store. Any supported
change to that policy is explicit, preserves other Codex configuration, checks
the effective configuration through Codex, and refuses managed, keyring-only,
ephemeral, or ambiguous states it cannot migrate safely.

## Switch account

The visible interaction is one **Switch** action. Rust owns the full transaction:

1. acquire the single credential-operation lock;
2. reject a pending recovery or active/uninspectable external Codex runtime;
3. confirm the effective file-backed store;
4. read the live credential and preserve it into the matching saved profile;
5. validate and refresh the target in an isolated Codex profile;
6. persist pending-switch recovery state;
7. check the external process state and live credential fingerprint again;
8. atomically replace the live credential;
9. ask Codex to confirm the effective target identity;
10. commit the active profile only after verification.

If verification fails, restore the previous credential only when the live file
still matches what GSwitch wrote. An external change makes the result ambiguous,
so recovery fails closed instead of overwriting it.

The UI never marks a target active optimistically. If Codex is running, ask the
user to quit it and retry; v1 does not kill or restart Codex.

Removing a saved profile never logs out the live identity. The active saved
profile cannot be removed until another identity is active.

## Quota

Quota is read through the official Codex App Server in an isolated account
profile. GSwitch normalizes the Codex bucket into five-hour and weekly windows
by the durations supplied by the provider, while retaining other buckets as
other. Missing or malformed values remain unknown.

The supported minimum is Codex 0.144.5. GSwitch sends its rate-limit request
with a null parameter payload for that version and retries once with an empty
object only when a newer server explicitly rejects the parameter shape. It
never treats a cached `account/read` result as proof that a credential can reach
the provider.

An isolated read can rotate credentials. GSwitch verifies the returned document
still belongs to the saved identity, then atomically stores it with the quota
snapshot; a failed store write retains protected recovery data rather than
discarding the refreshed credential. A snapshot is fresh for five minutes and
then visibly stale. API-key accounts show quota as not applicable. Quota is
operational account state, not usage analytics.

## Reset credits

The provider's available count is authoritative. Individual credit details stay
in Rust-owned storage; the frontend receives only the useful summary needed for
display and action eligibility.

**Use reset** is always explicit. Immediately before redemption GSwitch:

1. refreshes the provider's reset-credit state;
2. keeps only available and unexpired credits;
3. chooses the eligible credit with the earliest expiry, placing credits without
   an expiry after dated credits;
4. persists the selected credit and a unique idempotency key;
5. consumes that exact credit through Codex;
6. clears the pending record only after an authoritative outcome;
7. refreshes quota and reset-credit state.

If detailed credits are unavailable, GSwitch may show the count but cannot
redeem safely. A confirmed redemption remains confirmed if the post-action
refresh fails; the UI reports the refresh warning separately. An interrupted
request reuses the durable idempotency key during recovery.

`reset` and `alreadyRedeemed` are confirmed outcomes. `nothingToReset` and
`noCredit` explicitly mean no credit was consumed. Recovery is a user-directed
retry of the exact recorded credit and key; it never selects a replacement
credit automatically.

There is no automatic redemption, expiry watcher, or scheduler.

## Wake

Wake deliberately starts an eligible account's five-hour window with one small
Codex request. It is not a health check, router, load balancer, account rotation,
or keep-warm service.

Each Wake runs with the saved account in an isolated `CODEX_HOME` and an empty
workspace. It does not replace the live credential or load the user's project,
MCP servers, Skills, or normal Codex settings. The operation:

- refreshes quota first and skips a window already active;
- refuses to spend reset credits or Reserve when ordinary quota is exhausted;
- selects only an approved low-cost text model advertised for that account;
- uses the lowest supported reasoning effort and standard service tier when
  available;
- creates one ephemeral read-only thread with a minimal prompt, no tool use,
  no approval, and no retry;
- confirms the result from refreshed quota when possible;
- preserves refreshed credentials only if the identity still matches.

Wake All is sequential, cancellable, and returns one result per account. A
single account failure does not corrupt or silently relabel another account.
Wake is user-triggered; there is no cron, background schedule, automatic
rotation, history dashboard, or job-management surface.

## Recovery

Interrupted switch and reset-credit actions retain durable intent so a retry can
distinguish a completed operation from one that is still pending. If a validated
credential refresh cannot be written to the main store, GSwitch retains a
protected recovery copy instead of discarding it.

A damaged GSwitch account store starts in recovery mode. Resetting it preserves
the damaged file under a recovery name and creates an empty GSwitch store; it
never deletes or rewrites the live Codex credential.
