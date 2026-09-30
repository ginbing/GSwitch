# Workflows

This document owns durable user-visible workflow semantics. Source, tests, and
the Codex runtime own exact command names, payload fields, and behavior at a
particular revision.

## Account intake

All intake methods produce the same local saved-account model. A new login for
the currently selected account can also apply that credential to Codex after
the browser login is saved, provided Codex is closed and the live document has
not changed. Other intake never changes the live identity.

### ChatGPT OAuth

1. Create an isolated GSwitch-owned `CODEX_HOME`.
2. Ask the official Codex App Server to start ChatGPT login.
3. Show the returned HTTPS authorization URL for copying or an explicit
   **Open browser** action. Do not open it automatically. Use Codex's local
   success page so completing login does not launch the ChatGPT desktop app.
4. Wait for the matching completion event. Once accepted, show **Finishing
   sign-in**; closing the browser or dialog does not cancel this local commit.
   Read the complete credential document into protected recovery storage before
   ending the isolated profile. Use its fresh access-token snapshot for a
   read-only metadata check without requesting a proactive refresh.
5. Match the verified identity, email when available, and saved generation to
   the intended account. Write the complete document to a new protected-vault
   generation, then atomically commit metadata pointing at it. An existing
   identity replaces its original record rather than adding a duplicate.
6. If this was the selected live account when login started and is still selected,
   Codex is closed, and the live document is unchanged, apply the saved login
   through the normal switch transaction. Otherwise show **Apply** for a saved
   current login or leave a non-current account ready for its next switch.

Cancellation before the completion event and timeout end the isolated login.
OAuth is the default login experience; GSwitch does not implement a parallel
OAuth protocol. An unreadable live Codex credential does not block a new
browser login, but prevents automatic application to that live profile.
A failed status query in the WebView does not end the isolated login. Keep the
login ID and retry the read; only a terminal status from the login operation
may be presented as completed, failed, cancelled, or timed out.
For **Sign in again** on a saved account, retain the selected account ID and
compare the newly verified user and workspace identity, plus an available
email, before replacing that account's saved credential. A different login,
removed account, or changed identity leaves the saved account and live Codex
credential unchanged. The WebView receives only a safe failure category.
If account verification or saving fails after completion, the protected login
can be retried without repeating browser authorization. A rejected identity
cannot overwrite the selected account. A failed cleanup of the isolated profile
is reported separately from the saved-login result. The App Server is stopped
before the isolated profile is removed.

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
4. read the live credential and preserve it into the matching saved profile only
   when it belongs to the same saved generation and no newer login awaits
   application;
5. validate the target snapshot: ChatGPT uses the saved access token for one
   read-only account check, routing it to the selected workspace from the
   credential or its stable identity claim; API-key identity is checked locally
   without a network request;
