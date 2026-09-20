# GSwitch

GSwitch is a focused local desktop utility for people who use multiple Codex accounts.

> **GSwitch is a Codex switcher that stays a switcher.**

It exists to make local account state understandable and account changes reliable without becoming a proxy, router, account pool, or AI infrastructure platform.

## Documentation

Durable product and engineering knowledge lives in committed repository documentation, not in long-lived planning Issues.

Start with [`docs/README.md`](./docs/README.md).

## Development

The application uses Tauri 2, Rust, React, TypeScript, Vite, and Tailwind CSS.

Common validation:

```bash
pnpm install --frozen-lockfile
pnpm test
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
pnpm tauri build --no-bundle
```

The executable repository state is authoritative for what a particular build currently supports.
