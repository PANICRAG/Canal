# Orchestration

Canal Engine provides advanced multi-agent orchestration through feature-gated modules. Enable with `--features orchestration` (or `full-orchestration`).

## Collaboration Modes

### Direct Mode

Single agent, linear reasoning. Best for simple tasks.

```bash
curl -X POST http://localhost:4000/api/graph/execute/direct \
  -H "Authorization: Bearer TOKEN" \
  -d '{"task": "Explain how async/await works in Rust"}'
```

### Swarm Mode

Multiple agents work in parallel with handoff. Inspired by OpenAI Swarm.

Each agent specializes in a domain. When one agent determines that another is better suited for the current subtask, it hands off with context.

```bash
curl -X POST http://localhost:4000/api/graph/execute/swarm \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "task": "Build a blog with authentication",
    "agents": ["architect", "backend-dev", "frontend-dev", "reviewer"]
  }'
```

Key concepts:
- **Handoff Rules** — Define when agent A should hand off to agent B
- **Context Transfer** — Shared state passes between agents
- **Max Handoffs** — Configurable limit (default: 10) to prevent loops

### Expert Mode

A supervisor agent dispatches to a pool of specialist agents, then evaluates results through quality gates.

```bash
curl -X POST http://localhost:4000/api/graph/execute/expert \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "task": "Analyze this codebase for security vulnerabilities",
    "specialists": ["security-expert", "code-reviewer", "dependency-auditor"]
  }'
```

Key concepts:
- **Supervisor** — Plans which specialists to invoke
- **Specialist Pool** — Domain experts with specific tools/prompts
- **Quality Gates** — Threshold or composite validation of results
- **Max Dispatches** — Configurable limit (default: 5)

### Auto Mode

Let Canal select the best mode based on task analysis.

```bash
curl -X POST http://localhost:4000/api/graph/execute/auto \
  -H "Authorization: Bearer TOKEN" \
  -d '{"task": "Your task description"}'
```

The task classifier considers complexity, domain breadth, and risk level to choose Direct, Swarm, or Expert.

## Graph Execution Engine

The graph engine (feature: `graph`) provides LangGraph-inspired state graph execution.

### Core Concepts

- **StateGraph** — Define nodes and edges
- **Node** — A computation step (LLM call, tool execution, custom function)
- **Edge** — Transition between nodes (unconditional or conditional)
- **Parallel Nodes** — Execute multiple nodes concurrently
- **Checkpointing** — Save/restore execution state

### Building a Graph

```rust
use gateway_core::graph::{StateGraphBuilder, GraphExecutor};

let graph = StateGraphBuilder::new()
    .add_node("analyze", analyze_fn)
    .add_node("plan", plan_fn)
    .add_node("execute", execute_fn)
    .add_node("review", review_fn)
    .add_edge("analyze", "plan")
    .add_conditional_edge("plan", |state| {
        if state.needs_execution { "execute" } else { "review" }
    })
    .add_edge("execute", "review")
    .set_entry("analyze")
    .set_finish("review")
    .build()?;

let executor = GraphExecutor::new(graph);
let result = executor.execute(initial_state).await?;
```

### DAG Scheduling

The graph engine automatically parallelizes independent nodes:

```
   analyze
   /     \
plan_a  plan_b    ← executed in parallel
   \     /
   merge
     |
   execute
```

### Checkpointing

Save execution state for recovery:

```rust
// Memory-based (fast, lost on restart)
let checkpointer = MemoryCheckpointer::new();

// File-based (persistent)
let checkpointer = FileCheckpointer::new("/tmp/checkpoints");
```

### Budget Enforcement

Per-node and per-graph budget limits:

| Resource | Default Limit |
|----------|--------------|
| Max concurrent nodes | 10 |
| Max graph depth | 5 levels |
| Graph compilation time | < 500ms |
| Per-node execution | < 100ms overhead |

## Closed-Loop Learning

With the `learning` feature, Canal learns from execution outcomes:

1. **Experience Collection** — Records what worked and what didn't
2. **Pattern Mining** — Discovers repeated success/failure patterns
3. **Knowledge Distillation** — Converts patterns into reusable skills
4. **Confidence Decay** — Knowledge ages and is eventually forgotten

```bash
# Check learning status
curl http://localhost:4000/api/learning/status \
  -H "Authorization: Bearer TOKEN"

# Query learned knowledge
curl http://localhost:4000/api/learning/knowledge \
  -H "Authorization: Bearer TOKEN"
```

Resource limits:
- Max learning experiences: 10,000 per user
- Confidence decay: configurable rate

## Async Jobs

With the `jobs` feature, long-running tasks run asynchronously:

```bash
# Submit a job
curl -X POST http://localhost:4000/api/jobs \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "task": "Generate a comprehensive analysis...",
    "webhook_url": "https://your-server.com/webhook"
  }'

# Stream progress
curl -N http://localhost:4000/api/jobs/JOB_ID/stream \
  -H "Authorization: Bearer TOKEN"

# Human-in-the-loop input
curl -X POST http://localhost:4000/api/jobs/JOB_ID/input \
  -H "Authorization: Bearer TOKEN" \
  -d '{"input": "Approved, proceed with option B"}'
```
