# Architecture

## Design Principles

1. **AI-Only** — Canal Engine is a pure AI capability layer. No infrastructure management, no container orchestration, no billing logic.
2. **Trait-Based Abstraction** — Every subsystem uses traits for pluggability. Custom LLM providers, storage backends, executors, and observers can be swapped in.
3. **Feature-Gated Modularity** — Advanced features are Cargo feature flags. The default build is minimal; enable what you need.
4. **Zero Breaking Changes** — Existing endpoints work regardless of which features are enabled.
5. **Async-First** — Built on Tokio with full streaming support throughout.

## Crate Dependency Graph

```
                    gateway-api (HTTP server)
                         │
                    gateway-core (AI engine)
                   ╱     │     ╲        ╲
          gateway-llm  gateway-memory  gateway-plugins
              │           │
     gateway-tool-types   │
              │           │
         gateway-tools    │
                          │
                    canal-cv (vision)

    gateway-mcp-server ──── gateway-core
    gateway-service-traits ─ gateway-core
    canal-module ─────────── (standalone)
    canal-proto ──────────── (standalone)
    canal-rpc ────────────── (standalone)
    devtools-core ────────── (standalone)
```

## Core Crates

### gateway-core

The heart of the engine. Contains:

- **Agent Loop** (`agent/`) — Autonomous agent with tool calling, intent recognition, task planning, and step execution. Supports streaming and permission management.
- **Chat Engine** (`chat/`) — Conversation management with session persistence, artifact extraction, and multi-turn context.
- **MCP Gateway** (`mcp/`) — Full MCP client connecting to external servers, plus tool aggregation across all sources (built-in + MCP + plugins).
- **Workflow Engine** (`workflow/`) — DAG-based workflow execution with checkpoint/recovery, recording, and template generation.
- **Graph Executor** (`graph/`, feature-gated) — LangGraph-inspired state graph execution. Nodes, edges, conditional routing, parallel execution, and checkpointing.
- **Collaboration** (`collaboration/`, feature-gated) — Multi-agent modes: Direct, Swarm (parallel handoff), Expert (hierarchical dispatch).
- **Learning System** (`learning/`, feature-gated) — Closed-loop: experience collection, pattern mining, knowledge distillation.
- **Session Manager** (`session/`) — User memory with file/in-memory backends, context compaction.
- **Role System** (`roles/`) — Role-based tool filtering, permission modes, constraint profiles.
- **Computer Use** (`computer_use/`) — Screen automation via CV engine integration.
- **Context Manager** (`context/`) — Token budgeting and summarization.

### gateway-api

Axum-based HTTP server. Organized as route modules:

```
routes/
├── chat.rs          # /api/chat/*
├── agent.rs         # /api/agent/*
├── tools.rs         # /api/tools/*
├── mcp.rs           # /api/mcp/*
├── code.rs          # /api/code/*
├── filesystem.rs    # /api/filesystem/*
├── git.rs           # /api/git/*
├── workflow.rs       # /api/workflows/*
├── graph.rs         # /api/graph/* (feature-gated)
├── memory.rs        # /api/memory/*
├── sessions.rs      # /api/sessions/*
├── artifacts.rs     # /api/artifacts/*
├── connectors.rs    # /api/connectors/*
├── admin.rs         # /api/admin/*
├── devtools.rs      # /api/devtools/* (feature-gated)
└── ...
```

Middleware stack: Auth (JWT/API key) → RBAC → Rate limiting → Logging → Security headers.

### gateway-llm

Multi-provider LLM abstraction:

```
providers/
├── anthropic.rs     # Claude models
├── openai.rs        # GPT models
├── google.rs        # Gemini models
├── openrouter.rs    # OpenRouter proxy
├── qwen.rs          # Qwen/DashScope
└── ollama.rs        # Local models
```

**Routing engine** selects the optimal provider/model based on configurable strategies: primary fallback, cascade, A/B test, task-type rules, AI-powered selection, multimodal content routing.

**Health tracking** with circuit breaker pattern — providers are automatically removed from routing when unhealthy.

**Cost tracking** per model/provider with configurable daily budgets.

### gateway-tools

Code execution and filesystem access:

