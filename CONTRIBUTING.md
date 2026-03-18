# Contributing to Canal Engine

Thank you for your interest in contributing! This document covers the basics.

For detailed guides, see:
- [docs/contributing.md](docs/contributing.md) (English)
- [docs/contributing_cn.md](docs/contributing_cn.md) (中文)

## Quick Start

```bash
git clone https://github.com/Aurumbach/canal-engine.git
cd canal-engine
cargo check --all
cargo test --workspace
```

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

By contributing, you agree that your contributions will be licensed under MIT.
