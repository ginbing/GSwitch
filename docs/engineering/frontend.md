# Frontend

GSwitch should feel like a compact desktop utility rather than an operations dashboard.

## Main view

Keep one primary information hierarchy.

An account row should make the user's decision obvious with the minimum useful state:

- identity or label;
- active state;
- useful quota/reset state when available;
- reset-credit count / nearest expiry when available;
- direct account actions.

Global actions such as Add account or Wake All belong near the account list rather than behind a navigation system.

## Interaction

- Active identity must be obvious at a glance.
- Do not optimistically display a switch as complete.
- Show progress and errors next to the action they affect.
- Technical Rust, OAuth, HTTP, filesystem, or token details do not belong in the primary UI.
- Errors should explain what did not happen, whether the current account is safe, and the next user action.

## Anti-overdesign

Avoid interface structure whose only purpose is to make the application feel larger:

- no sidebar for a one-screen utility;
- no multi-page dashboard without a real navigation need;
- no decorative analytics charts for quota state;
- no heavy card-grid hierarchy;
- no marketing hero inside the desktop app;
- no large preference center;
- no excessive glass, gradients, motion, or custom chrome.

Use system typography, natural information density, clear light/dark behavior, restrained state color, and simple spacing.

A new page, panel, or navigation layer should correspond to a real user workflow, not visual sophistication.
