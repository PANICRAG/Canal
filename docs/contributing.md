# Contributing

Thank you for your interest in contributing to Canal Engine!

## Development Setup

### Prerequisites

- Rust 1.80+ (`rustup update`)
- PostgreSQL 14+
- Docker (for code execution tests)

### Clone and Build

```bash
git clone https://github.com/Aurumbach/canal-engine.git
cd canal-engine
cp .env.example .env
# Edit .env with your configuration

# Full build with all features
cargo build -p gateway-api --features full-orchestration

# Run tests
cargo test --workspace
```

### Development Workflow

```bash
# Check compilation after changes
cargo check --all

# Run clippy for linting
cargo clippy --all

# Format code
cargo fmt --all

# Run with hot reload
cargo install cargo-watch
cargo watch -x "run -p gateway-api --features full-orchestration"
```

## Code Style

- **Language**: All source code comments in English
- **Formatting**: `cargo fmt` (configured in `rustfmt.toml`)
- **Linting**: `cargo clippy` (configured in `clippy.toml`)
- **Error handling**: Use `thiserror` for library errors, `anyhow` for application errors
- **Async**: Use `tokio` runtime, `async-trait` for trait methods

## Architecture Guidelines

### Crate Boundaries

- `gateway-core` — AI logic only. No HTTP, no infrastructure.
- `gateway-api` — HTTP bindings. No business logic.
- `gateway-tools` — Execution implementations. No routing logic.
- `gateway-memory` — Storage only. No AI decisions.
- `canal-cv` — Vision only. No agent logic.
- `devtools-core` — Zero gateway-core dependencies.

### Feature Gates

All new features MUST be behind feature flags:

```toml
[features]
my-feature = []

# In code:
#[cfg(feature = "my-feature")]
mod my_feature;
```

No feature-gated code in the default compilation path.

### Trait-Based Design

Every new subsystem should define a trait:

```rust
#[async_trait]
pub trait MyService: Send + Sync {
    async fn do_something(&self, input: Input) -> Result<Output>;
}
```

This enables testing with mocks and swapping implementations.

## Pull Request Process

1. **Fork** the repository
2. **Create a branch** from `main`: `git checkout -b feat/my-feature`
3. **Make changes** following the guidelines above
4. **Test**: `cargo test --workspace`
5. **Lint**: `cargo clippy --all`
6. **Format**: `cargo fmt --all`
7. **Commit** with conventional format: `feat(scope): description`
8. **Submit PR** against `main`

### Commit Message Format

```
type(scope): description

Types: feat, fix, refactor, test, docs, chore
Scope: core, api, llm, tools, memory, cv, mcp, etc.
```

Examples:

```
feat(llm): add DeepSeek provider
fix(mcp): handle reconnection on transport failure
refactor(core): extract tool registry into separate module
test(graph): add parallel execution tests
docs: update API reference for memory endpoints
```

### PR Checklist

- [ ] `cargo check --all` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --all` has no warnings
- [ ] `cargo fmt --all` applied
- [ ] New features are behind feature flags
- [ ] New public APIs have documentation
- [ ] Breaking changes are noted in PR description

## Adding a New LLM Provider

1. Create `crates/gateway-llm/src/providers/my_provider.rs`
2. Implement `LlmProvider` trait
3. Register in provider factory
4. Add configuration in `config/llm-providers.yaml`
5. Add tests

## Adding a New Tool

1. Create tool in `crates/gateway-core/src/agent/tools/my_tool.rs`
2. Implement `AgentTool` or `DynamicTool` trait
3. Register in tool registry
4. Add tests

## Adding a New MCP Server

1. Add server definition to `config/mcp-servers.yaml`
2. Test connection and tool discovery
3. Add permission configuration if needed

## Testing

```bash
# All tests
cargo test --workspace

# Specific crate
cargo test -p gateway-core

# Specific test
cargo test -p gateway-core -- test_name

# With features
cargo test --workspace --features full-orchestration
```

## Questions?

Open an issue on GitHub for questions, bug reports, or feature requests.
