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
3. Show the returned HTTPS authorization URL for copying or an explicit
   **Open browser** action. Do not open it automatically. Use Codex's local
   success page so completing login does not launch the ChatGPT desktop app.
4. Wait for the matching completion event, read the complete resulting
   credential document, and use its fresh access-token snapshot for the
   read-only account metadata check. Do not request a proactive refresh.
5. Write the verified complete document to a new protected-vault generation,
   then atomically commit metadata pointing at it only after its identity is
   known.

Cancellation and timeout end the isolated login. OAuth is the default login
experience; GSwitch does not implement a parallel OAuth protocol.
For **Sign in again** on a saved account, retain the selected account ID and
compare the newly verified user and workspace identity, plus an available
email, before replacing that account's saved credential. A different login,
removed account, or changed identity leaves the saved account and live Codex
credential unchanged. The WebView receives only a safe failure category.
The App Server is stopped before the isolated profile is removed, including
after cancellation or validation failure; cleanup failure is reported.

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
user-selected public exports from Cockpit Tools, Sub2API, and CPA. It does not
inspect another application's account storage automatically. The Add Account
dialog prioritizes **Import existing** with one-shot **Find on this computer**
and **Choose files** actions, then **Add new** with official Codex sign-in;
paste JSON and API-key intake remain under Other methods. After the user starts
Find on this computer, Rust reads only the documented Official Codex profile and
Cockpit production/legacy roots (`codex_accounts.json`, direct detail files,
and the existing secure-storage key). A user-selected alternate folder is
bounded to the same allowlist. The preview contains only email, workspace or
account name, local plan, source, and New/Already/Unsupported state. Already
saved identities are disabled and never replaced.

The local migration decoder accepts only the current Cockpit version-1
`codex`/`AES-256-GCM` envelope with its existing 32-byte key and 12-byte nonce,
or an unencrypted known Codex record. It never creates, rotates, repairs, or
rewrites Cockpit files. Confirming a preview rereads the selected records and
rederives identity; a changed identity makes the preview stale. Supported
records are reduced to the minimum complete Codex credential shape and then
sent through the normal snapshot-first intake path. There is no startup scan,
watcher, scheduler, plugin source, or generic search surface.

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

### Portable GSwitch export

Selection mode is the normal way to act on several saved accounts. The
WebView holds only selected account IDs; Rust verifies those IDs under the
operation lock, reads the matching complete snapshots from the protected vault,
opens the native save dialog, and writes one explicitly user-authorized
portable document. It returns only an aggregate count or cancellation state.

The version-1 format is a single JSON object:

```json
{"format":"gswitch-accounts","version":1,"accounts":[...]}
```

Each entry contains the complete credential document and optional label or
workspace display metadata. It excludes account IDs, account kind, email, plan,
identity, quota/cache data, reset-credit state, Wake state, recovery records,
settings, paths, updater data, and source metadata. Before the native save
dialog, the UI makes clear that the file is unencrypted. On Unix the write uses
private file permissions; users still choose a private location and remain
responsible for deleting the export when finished.

The existing bounded Rust batch parser recognizes this format by its explicit
`format` field, accepts only version 1, and fails closed for other versions. It
then uses the normal identity deduplication and validation path. Filename or
extension never selects a parser.

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

**Add current account** reads a file-backed ChatGPT credential and derives its
stable user/workspace identity locally. A read-only account check uses that
access-token snapshot; the returned workspace must match. Before saving the
complete live document and non-secret account metadata, GSwitch rereads the
live credential. If Codex has written a newer document for the same identity,
it retries the read-only check once with that snapshot; an identity change or
another update aborts. It never requests a managed refresh, even when the
read-only check returns an authentication error. Copying the live document to
an isolated profile would not make such a refresh safe: it could rotate the
provider's refresh-token chain while Codex still owns the live credential.
This read/save action remains available while Codex is running and never writes
live `auth.json`. API-key current-account intake keeps its existing isolated
non-refreshing account read.

Switching requires an effective file-backed credential store. Any supported
change to that policy is explicit, preserves other Codex configuration, checks
the effective configuration through Codex, and refuses managed, keyring-only,
ephemeral, or ambiguous states it cannot migrate safely.

## Switch account

The visible interaction is one **Switch** action. Rust owns the full transaction:

1. acquire the single credential-operation lock;
2. reject a pending recovery, then reject an active or uninspectable external
   Codex runtime before any provider request;
3. confirm the effective file-backed store through the supported Codex
   configuration surface, then stop that helper process;
4. read the live credential and preserve it into the matching saved profile;
5. validate the target snapshot: ChatGPT uses the saved access token for one
   read-only account check, routing it to the selected workspace from the
   credential or its stable identity claim; API-key identity is checked locally
   without a network request;
6. only when that ChatGPT check returns 401 or 403, check the external process
   state again and allow one isolated managed refresh; identity-check the
   refreshed complete document before saving it;
7. persist pending-switch metadata and protected rollback auth;
8. check the external process state and live credential fingerprint again;
9. atomically replace the live credential;
10. reread `auth.json` and confirm the requested identity locally;
11. commit the active profile only after that verification.

A successful ChatGPT snapshot check updates only normalized non-secret account
metadata. It does not start an isolated App Server, refresh the token, or rewrite
the saved credential. Rate limits, transport and TLS failures, timeouts, 5xx
responses, and malformed responses fail the switch without entering managed
refresh. The write-time verification never starts a second App Server.

