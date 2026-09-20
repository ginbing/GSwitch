# Testing

Tests prove specific behavior. They do not replace product or architecture ownership.

## Standard validation

Frontend:

```bash
pnpm test
pnpm build
```

Rust:

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

Desktop build:

```bash
pnpm tauri build --no-bundle
```

GitHub Actions runs frontend validation plus Rust/Tauri checks on macOS and Windows.

## Credential-state changes

Changes to intake, storage, identity, process detection, switching, recovery, quota mutation, reset-credit redemption, or Wake require focused regression coverage for the affected safety boundary.

Important scenarios include:

- complete credential-document preservation;
- stale/rotated credential reconciliation;
- overlapping-operation rejection;
- unreadable or malformed state failing closed;
- atomic-write safety;
- external Codex runtime blocking live mutation;
- target identity verification before switch success;
- unknown live identity preservation;
- recovery after an interrupted switch;
- provider-state refresh before destructive capacity actions.

## Proof rule

Run the smallest focused tests while editing, then the repository CI path before merge.

Do not claim a platform build or behavior was validated unless the corresponding check actually ran.
