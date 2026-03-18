<p align="center">
  <h1 align="center">Canal Engine</h1>
  <p align="center">
    Open-source AI orchestration engine built in Rust.<br>
    LLM routing · Agent loops · MCP · Multi-agent collaboration · Computer vision.
  </p>
</p>

<p align="center">
  <a href="README_CN.md">中文</a> ·
  <a href="docs/getting-started.md">Getting Started</a> ·
  <a href="docs/architecture.md">Architecture</a> ·
  <a href="docs/api-reference.md">API Reference</a> ·
  <a href="docs/configuration.md">Configuration</a> ·
  <a href="docs/contributing.md">Contributing</a>
</p>

<p align="center">
  <img alt="License" src="https://img.shields.io/badge/license-MIT-blue.svg">
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.80%2B-orange.svg">
  <img alt="LOC" src="https://img.shields.io/badge/lines%20of%20code-160K%2B-brightgreen.svg">
</p>

---

## Why Canal Engine?

Most AI frameworks give you either a thin wrapper around one LLM, or a heavyweight platform that bundles everything from auth to billing. Canal Engine sits in between — it's the **AI capability layer** you embed into your own stack.

- **Multi-provider by default** — Route between Anthropic, OpenAI, Google, Qwen, OpenRouter, and Ollama with cascade fallback, A/B testing, or cost-optimized strategies. No vendor lock-in.
- **Real agent execution** — Not just "call LLM in a loop". Canal's agent has intent recognition, task planning, tool permissions, session checkpoints, and streaming — 72K lines of production logic.
- **MCP-native** — Full Model Context Protocol client and server. Connect any MCP tool (filesystem, browser, video editing) and expose your own tools via MCP.
- **Multi-agent orchestration** — Swarm (parallel handoff), Expert (hierarchical dispatch), and graph-based execution inspired by LangGraph. Not stubs — 8.7K lines of real collaboration logic.
- **Rust performance** — Async-first on Tokio. Sub-100ms per-node overhead. Cold start under 3 seconds.

### Who is this for?

- **AI platform builders** — Use Canal as the orchestration backend for your AI product. Bring your own auth, billing, and frontend.
- **Agent developers** — Build complex multi-step agents with tool calling, code execution, and multi-agent collaboration.
- **LLM infrastructure teams** — Route across providers, track costs, enforce budgets, monitor with Langfuse-style traces.

---

## Quick Start

### As a Rust library

```bash
git clone https://github.com/Aurumbach/canal-engine.git
cd canal-engine

# Build the core engine
cargo build -p gateway-core

# Build with all features
cargo build -p gateway-core --features "graph,collaboration,cache,learning,jobs"
```

### Use in your project

Add to your `Cargo.toml`:

```toml
[dependencies]
gateway-core = { path = "path/to/canal-engine/crates/gateway-core", features = ["graph", "collaboration"] }
gateway-llm = { path = "path/to/canal-engine/crates/gateway-llm" }
gateway-tools = { path = "path/to/canal-engine/crates/gateway-tools" }
```

```rust
use gateway_llm::router::LlmRouter;
use gateway_llm::types::{ChatRequest, Message};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create an LLM router with your provider config
    let router = LlmRouter::from_config(&config).await?;

    // Route a chat request — Canal picks the best provider
    let request = ChatRequest {
        messages: vec![Message::user("Explain quantum computing in one paragraph")],
        ..Default::default()
    };
    let response = router.chat(request).await?;
    println!("{}", response.content);
    Ok(())
}
```

See [Getting Started](docs/getting-started.md) for full setup including database, vector DB, and Docker.

---

## Core Features

### LLM Routing

Route requests across 6 providers with 7 strategies:

| Provider | Models | Capabilities |
|----------|--------|-------------|
| **Anthropic** | Claude 4.x (Opus, Sonnet, Haiku) | Streaming, vision, tools, extended thinking |
| **OpenAI** | GPT-4o, GPT-4, GPT-3.5 | Streaming, vision, function calling |
| **Google** | Gemini 3 Pro / Flash | Streaming, multimodal |
| **Qwen** | Qwen 3 Max, QwQ, Qwen VL | Streaming, vision, reasoning, tools |
| **OpenRouter** | UI-TARS + 100 others | Any model via OpenRouter |
| **Ollama** | Any local model | Local inference, no API key |

