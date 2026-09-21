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
4. Wait for the matching completion event, read the complete resulting
   credential document, and use its fresh access-token snapshot for the
   read-only account metadata check. Do not request a proactive refresh.
5. Persist the verified profile only after its identity is known.

Cancellation and timeout end the isolated login. OAuth is the default login
experience; GSwitch does not implement a parallel OAuth protocol.

### JSON or file import

A complete Codex auth document is migration input, not an editable GSwitch
schema. Derive its stable identity locally, preserve unknown fields, and use
the existing access-token snapshot for the current ChatGPT account metadata
check. Require the returned workspace entry to match the imported identity.
Only an authentication-specific failure for an inactive ChatGPT identity may
fall back to the isolated official Codex runtime; network, timeout, 5xx, and
parse failures never trigger managed refresh. A current externally owned
identity gets one live-credential reread/retry and never enters that fallback.

Pasted JSON is transient form input and is cleared after submission. Selected
or dropped files are read by Rust; their contents are not returned to the
WebView. After successful saves, the normal background quota projection path
refreshes each imported ChatGPT account without making the batch wait for
network requests.

GSwitch can import a complete official Codex auth document and explicitly
user-selected public exports from Cockpit Tools, Sub2API, and CPA. It never
searches for, reads, or decrypts Cockpit Tools private application storage.
The native picker and drop handler accept one or more files in one bounded
operation: at most 64 files and 64 MiB in aggregate, with the existing 10 MiB
per-file limit. Rust reads and parses all readable files before starting
credential validation, deduplicates candidates across the selection and saved
profiles by `AccountIdentity`, then validates candidates sequentially. The
result reports imported accounts plus duplicate, unsupported, and failed
counts; it does not expose file paths, credential material, or provider errors.
Portable exports are converted only to the minimum complete Codex credential
shape needed for validation. Cockpit-specific private metadata such as 2FA
secrets, passwords, phone fields, notes, labels, tags, and mail settings is
dropped rather than copied into GSwitch.

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

Quota is read from ChatGPT's current read-only usage endpoint with the live
access-token snapshot when Codex is running, or the saved credential snapshot
when it is not. GSwitch sends no App Server request and writes no credential
for a successful ordinary read. It normalizes the provider's primary,
secondary, and additional buckets into five-hour, weekly, and other windows by
the durations supplied by the provider; missing or malformed values remain
unknown.

The supported minimum is Codex 0.144.5. GSwitch sends its rate-limit request
with a null parameter payload for that version and retries once with an empty
object only when a newer server explicitly rejects the parameter shape. It
never treats a cached `account/read` result as proof that a credential can reach
the provider.

If the read-only endpoint rejects an inactive saved credential with an
authentication response, GSwitch may fall back to the existing isolated App
Server refresh path, verifies the returned document still belongs to the saved
identity, and atomically stores it with the quota snapshot. A successful
read-only result stores only the quota projection. A snapshot is fresh for five
minutes and then visibly stale. API-key accounts show quota as not applicable.
Quota is operational account state, not usage analytics.

The workspace renders its cached quota immediately and refreshes unknown or
stale ChatGPT accounts in the background. A manual refresh joins that account's
existing request rather than starting another one. Adding, importing, or saving
an account follows the same refresh path. When a running Codex instance is
identified as using the account, GSwitch rereads the live file-backed
credential immediately before the request and retries once only when the same
identity has a newer credential. If the active identity cannot be safely
identified, the cached projection remains and the UI offers a retry. A 429,
transport, TLS, DNS, timeout, parse, or provider-server failure never starts a
managed refresh. Reset-credit detail failure does not erase a successful usage
result; its detailed rows remain unavailable until a later successful detail
read.

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
credit automatically. When recovery is pending, the workspace shows only a
generic recovery prompt; it never exposes the recorded account, credit ID, or
idempotency key.

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
- skips an account when an external Codex process is using that same identity,
  or when that identity cannot be checked safely;
- automatically selects only `gpt-5.6-luna` or `gpt-5.4-mini` when that
  account advertises a visible text model with the normal (`standard`) service
  tier; otherwise it asks the user to choose from eligible models;
- uses the lowest supported reasoning effort and the normal service tier;
- creates one ephemeral read-only thread with a minimal prompt, no approval,
  and no retry; the isolated profile has no user MCP servers, Skills, or
  project configuration, and the instruction asks Codex not to inspect files
  or use tools;
- confirms the result from refreshed quota when possible;
- preserves refreshed credentials only if the identity still matches.

Wake All is sequential, cancellable, and returns one result per account. A
single account failure does not corrupt or silently relabel another account.
After a turn begins, an uncertain result is reported without retrying and any
refreshed credential is persisted before the isolated profile is cleaned up.
Wake is user-triggered; there is no cron, background schedule, automatic
rotation, history dashboard, or job-management surface.

## Recovery

Interrupted switch and reset-credit actions retain durable intent so a retry can
distinguish a completed operation from one that is still pending. If a validated
credential refresh cannot be written to the main store, GSwitch retains a
protected recovery copy instead of discarding it.

A damaged GSwitch account store starts in recovery mode. Resetting it preserves
the damaged file under a recovery name and creates an empty GSwitch store; it
never deletes or rewrites the live Codex credential. The recovery workspace
does not show an empty-state onboarding flow or allow account actions: the user
must explicitly confirm the GSwitch-only reset before normal intake resumes.