6. only when that ChatGPT check returns 401 or 403, check the external process
   state again, allow one [managed refresh](#managed-refresh), and repeat the
   account check once with the resulting credential;
7. persist pending-switch metadata and protected rollback auth;
8. check the external process state and live credential fingerprint again;
9. atomically replace the live credential;
10. reread `auth.json` and confirm the requested complete credential locally;
11. commit the active profile only after that verification.

A switch back to the same identity is still a transaction when the saved login
is newer than the live document. Identity equality alone cannot report success.
The write and readback must apply that saved generation. A normal Codex token
rotation may still reconcile to the saved record when it matches the expected
prior generation.

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

## Managed refresh

Switch, a manual quota refresh, and Wake share one isolated refresh for a saved
ChatGPT sign-in that ChatGPT rejected with 401 or 403 and that no running Codex
process owns. GSwitch copies the saved credential into a GSwitch-owned profile
and asks the official App Server for one token refresh.

Codex does not report that refresh's outcome in a supported form. The minimum
supported version answers from its cached account after a failed refresh, and
current versions reject the read without a typed reason. GSwitch therefore
never takes the outcome from the App Server reply. It rereads the profile's
complete credential and requires the saved identity. A rotated document is
committed immediately, or to protected recovery if that commit fails, so a
consumed refresh token never stays saved; a document for another identity is
never saved.

The caller then repeats its own provider request once with that credential. A
second 401 or 403 is the confirmed rejection, and only then is the account
marked as needing sign-in. A Codex runtime that cannot start or answer, an
unreadable profile, and every non-authentication provider failure leave the
sign-in unjudged and are reported as unavailable. A refresh that fails only
transiently at the identity provider while ChatGPT stays reachable cannot be
told apart from a rejected one and is also reported as needing sign-in; a
later successful refresh or a new sign-in clears it.

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
authentication response, a manual refresh may use the
[managed refresh](#managed-refresh) and then repeats the read-only request
once. A successful read-only result stores only the quota projection. A snapshot is fresh for five
minutes and then visibly stale. API-key accounts show quota as not applicable.
Quota is operational account state, not usage analytics.

The workspace renders its cached quota immediately and refreshes unknown or
stale ChatGPT accounts in the background. Startup and manual refreshes share one
serial request queue, and requests for the same account join the in-flight
request. Automatic refresh uses only a credential snapshot for its provider
read. It does not hold the credential-operation lock while waiting for the
provider or start a managed token refresh. A short locked commit rechecks the
saved account identity and credential generation, so a concurrent switch or
sign-in cannot make an old result authoritative. If the saved sign-in is
rejected, the card asks for a manual refresh, which may use the existing
identity-checked managed refresh path when safe. One failed account does not
stop the remaining queue. Its card keeps
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

Wake sends one short Codex model request for each selected saved ChatGPT
account, even when quota is already available or that plan has no five-hour
window. It does not redeem reset credits, route normal work, schedule future
requests, or rotate the active account. The card exposes Wake for every saved
ChatGPT account; quota display and refresh are separate actions. A running
Codex process does not make an account ineligible: a matching active identity
uses one live access-token snapshot, a different identity uses the saved
snapshot, and an unidentifiable active process uses the saved snapshot without
a managed refresh. GSwitch never writes live `auth.json`.

An authentication failure for a definitely inactive account may use one
[managed refresh](#managed-refresh), after which the Wake request is sent once
more. Only a second authentication failure reports Needs sign-in; a refresh
that could not run reports Failed. An active or uncertain account never
enters this fallback. GSwitch reads the live token once immediately before the
request. It does not retry after uncertain delivery.

The request is one direct streaming `POST /codex/responses` call through the
current ChatGPT Codex route: `gpt-6-luna`, low reasoning effort, the provider's
default service tier, a single `hi` text input, no tools, files, or project context, and
`store: false`. HTTP success alone is insufficient: the response stream must contain
nonempty assistant output and `response.completed`. A rejected, incomplete,
malformed, or interrupted stream is never labeled successful. If ChatGPT
explicitly rejects the default model, the result offers a user-initiated retry
of that account with `gpt-5.6-luna` at low effort. Other failures do not offer
that model retry. There is no generic Responses client, proxy, second App
Server, or automatic retry after a request may have reached ChatGPT. GSwitch
does not create a local Codex conversation or persist the Wake prompt or reply.
`store: false` is a request setting, not a claim that the provider retains no
operational records. A card's quota reset countdown is unrelated to whether
Wake sent a request or received a reply.

Wake all, one-account Wake, and selected-account Wake run sequentially and are
cancellable between accounts. They return one result per ChatGPT account:
Reply received, Rate limited, Needs sign-in, Sent but not confirmed, Model
unavailable, Invalid request, Service unavailable, Request rejected, Failed,
or Cancelled. Rust records each account's request state as not sent, possibly
sent, or sent. It exposes the HTTP status when one exists, while provider bodies
and credentials stay in Rust. The focused result dialog resolves each result to
the saved account's email and workspace and shows a localized status. The
heading counts replies, uncertain requests, failures, and cancellations. If the WebView temporarily cannot read
progress, it retains the operation ID and retries status reads without
restarting the request. The dialog remains open during the operation, then
closes with Done; there is no persistent result entry or history dashboard.
One account's failure does not stop the remaining queue.

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