**Routing strategies:**

| Strategy | How it works |
|----------|-------------|
| Primary fallback | Try provider A, fall back to B on failure |
| Cascade | Start cheap (Qwen Turbo), escalate if quality is low |
| A/B test | Split traffic 70/30 between providers |
| Task-type rules | Code tasks → Anthropic, translation → Qwen |
| Multimodal | Text → Qwen, images → Claude, mixed → Claude |
| AI auto-select | A lightweight model picks the best model for each request |
| Round-robin | Distribute evenly across providers |

**Built-in cost control:** Daily budget ($100 default), per-model tracking, 80% alert threshold, circuit breaker for unhealthy providers.

See [LLM Providers](docs/llm-providers.md) for configuration.

---

### Agent Execution

The agent loop supports autonomous tool use with human-in-the-loop permissions:

```
User request
  → Intent recognition (keyword heuristics + LLM classification)
  → Task planning (single-step or multi-step decomposition)
  → Tool selection + permission check
  → Execution (MCP tools, code, filesystem, browser, git)
  → Result observation
  → Continue or complete
  → Session checkpoint
```

**Built-in tools:**

| Category | Tools |
|----------|-------|
| File ops | `Read`, `Write`, `Edit`, `Glob`, `Grep` |
| Shell | `Bash` (background shell support) |
| Code | `CodeAct` engine (stateful multi-step sessions) |
| Browser | `Navigate`, `Click`, `Type`, `Screenshot`, `Snapshot` |
| Vision | `TakeScreenshot`, `FindElement`, `OcrText`, `MouseClick` |
| Git | `Clone`, `Status`, `Diff`, `Commit`, `Push`, `Pull`, `Branch` |
| Search | `WebSearch`, `Research` |

**Permission modes:** `AllowAll` (no confirmation), `RequireConfirmation` (dangerous tools need approval), `Restricted` (whitelist only).

See [Agent Tools](docs/agent-tools.md) for the full tool reference.

---

### Multi-Agent Collaboration

Three orchestration modes, all fully implemented (8.7K LOC):

#### Direct Mode

Single agent, linear reasoning. Best for simple tasks.

#### Swarm Mode