```
executor/
├── mod.rs           # Execution router
├── python.rs        # Python (Docker/subprocess)
├── bash.rs          # Bash/shell
├── nodejs.rs        # Node.js/TypeScript
├── golang.rs        # Go
├── rust_exec.rs     # Rust
├── docker.rs        # Docker container execution
├── codeact.rs       # CodeAct stateful sessions
├── security.rs      # Security validation
└── pool.rs          # Container pooling

filesystem/
├── reader.rs        # File reading
├── writer.rs        # File writing
├── search.rs        # File search
└── permissions.rs   # Access control
```

### gateway-memory

Semantic caching and unified memory:

- **Semantic Cache** — Vector-based similarity search for past queries. Avoids redundant LLM calls.
- **Plan Cache** — Caches execution plans for repeated tasks. LRU with configurable TTL.
- **Unified Memory Store** — Typed memory entries (fact, learning, preference, reflection) with confidence scoring and decay.
- **Embedding Providers** — Pluggable embedding generation (local mock for testing, remote for production).

### canal-cv

Computer Vision engine:

- **Screen Capture** — `ScreenController` trait for desktop and browser.
- **Element Detection** — `VisionDetector` trait with OmniParser (ONNX) and Molmo implementations.
- **Action Pipeline** — `ComputerUsePipeline` for observe → detect → act loops.
- **Workflow Recording** — Record user actions, replay them, and generalize into templates.
- **Screen Monitoring** — Real-time change detection for automation verification.

### canal-module

Composable deployment architecture:

- `CanalModule` trait — Every module implements `routes()`, `health()`, `shutdown()`.
- `SharedContext` — Minimal shared state across modules.
- `ModuleFlags` — Deployment profiles: `platform`, `engine-full`, `engine-lite`, `all`.

This enables running Canal as a single binary or distributed across multiple services.

## Feature Flag Architecture

```
full-orchestration
├── orchestration
│   ├── graph           # StateGraph executor
│   └── collaboration   # Swarm, Expert, Direct modes
├── intelligence
│   ├── multimodal      # Content-type routing
│   ├── cache           # Semantic + plan cache
│   └── learning        # Closed-loop learning
├── jobs                # Async job queue
├── prompt-constraints  # Security constraints
├── context-engineering # Context optimization
├── billing             # Usage tracking
├── devtools            # Observability
└── database            # PostgreSQL persistence
```

Each feature is independently toggleable. Dependencies are explicit in `Cargo.toml`.

## Data Flow

### Chat Request

```
Client → HTTP → Auth Middleware → Chat Route
  → Context Manager (token budgeting)
  → Semantic Cache (check for similar past query)
  → LLM Router (select provider/model)
  → Provider (Anthropic/OpenAI/Qwen/...)
  → Response → Artifact Extraction → Cache Update
  → Stream back to client (SSE)
```

### Agent Execution

```
Client → Agent Route → AgentRunner
  → Intent Recognition
  → Task Planning (if complex)
  → Loop:
      → Select Tool
      → Permission Check
      → Execute Tool
      → Observe Result
      → Decide: continue or complete
  → Session Save
  → Response
```

### Graph Execution

```
Client → Graph Route → Mode Selection
  → Direct / Swarm / Expert
  → StateGraph compilation
  → Loop:
      → Schedule next nodes (DAG)
      → Execute nodes (parallel where possible)
      → Evaluate edges (conditional routing)
      → Checkpoint (if enabled)
  → Quality Gate (Expert mode)
  → Learning Record (if enabled)
  → Response
```

## gRPC Services

For distributed deployment, Canal exposes gRPC services defined in `proto/`:

| Service | File | Purpose |
|---------|------|---------|
| AgentService | `agent_service.proto` | Streaming agent chat |
| LlmService | `llm_service.proto` | LLM routing |
| ToolService | `tool_service.proto` | Tool execution |
| MemoryService | `memory_service.proto` | Memory storage |

The `gateway-service-traits` crate defines trait boundaries that can be backed by either local implementations or remote gRPC calls.

## Security Model

- **Authentication** — JWT (RS256), API keys, Supabase JWT.
- **Authorization** — Role-based access control (RBAC) with admin/user roles.
- **IDOR Prevention** — Memory and session endpoints validate ownership.
- **Tool Permissions** — Per-tool confirmation requirements, blocked tools, rate limits.
- **Code Execution** — Docker sandboxing with resource limits (CPU, memory, timeout).
- **Prompt Constraints** — Blocked patterns (`.env`, `.pem`), blocked commands (`rm -rf`, `sudo`), confirmation requirements (`git push --force`).
