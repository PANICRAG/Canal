# Contributing to Canal Engine

Thank you for your interest in contributing! This document covers the basics.

For detailed guides, see:
- [docs/contributing.md](docs/contributing.md) (English)
- [docs/contributing_cn.md](docs/contributing_cn.md) (中文)

## Quick Start

```bash
git clone https://github.com/PANICRAG/Canal.git
cd Canal

# Enable pre-push hooks (runs fmt, clippy, test, typos before each push)
git config core.hooksPath .githooks

# Verify everything compiles
cargo check --all
cargo test --workspace
```

## Pre-push Hooks

This project uses git hooks to catch issues **before** they reach CI. After cloning, run:

```bash
git config core.hooksPath .githooks
```

The pre-push hook runs:
1. `cargo fmt --check` — formatting
2. `cargo clippy` — linting
3. `cargo test --workspace` — all tests
4. `typos` — spell checking (optional, install with `cargo install typos-cli`)

If any check fails, the push is blocked. Fix the issue and try again.

## Pull Request Process

1. Fork and create a branch: `git checkout -b feat/my-feature`
2. Make changes, run `cargo fmt --all && cargo clippy --all`
3. Run tests: `cargo test --workspace`
4. Commit: `type(scope): description` (e.g., `feat(llm): add DeepSeek provider`)
5. Open PR against `main`

## Code Style

- Rust formatting: `cargo fmt` (see `rustfmt.toml`)
- Linting: `cargo clippy` (see `clippy.toml`)
- Source code comments in English
- New features behind feature flags

## License

By contributing, you agree that your contributions will be licensed under Apache 2.0.
