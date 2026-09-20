# Design

GSwitch should feel like a quiet, direct desktop utility. A user should
understand the current state and act within seconds.

The primary flow is:

> Open → see state → Switch, reset, or Wake → done

The interface does not need to advertise feature depth.

## Window and hierarchy

Keep one main window and one primary information hierarchy. The main surface is
a compact account list with global actions close to it. Focused dialogs may
handle add-account, explicit confirmation, progress/results, or recovery; they
do not create a second navigation system.

An account row should show only the state needed for a decision:

- label and useful identity;
- active, ready, needs-login, unsupported, or busy state;
- five-hour and weekly quota with reset timing when available;
- reset-credit count and nearest expiry when available;
- direct actions such as Switch, Wake, refresh, reset, or remove when eligible.

The active account must be obvious at a glance. Actions stay next to the account
they affect. Global actions such as **Add account** and **Wake All** stay near the
list rather than behind a sidebar.

Reset credits remain compact in the row. Details and the destructive
**Use reset** confirmation appear only on demand, with the earliest-expiring
eligible credit presented first.

## Interaction states

- Do not optimistically display a switch or redemption as complete.
- Disable conflicting credential actions while one is in progress.
- Show progress on the affected row or in the focused operation dialog.
- Keep Wake results per account; cancellation and partial failure are explicit.
- Keep pasted credentials and API keys in transient, non-persistent inputs and
  clear them immediately after submission.
- File import reads the selected path in Rust rather than copying file contents
  into persistent WebView state.

Errors should answer three questions: what did not happen, whether the current
Codex state is safe, and what the user can do next. Raw Rust, HTTP, OAuth,
filesystem, protocol, or token details do not belong in the primary UI.

## Visual rules

Use system typography, natural information density, clear light/dark behavior,
restrained status color, simple borders, and consistent spacing. Controls should
look native to a desktop utility without imitating macOS chrome on every
platform.

## Anti-overdesign

Do not add interface structure whose purpose is to make the application feel
larger:

- no sidebar for a one-screen utility;
- no multi-page dashboard without a real workflow need;
- no marketing hero inside the app;
- no heavy card grid or decorative quota charts;
- no analytics or account-history dashboard;
- no large preference center;
- no excessive glass, gradients, motion, floating layers, or custom chrome.

If See, Switch, reset, or Wake requires extra navigation or explanation, first
remove interface structure. A new page, panel, or hierarchy requires a real
workflow, not visual sophistication.