If verification fails, restore the previous credential only when the live file
still matches what GSwitch wrote. An external change makes the result ambiguous,
so recovery fails closed instead of overwriting it.

Switch failures cross the Tauri boundary only as a sanitized code. Target
provider unavailability, workspace mismatch, local verification failure, and
post-write verification with a successful rollback are distinguished from
sign-in, open-Codex, credential-change, file-store, and recovery failures. The
frontend combines the code with already-present account display data; provider
errors, credential contents, and filesystem paths remain in Rust.

The UI never marks a target active optimistically. If Codex is running, ask the
user to quit it and retry; v1 does not kill or restart Codex.

Removing a saved profile is serialized by GSwitch's single-operation lock and
is refused while switch recovery is pending. GSwitch protects the live identity:
it cannot remove a profile that matches the current file-backed Codex credential
or the account marked active in its store. Removing another saved profile only
updates GSwitch's account library; it never edits live `auth.json` and is allowed
while Codex runs. If another GSwitch operation owns the lock, the confirmation
stays open and asks the user to retry after that operation finishes.

## Quota

Quota is read from ChatGPT's current read-only usage endpoint with the live
access-token snapshot when Codex is running, or the saved credential snapshot
when it is not. GSwitch sends no App Server request and writes no credential
for a successful ordinary read. It normalizes the provider's primary,
secondary, and additional buckets into five-hour, weekly, and other windows by
the durations supplied by the provider; missing or malformed values remain
unknown. The card renders whichever Codex windows the provider actually supplies,
including a five-week Free-plan window, with its reset time. It does not
invent a five-hour or weekly window for a plan that lacks one.

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
stale ChatGPT accounts in the background. Startup and manual refreshes share one
serial request queue, and requests for the same account join the in-flight
request. One failed account does not stop the remaining queue. Its card keeps
the last result marked stale, or shows quota as unavailable when no snapshot
exists. A compact warning on that card explains the safe failure category and
last successful update; old percentages are explicitly labelled as the last
result. The batch notice reports only failed quota refreshes. Adding, importing,
or saving an account follows the same refresh path. When a running Codex instance is
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
4. persists a non-secret pending reference plus the selected credit and unique
   idempotency key in protected storage;
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

Wake reads quota first through GSwitch's narrow ChatGPT backend client. It
reports an already-active five-hour window or unavailable ordinary capacity
without sending a request, and it never uses Reserve or reset credits. A running
Codex process never makes an account ineligible: a matching active identity uses
one live access-token snapshot, a different identity uses the saved snapshot,
and an unidentifiable active process uses the saved snapshot without a managed
refresh. GSwitch never writes live `auth.json`.

An authentication failure for a definitely inactive account may use one
isolated official Codex App Server refresh. That profile is identity-checked
before its refreshed credential is stored. An active or uncertain account never
enters this fallback. If a matching active token changes before Wake sends, it
rereads the live token once and repeats only the quota preflight.

The request is one direct `POST /codex/responses` call through the current
ChatGPT Codex route: `gpt-5.6-luna`, standard tier, no reasoning, a single
`OK` text input, no tools, no files or project context, and no stored response.
There is no generic Responses client, proxy, second App Server, or retry after
the request may have reached ChatGPT. Wake rereads quota afterward only to
confirm the new window; an unavailable confirmation is reported as sent but not
confirmed.

Wake All and selected-account Wake are sequential, cancellable, and return one
result per eligible ChatGPT account:
Started, Already active, No ordinary capacity, Needs sign-in, Sent not
confirmed, Failed, or Cancelled. The result view resolves each result to the
saved account's email and workspace and shows a localized outcome instead of
provider error text. A running operation can continue in the background and be
reopened from the toolbar; completed results stay available there until dismissed. A single account failure does not corrupt or
silently relabel another account. Wake is user-triggered; there is no cron,
background schedule, automatic rotation, history dashboard, or job-management
surface.

When the freshly read quota specifically shows a zero five-hour or weekly
balance in an active window, name that window in the result. Otherwise say only
that Wake has no available quota; do not infer which limit was exhausted.

## Recovery

Interrupted switch and reset-credit actions retain non-secret durable intent
plus protected recovery material so a retry can distinguish a completed
operation from one that is still pending. If a validated credential refresh
cannot be committed to metadata, GSwitch retains a copy only after the
encrypted-vault write and readback succeed. If protected recovery also fails,
the isolated profile is removed instead of being retained as plaintext. Legacy
version-3 and unversioned inline account stores migrate each complete document
into the vault and verify stable identity before atomically replacing active
metadata; interruption leaves the old source recoverable.

Pending-credential recovery is an explicit Rust command, not automatic startup
replay. It rederives identity, checks the saved generation before replacing an
existing credential, and refuses a stale or ambiguous entry. For an account
not yet saved, it creates one local recovered profile without a provider
request. A successful metadata commit precedes removal of the encrypted queue
entry; the command returns only the number recovered. The current WebView does
not yet offer a control for this command.

A damaged GSwitch account store starts in recovery mode. Resetting it preserves
the damaged file under a recovery name and creates an empty GSwitch store; it
never deletes or rewrites the live Codex credential. The recovery workspace
does not show an empty-state onboarding flow or allow account actions: the user
must explicitly confirm the GSwitch-only reset before normal intake resumes.
