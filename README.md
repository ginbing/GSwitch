# GSwitch

GSwitch is a compact, local-first desktop utility for people who personally use
multiple Codex accounts. Its core job is to make account state understandable,
account switching safe, and a small number of capacity actions explicit.

> **GSwitch is a Codex switcher that stays a switcher.**

It is not a proxy, request router, account pool, or AI infrastructure platform.
The durable product boundary is in [PRODUCT.md](./PRODUCT.md).

## Documentation

Long-lived product and engineering facts live in this repository, not in
planning Issues. Start with [docs/README.md](./docs/README.md), which routes each
kind of change to its single documentation owner.

GitHub Issues are for current bugs, features, and bounded tasks. Exact behavior
of a particular build is established by its source, configuration, tests, and
the Codex runtime it uses.

## Development

GSwitch uses Tauri 2 and Rust for the trusted desktop boundary, with React,
TypeScript, Vite, and Tailwind CSS for the WebView UI.

```bash
pnpm install --frozen-lockfile
pnpm test
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
pnpm tauri build --no-bundle
```

See [docs/testing.md](./docs/testing.md) for what these checks do and do not
prove, and [docs/release.md](./docs/release.md) before producing a public build.
