# Design

GSwitch should feel like a quiet, direct desktop utility. A user should
understand the current state and act within seconds.

The primary flow is:

> Open → see state → Switch, reset, or Wake → done

The interface does not need to advertise feature depth.

## Window and hierarchy

Keep one native-titlebar main window and one primary information hierarchy.
The default desktop canvas is 1360 by 900, centered on cold start and kept
inside the available work area, with a 700 by 520 minimum. Refocusing does
not move a window the user has positioned. The
main surface is an account workspace: a quiet toolbar, a narrow safety notice
when needed, and a responsive grid of small account cards. Three cards fit on a
wide desktop with enough card width, then collapse to two and one card without introducing navigation.
Focused dialogs may handle add-account, explicit confirmation, progress/results,
or recovery; they do not create a second navigation system.

The toolbar contains the vertically centered GSwitch mark and name, Refresh,
Wake all, Add account, and a compact language menu. The current account is
identified on its card rather than repeated beside the brand. The language
button uses Remix Icon's `earth-line` at 18px, inheriting the current theme
color. It is a command bar, not a dashboard header or custom window chrome.
Unknown, setup, and recovery states use the existing actionable notice. With
saved accounts, a signed-out Codex uses one quiet text notice without another
button; without saved accounts, the empty state provides the next step.

An account card should show only the state needed for a decision:

- label and useful identity;
- current selection, needs-login, pending-apply, unsupported, or busy state;
- the Codex quota windows actually supplied for that plan, with reset timing when available;
- reset-credit count and nearest expiry when available;
- direct actions such as Switch, Wake, refresh, reset, or remove when eligible.

For ChatGPT accounts, the email is the primary identity and the normalized
workspace or account name is secondary context when it adds information. A
legacy label may provide that secondary context only when no workspace is
available and it differs from the email; duplicate email text is omitted.
API-key cards remain label-first. Card action labels identify the primary
identity so accounts in the same workspace remain distinguishable to screen
readers and keyboard users.

The active account must be obvious at a glance through a clear border and
status badge. Actions stay next to the account they affect. Global actions such
as **Add account** and **Wake all** stay near the cards rather than behind a
sidebar.

The account section has one heading: the saved-account count. A persistent,
bordered **Select accounts** button sits directly beside it, with a clear
keyboard focus indicator. It enters a compact selection mode. Checkboxes appear
only in that mode, with Select all, Clear, Wake, and Export in one compact
action bar. It remains a card-grid interaction. Export always opens a focused
warning before the native save dialog because its portable JSON contains
unencrypted credentials.

Reset credits remain compact in the row. **Details** opens a list like Codex's
own usage resets: an available count, then one row per credit with the
provider's title, its expiry and relative time, and its own **Use** button that
becomes **Confirm** on the first click. The user picks which credit to use;
credits are listed soonest-expiring first, and a note counts any the provider
did not list. The dialog title names the account email, adding the workspace
only when the email alone is ambiguous.

The empty state is two deliberate choices: import one or more export files the
user selects, or add an account manually. The add dialog groups the normal path
as **Import existing** (Find on this computer and Choose files) then **Add
new** (official Codex sign-in). Pasted auth JSON and API key remain an **Other
methods** disclosure. One concise privacy note says that GSwitch reads only
accounts the user chooses to import and does not change their source. The local
preview explains its allowlist before scanning and keeps already-saved
identities disabled.

A damaged GSwitch account library is not an empty state. It replaces the cards
with one recovery-only surface, disables account actions, and explains that an
explicit reset preserves GSwitch's damaged file without touching Codex.

## Interaction states

After the account grid appears, GSwitch checks the installed Codex CLI against
OpenAI's latest stable release in the background. The toolbar shows a compact
entry only when a newer version is confirmed. Its dialog states the version
once, in one status line ("up to date" or "{latest} available"), and offers an
explicit update action only when that CLI exposes the official `update`
command; a single note says the desktop app updates separately and Codex must
be quit first, and progress shows on the Update button. An unsupported CLI
points to the official install guide. A failed check remains quiet; no update
is installed without a click.

