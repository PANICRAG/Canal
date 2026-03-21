# Getting Started

This guide walks you through setting up Canal Engine from scratch.

## Prerequisites

| Requirement | Version | Required |
|-------------|---------|----------|
| Rust | 1.80+ | Yes |
| PostgreSQL | 14+ | Yes |
| Docker | 20+ | No (for sandboxed code execution) |
| Qdrant | 1.7+ | No (for semantic memory) |
| Redis | 7+ | No (for caching) |

## Step 1: Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update
```

Verify:

```bash
rustc --version  # Should be 1.80+
cargo --version
```

## Step 2: Clone and Configure

```bash
git clone https://github.com/Aurumbach/canal-engine.git
cd canal-engine
```

Create your environment file:

```bash
cp .env.example .env
```

Edit `.env` with your configuration. At minimum, you need:

```bash
# Required: at least one LLM provider
ANTHROPIC_API_KEY=sk-ant-your-key
# or
OPENAI_API_KEY=sk-your-key
# or
QWEN_API_KEY=your-qwen-key

# Required: PostgreSQL database
DATABASE_URL=postgresql://user:password@localhost:5432/canal

# Required: JWT authentication
JWT_SECRET=your-256-bit-secret  # Generate with: openssl rand -hex 32
API_KEY_SALT=your-random-salt
```

## Step 3: Database Setup

Create the database and run migrations:

```bash
# Create the database
createdb canal

# Migrations run automatically on first start
# Or run manually with sqlx-cli:
cargo install sqlx-cli
sqlx migrate run
```

## Step 4: Build

### Minimal Build

For basic LLM routing, chat, MCP, and tool execution:

```bash
cargo build --release -p gateway-api
```

### Full Build

For all features including graph execution, multi-agent collaboration, learning, and observability:

```bash
cargo build --release -p gateway-api --features full-orchestration
```

### Feature Combinations

```bash
# Just graph execution + collaboration
cargo build --release -p gateway-api --features orchestration

# Graph + caching + learning
cargo build --release -p gateway-api --features "graph,cache,learning"

# With observability
cargo build --release -p gateway-api --features "orchestration,devtools"
```

## Step 5: Run

```bash
cargo run --release -p gateway-api --features full-orchestration
```

You should see:

```
INFO  gateway_api > Canal Engine starting on 0.0.0.0:4000
INFO  gateway_api > LLM providers loaded: anthropic, qwen
INFO  gateway_api > MCP servers connected: 3
INFO  gateway_api > Ready to serve requests
```

## Step 6: Verify

### Health Check

```bash
curl http://localhost:4000/api/health
```

Response:

```json
{
  "status": "healthy",
  "version": "0.1.0"
}
```

### Chat Request

```bash
curl -X POST http://localhost:4000/api/chat \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -d '{
    "message": "Hello! What can you do?",
    "stream": false
  }'
```

### Streaming Chat

```bash
curl -N -X POST http://localhost:4000/api/chat/stream \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -d '{
    "message": "Write a Python function to sort a list",
    "stream": true
  }'
```

### List Available Tools

```bash
curl http://localhost:4000/api/tools \
  -H "Authorization: Bearer YOUR_JWT_TOKEN"
```

### Execute Code

```bash
curl -X POST http://localhost:4000/api/code/execute \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -d '{
    "language": "python",
    "code": "print(sum(range(100)))"
  }'
```

## Optional: Docker for Code Execution

Install Docker for sandboxed code execution:

```bash
# macOS
brew install --cask docker

# Linux
curl -fsSL https://get.docker.com | sh
```

Canal Engine will automatically use Docker for Python and Bash execution when available.

## Optional: Qdrant for Semantic Memory

For vector-based semantic caching and memory:

```bash
# Run Qdrant locally
docker run -p 6333:6333 qdrant/qdrant
```

Add to `.env`:

```bash
QDRANT_URL=http://localhost:6333
```

## Optional: Redis for Caching

```bash
docker run -p 6379:6379 redis
```

Add to `.env`:

```bash
REDIS_URL=redis://localhost:6379
```

## Development Mode

For development with hot reload:

```bash
# Install cargo-watch
cargo install cargo-watch

# Run with auto-reload
cargo watch -x "run -p gateway-api --features full-orchestration"
```

Enable the debug dashboard:

```bash
# In .env
DEV_MODE=true
```

This enables `/api/debug/*` endpoints for inspecting graph execution, cache stats, and more.

## Troubleshooting

### Port already in use

```bash
lsof -i :4000
kill -9 <PID>
```

### Compilation errors after feature flag changes

```bash
cargo clean -p gateway-api -p gateway-core
cargo build -p gateway-api --features full-orchestration
```

### Database connection issues

Check your `DATABASE_URL` format:

```
postgresql://user:password@host:5432/database_name
```

### LLM provider errors

Verify your API keys:

```bash
# Test Anthropic
curl https://api.anthropic.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{"model":"claude-sonnet-4-6","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}'
```

## Next Steps

- [Architecture](architecture.md) — Understand how Canal Engine is designed
- [Configuration](configuration.md) — Full configuration reference
- [API Reference](api-reference.md) — Complete endpoint documentation
- [LLM Providers](llm-providers.md) — Provider setup and routing strategies
- [MCP Integration](mcp-integration.md) — Connect MCP servers