Multiple agents with handoff. Inspired by [OpenAI Swarm](https://github.com/openai/swarm).

```
Agent A (Architect)
  → handoff with context → Agent B (Developer)
  → handoff with context → Agent C (Reviewer)
  → done
```

- Configurable handoff rules and conditions
- Context transfer: full, summary, or selective
- Max handoff limit (default: 10) to prevent loops

#### Expert Mode

Supervisor dispatches to a specialist pool, then evaluates results:

```
Supervisor
  → dispatch → Specialist 1 (Security Expert)
  → dispatch → Specialist 2 (Code Reviewer)
  → quality gate evaluation
  → synthesize final result
```

- Round-robin or AI-powered specialist selection
- Quality gates with threshold and composite evaluation
- Max dispatch limit (default: 5)

#### Graph Execution Engine

LangGraph-inspired state graph for complex workflows:

- **Nodes:** LLM calls, tool execution, custom functions
- **Edges:** Unconditional, conditional (dynamic routing), parallel
- **DAG scheduler:** Automatically parallelizes independent nodes
- **Checkpointing:** Memory or file-based persistence for recovery
- **Budget enforcement:** Per-node and per-graph limits

See [Orchestration](docs/orchestration.md) for the full guide.

---

### MCP Gateway

Full [Model Context Protocol](https://modelcontextprotocol.io/) support — both client and server.

**As client:** Connect to any MCP server and use its tools.

| Pre-configured Server | Namespace | Transport |
|----------------------|-----------|-----------|
| Filesystem | `fs` | stdio |
| Browser (Chrome ext.) | `browser` | SSE |
| macOS (AppleScript) | `mac` | stdio |
| Windows (GUI) | `win` | stdio |
| Video CLI | `videocli` | stdio |
| DaVinci Resolve | `davinci` | stdio |

**As server:** Expose Canal's tools (code execution, file ops, git, etc.) via MCP to Claude Desktop, Cursor, or any MCP client.

**Tool aggregation:** The agent sees a unified tool list from three sources — built-in tools, MCP tools, and plugin tools — all namespaced to avoid conflicts.

See [MCP Integration](docs/mcp-integration.md) for the full guide.

---

### Computer Vision

Screen-based automation via OmniParser (local ONNX) or UI-TARS (cloud via OpenRouter):

- **Element detection** — Identify UI elements with bounding boxes
- **Action pipeline** — Observe → detect → act loops
- **Actions** — Click, double-click, right-click, type, scroll, drag, key press
- **Workflow recording** — Record actions and replay them
- **Templates** — Generalize recordings into parameterized, reusable workflows
- **Screen monitoring** — Real-time change detection for verification

See [Computer Vision](docs/computer-vision.md) for setup.

---

### Semantic Memory

Vector-based caching and long-term memory:

- **Semantic cache** — Qdrant vector similarity search to avoid redundant LLM calls (0.92 similarity threshold, 1hr TTL)
- **Plan cache** — Reuse execution plans for repeated tasks (< 1ms lookup)
- **Unified memory store** — Typed entries (fact, learning, preference, reflection) with confidence scoring and time-based decay
- **Embedding providers** — Pluggable (local mock for testing, remote for production)

---

### Closed-Loop Learning

With the `learning` feature, Canal learns from execution outcomes:

1. **Experience collection** — Records tool sequences, success/failure, timing
2. **Pattern mining** — Discovers repeated success patterns and common failure modes
3. **Knowledge distillation** — Converts patterns into reusable skills
4. **Confidence decay** — Knowledge ages and is eventually forgotten

Resource limits: 10,000 experiences/user, configurable decay rate.

---

### Plugin System

Connector bundles package MCP servers + skills + prompts for domain workflows:

| Bundle | Domain |
|--------|--------|
| Customer Support | Ticket handling, knowledge base |
| Enterprise Search | Document retrieval, semantic search |
| Bio Research | Literature review, data analysis |
| Product Management | Spec writing, prioritization |
| Finance | Analysis, reporting |
| Legal | Contract review, compliance |
| Sales | CRM, outreach |
| Marketing | Content, campaigns |
| Productivity | Task management, scheduling |

Document processing plugins: PDF, DOCX, XLSX, PPTX.

---

## Feature Flags

Canal Engine uses Cargo feature flags. The default build is minimal — enable what you need:

| Feature | Enables | LOC |
|---------|---------|-----|
| `graph` | StateGraph execution engine (LangGraph-inspired) | 8.4K |
| `collaboration` | Swarm, Expert, Plan-Execute modes | 8.7K |
| `orchestration` | `graph` + `collaboration` | 17.1K |
| `cache` | Semantic cache + plan cache | 3.5K |
| `learning` | Closed-loop learning system | 3.2K |
| `jobs` | Async job queue with HITL | 3K |
| `multimodal` | Content-type detection, vision routing | — |
| `prompt-constraints` | Security constraint profiles | — |
| `context-engineering` | Context optimization, relevance scoring | — |
| `devtools` | LLM observability (Langfuse-style) | — |
| `database` | PostgreSQL persistence | — |

```bash
# Default (LLM routing + chat + MCP + agent + tools)
cargo build -p gateway-core

# Full orchestration
cargo build -p gateway-core --features "graph,collaboration,cache,learning,jobs,prompt-constraints,context-engineering,devtools"
```

---

## Performance Targets

| Metric | Target |
|--------|--------|
| Cold start | < 3s |
| Graph compilation | < 500ms |
| Per-node execution overhead | < 100ms |
| Semantic cache lookup | < 200ms |
| Plan cache lookup | < 1ms |
| Mode selection (Direct/Swarm/Expert) | < 50ms |

| Resource | Limit |
|----------|-------|
| Concurrent parallel graph nodes | 10 |
| Max graph depth | 5 levels |
| Max Swarm handoffs | 10 |
| Max Expert dispatches | 5 |
| Cached plans (LRU) | 1,000 |
| Learning experiences per user | 10,000 |
| Semantic cache TTL | 1 hour |

---

## Platform Integration

Canal Engine is the **AI capability layer**. It does LLM routing, agent execution, and orchestration. It does **not** do auth, billing, or infrastructure — you bring those.

### Compiles out of the box

| Crate | What it does |
|-------|-------------|
| `gateway-core` | Agent loop, chat, MCP, workflow, graph, collaboration, learning (72K LOC) |
| `gateway-llm` | LLM routing — Anthropic, OpenAI, Google, Qwen, OpenRouter, Ollama (9.7K LOC) |
| `gateway-tools` | Code execution — Python, Bash, Node.js, Go, Rust + Docker sandbox (16.5K LOC) |
| `gateway-memory` | Semantic cache (Qdrant), plan cache, unified memory (3.5K LOC) |
| `gateway-plugins` | Plugin and connector bundle management |
| `canal-cv` | Computer Vision — OmniParser, UI-TARS, screen automation |
| `devtools-core` | LLM observability — Langfuse-style tracing |

### Requires external crates

`gateway-api` (HTTP server) and `gateway-mcp-server` source code is included but requires:

- `canal-auth` — JWT authentication, Supabase integration
- `canal-identity` — API key management, agent identity

Not included in the open-source release. Two integration paths:

1. **Implement the traits** in `gateway-service-traits` and wire your own auth
2. **Use core crates as a library** in your own Axum/Actix server

---

## Configuration

```
.env.example              # API keys, database, JWT secret
config/
├── gateway.yaml          # Server, CORS, DB, rate limiting
├── llm-providers.yaml    # Provider config, health checks, cost control
├── model-profiles.yaml   # Routing profiles (7 presets)
├── mcp-servers.yaml      # MCP connections + tool permissions
├── memory.yaml           # Embedding model, recall, extraction
├── plugins.yaml          # Plugin catalog
├── constraints/          # Prompt security profiles
└── workflows/            # Workflow templates
```

See [Configuration](docs/configuration.md) for the full reference.

---

## Project Structure

```
canal-engine/
├── Cargo.toml              # Workspace (12 core crates active)
├── LICENSE                  # MIT
├── crates/                 # Rust crates
│   ├── gateway-core/       # AI engine (72K LOC)
│   ├── gateway-llm/        # LLM routing (9.7K LOC)
│   ├── gateway-tools/      # Code execution (16.5K LOC)
│   ├── gateway-memory/     # Memory & caching (3.5K LOC)
│   ├── gateway-plugins/    # Plugin system
│   ├── gateway-api/        # HTTP server (requires auth crates)
│   ├── gateway-mcp-server/ # MCP server (requires auth crates)
│   └── ...                 # 8 more crates
├── engine/
│   ├── canal-cv/           # Computer Vision
│   └── devtools-server/    # Observability server
├── config/                 # YAML configuration (21 files)
├── proto/                  # gRPC definitions (5 services)
├── plugins/                # Document processing (PDF, DOCX, XLSX, PPTX)
├── plugin-bundles/         # 10 domain-specific bundles
├── spec/                   # OpenAPI spec
├── migrations/             # PostgreSQL migrations
├── tests/                  # Integration tests
├── benches/                # Benchmarks
└── docs/                   # Documentation (20 files, EN + CN)
```

---

## Documentation

| Document | Description |
|----------|-------------|
| [Getting Started](docs/getting-started.md) | Installation, configuration, first build |
| [Architecture](docs/architecture.md) | Crate graph, data flow, security model |
| [API Reference](docs/api-reference.md) | All REST endpoints + gRPC services |
| [Configuration](docs/configuration.md) | Environment variables + YAML reference |
| [LLM Providers](docs/llm-providers.md) | Provider setup, routing strategies, cost control |
| [MCP Integration](docs/mcp-integration.md) | Client/server setup, tool permissions |
| [Orchestration](docs/orchestration.md) | Swarm, Expert, Graph, learning, async jobs |
| [Computer Vision](docs/computer-vision.md) | OmniParser, UI-TARS, workflow recording |
| [Agent Tools](docs/agent-tools.md) | Built-in tools, permissions, custom tools |
| [Contributing](docs/contributing.md) | Dev setup, code style, PR process |

All docs available in [Chinese (中文)](README_CN.md).

---

## Contributing

We welcome contributions. See [Contributing Guide](docs/contributing.md).

```bash
cargo check --all          # Verify compilation
cargo test --workspace     # Run tests
cargo clippy --all         # Lint
cargo fmt --all            # Format
```

Commit format: `type(scope): description` (e.g., `feat(llm): add DeepSeek provider`).

---

## License

[MIT](LICENSE)