- Do not optimistically display a switch or redemption as complete.
- After a verified switch, update the active card directly.
  That state is the visible confirmation; do not insert a success banner above
  the account grid or reload the whole workspace before showing it. Other brief
  success and information messages use one compact floating notice without
  moving the grid and dismiss themselves. Errors stay until dismissed. Focused
  dialogs cover this notice; recovery and update actions use their own surfaces.
- Opening a dialog moves keyboard focus inside it. Tab stays inside, and closing
  restores focus to the launching control. A dialog cannot be dismissed while
  its non-cancellable action is in progress.
- The account and language menus are small popovers, and only one is open at a
  time. Pressing outside, Escape, moving keyboard focus away, opening another
  menu, or choosing an action closes the menu. Escape and a chosen action
  return focus to the menu button.
- Disable conflicting credential actions while one is in progress.
- Show progress on the affected row or on the focused dialog's own action
  button, not as a separate dialog header label. A dialog states each fact
  once: no heading that restates its title, and no status line that repeats
  its button. A dialog can be closed while
  unrelated work runs; only its own operation keeps it open.
- Keep Wake results per account; identify each result by email and workspace,
  localize its status, and distinguish a confirmed model reply from an
  unconfirmed request or failure. While running, show only "Waking" and the
  processed count. A confirmed reply displays one short success label; failed
  rows give one actionable reason, with "not sent" or "may have been sent"
  only when that changes how a retry should be understood. HTTP status follows
  the failed reason. The completed header summarizes actual outcomes rather
  than repeating each request's transport steps. Each selected ChatGPT account
  receives one request regardless of its displayed quota or plan window. The
  result dialog stays in front until the operation finishes or the remaining queue is
  cancelled. Done dismisses its one-time results. There is no permanent Last
  result control. Only an explicit default-model rejection offers a compact,
  account-specific manual retry with the older model in this dialog; cards do
  not carry a model selector. Quota reset times never imply Wake success.
- Account removal confirms the email and workspace, and says once that it
  removes only GSwitch's saved copy without signing out or changing the
  current Codex account, while the current identity stays protected. Operation-lock failures remain visible in
  the confirmation with a retry instruction.
- Quota refreshes run in separate lanes: automatic reads one at a time, and a
  user's Refresh or Refresh all up to three read-only reads at once, never
  waiting behind automatic reads. Refresh all shows "Refreshing n/total" on the
  toolbar and leaves the workspace usable; a successful card refresh needs no
  notice because the card itself updates. Each card shows how long ago its
  quota was read next to its refresh button. Automatic reads leave switching
  available while waiting for the provider and never refresh a saved
  credential. A failed card has one compact warning icon beside its first quota
  title, with no permanent failure sentence or stale badge. Hover, keyboard
  focus, or click opens the safe reason and last successful update; Escape,
  focus leaving, or an outside click closes it. Old percentages say "Last" and
  use muted meters without reset countdowns or replacement explanation lines.
  An unavailable result stays unknown. Stale data without a failure says
  "Quota awaiting update" in its details rather than claiming a failed refresh.
  A successful refresh clears the warning and historical labels. Actions stay
  in the account menu, not in the warning details. Batch feedback describes
  quota results only.
- A saved email can be copied from its card menu. Every saved ChatGPT account
  also keeps **Sign in again** in that menu, so recovery never depends on
  GSwitch diagnosing a failure correctly. After a confirmed authentication
  failure, the card additionally shows **Sign in required** and makes
  **Sign in again** its primary action. The focused dialog shows the saved
  email and workspace, offers Copy email, and updates the original record only
  after verifying the returning identity. A saved login for the current account
  that could not be applied shows **Apply**. The current badge identifies the
  selected local Codex account without claiming that its remote sign-in is valid.
