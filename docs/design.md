# Design

GSwitch should feel like a quiet, direct desktop utility. A user should
understand the current state and act within seconds.

The primary flow is:

> Open → see state → Switch, reset, or Wake → done

The interface does not need to advertise feature depth.

## Window and hierarchy

Keep one native-titlebar main window and one primary information hierarchy.
The default desktop canvas is 1100 by 700, with a 900 by 580 minimum. The
main surface is an account workspace: a quiet toolbar, a narrow safety notice
when needed, and a responsive grid of small account cards. Three cards fit on a
wide desktop, then collapse to two and one card without introducing navigation.
Focused dialogs may handle add-account, explicit confirmation, progress/results,
or recovery; they do not create a second navigation system.

The toolbar contains the current Codex account, Refresh, Wake all, Add account,
and a lightweight Settings button. It is a command bar, not a dashboard header
or a custom window chrome. Its status dot always remains visible; only a long
account label may truncate. The status is also written in text so ready, unknown,
signed-out, setup, and recovery states do not rely on color.

An account card should show only the state needed for a decision:

- label and useful identity;
- active, ready, needs-login, unsupported, or busy state;
- five-hour and weekly quota with reset timing when available;
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
as **Add account** and **Wake All** stay near the cards rather than behind a
sidebar.

Next to Saved accounts, **Select** enters a compact selection mode. Checkboxes
appear only in that mode, with Select all, Clear, Wake, and Export in one
compact action bar. It must remain a card-grid interaction rather than a
dashboard, table, bulk-management page, or second navigation system. Export
always opens a focused warning before the native save dialog because its
portable JSON contains unencrypted credentials.

Reset credits remain compact in the row. Details and the destructive
**Use reset** confirmation appear only on demand, with the earliest-expiring
eligible credit presented first.

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

- Do not optimistically display a switch or redemption as complete.
- Opening a dialog moves keyboard focus inside it. Tab stays inside, and closing
  restores focus to the launching control. A dialog cannot be dismissed while
  its non-cancellable action is in progress.
- Disable conflicting credential actions while one is in progress.
- Show progress on the affected row or in the focused operation dialog.
- Keep Wake results per account; cancellation and partial failure are explicit.
- Keep pasted credentials and API keys in transient, non-persistent inputs and
  clear them immediately after submission.
- File import reads the selected path in Rust rather than copying file contents
  into persistent WebView state.
- Export selection state and its completion count are safe presentation state;
  credential documents and the selected destination never enter React.

Errors should answer three questions: what did not happen, whether the current
Codex state is safe, and what the user can do next. Raw Rust, HTTP, OAuth,
filesystem, protocol, or token details do not belong in the primary UI.

## Language

The application ships English and Simplified Chinese in one frontend resource.
On first launch it maps a compatible system/WebView locale to one of those
languages and otherwise falls back to English. Settings offers only System /
Automatic, English, and Simplified Chinese; a manual choice applies immediately
and wins over automatic detection. The only persisted WebView preference is that
language choice. Dates, reset/expiry timing, numbers, and percentages use the
selected locale's platform formatters.

## Visual rules

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
