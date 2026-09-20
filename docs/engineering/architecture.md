# Architecture

## Overview

GSwitch is a Tauri 2 desktop application.

- Rust owns credentials, Codex runtime interaction, filesystem mutation, process checks, persistence, and safety-critical account transitions.
- React + TypeScript owns presentation and direct user interaction.
- Vite builds the frontend.
- Tailwind CSS provides styling.
- The frontend calls explicit Tauri commands. There is no local HTTP API or listening port.

This architecture is intentionally smaller than a general account-management platform.

## Rust ownership

Current backend modules are organized by concrete responsibility:

- `accounts.rs`: saved-account state, operation serialization, account-store coordination, and recovery persistence
- `app_server.rs`: Codex App Server process and request/response boundary
- `codex.rs`: `CODEX_HOME`, Codex config, and file-backed auth-document access
- `commands.rs`: thin Tauri IPC adapters
- `identity.rs`: non-secret account identity derivation and credential classification
- `intake.rs`: OAuth, auth-document import, and API-key intake
- `runtime.rs`: external Codex process detection
- `storage.rs`: versioned local store and atomic/private writes
- `switching.rs`: live-account state, file-store enablement, switch transaction, removal, and recovery
- `types.rs`: serialized view models and backend state types
- `lib.rs`: Tauri setup, managed state, and command registration

Do not create service/repository/domain/provider layers, dependency injection, a database, daemon, or local API server for hypothetical growth.

## Runtime model

Codex owns the effective runtime identity.

GSwitch owns saved local profiles and its own recovery metadata.

A saved credential document is treated as an opaque Codex-owned document. GSwitch derives only the minimum non-secret identity needed to match profiles and otherwise preserves the complete document.

The frontend receives sanitized account views, never stored credential documents.

## Codex App Server

Where an official Codex App Server operation exists, prefer it over reimplementing Codex protocol behavior.

Temporary isolated Codex profiles are appropriate when an operation must validate or refresh a saved account without replacing the user's live Codex identity.

## Mutation boundary

Credential-changing operations are serialized in Rust.

A switch is a transaction, not a file copy:

1. prove the live runtime is safe to mutate;
2. preserve the latest live state;
3. validate the target;
4. persist recovery intent;
5. atomically replace the live credential;
6. verify the effective identity;
7. commit the active-account state only after verification.

See `OWNERSHIP.md` and `engineering/security.md` for authority and failure rules.

## Dependencies

New dependencies require a current product or safety need.

Do not add infrastructure because future work might use it.