- After Wake, sign-in, or refresh, update affected cards in place. Preserve
  order and last known quota while a new provider projection is pending; old
  results cannot replace a newer login or clear the entire account grid.
- Keep card content compact. Truncated account and workspace names expose their
  full value on hover; do not reserve empty vertical space for the old heading
  reminder. Quota reset timing shows one compact relative and absolute line,
  such as `4h 55m · 09/27 16:30` or `4小时55分 · 09/27 16:30`, without a visible
  introductory label or trailing “in/后”. Show the year only across years and
  allow wrapping at the separator on narrow cards without truncating the date.
  Stale data omits the live countdown; an elapsed
  fresh reset marker says to refresh. Hover, keyboard focus, and assistive
  text provide the full reset meaning and date.
  Reset credits use two aligned lines: count and action above, compact relative
  expiry with month, day, and time below. Show the year only when the expiry
  crosses into another year; the exact date remains available on hover, focus,
  and to assistive technology.
- Keep pasted credentials and API keys in transient, non-persistent inputs and
  clear them immediately after submission.
- File import reads the selected path in Rust rather than copying file contents
  into persistent WebView state.
- Export selection state and its completion count are safe presentation state;
  credential documents and the selected destination never enter React.

Errors should answer three questions: what did not happen, whether the current
Codex state is safe, and what the user can do next. Raw Rust, HTTP, OAuth,
filesystem, protocol, or token details do not belong in the primary UI.
When completion is unknown, say so and keep checking the existing operation;
do not call it failed or stopped solely because a status query failed. A
reset-credit confirmation names the account email, and an unencrypted export
warns about credential access before anything is written. Irreversible
actions (using a reset credit, exporting unencrypted accounts, clearing a
damaged account store) take two clicks on the same button, as Codex's own
reset button does: the first click changes it to a confirm label, a second
click within 300 ms is ignored, and pressing anywhere else disarms it. Unknown quota
shows an actionable Refresh control rather than a disabled Wake action whose
only explanation is a tooltip. Update copy names the available version and
each actual phase without promising that signing protects account data.

## Language

The application ships English and Simplified Chinese in one frontend resource.
On first launch it maps a compatible system/WebView locale to one of those
languages and otherwise falls back to English. The toolbar language menu offers
System / Automatic, English, and Simplified Chinese; a manual choice applies immediately
and wins over automatic detection. The only persisted WebView preference is that
language choice. Dates, reset/expiry timing, numbers, and percentages use the
selected locale's platform formatters.

## Visual rules

The English and Chinese README screenshots come from `demo/api.ts`, which contains only fictional
reserved-domain accounts and fixed sample quota data. Generate it with
`pwsh scripts/capture-demo.ps1` on Windows and visually inspect both images before
committing it. The demo build replaces account and updater APIs and rejects
account operations; never capture the installed app or a personal account list
for README. Keep the screenshot source and command alongside the image.

Use Segoe UI Variable or the system UI font, natural information density, clear
light/dark behavior, restrained purple action color, quiet status color, simple
borders, and an eight-pixel spacing rhythm. Surfaces use modest 10 to 15 pixel
corner radii and short response transitions. Controls should look native to a
desktop utility without imitating macOS chrome on every platform. The canonical
GSwitch mark is the compact white switching loop on a flat indigo rounded
square in `assets/gswitch-icon.svg`; it is the source for the toolbar and native
or installer icon assets.

## Anti-overdesign

Do not add interface structure whose purpose is to make the application feel
larger:

- no sidebar for a one-screen utility;
- no multi-page dashboard without a real workflow need;
- no marketing hero inside the app;
- no oversized card deck, decorative quota charts, or Cockpit-style operations
  dashboard;
- no analytics or account-history dashboard;
- no large preference center;
- no excessive glass, gradients, motion, floating layers, or custom chrome.

If See, Switch, reset, or Wake requires extra navigation or explanation, first
remove interface structure. A new page, panel, or hierarchy requires a real
workflow, not visual sophistication.
