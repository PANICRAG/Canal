# API Reference

Canal Engine exposes a REST API on port 4000 and gRPC services for distributed deployment.

**Authentication:** All protected endpoints require `Authorization: Bearer <JWT_TOKEN>` header.

---

## Chat

### POST /api/chat

Send a chat message.

```bash
curl -X POST http://localhost:4000/api/chat \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "message": "Hello",
    "stream": false,
    "conversation_id": "optional-id",
    "profile_id": "default",
    "task_type": "general"
  }'
```

### POST /api/chat/stream

Streaming chat response (SSE).

```bash
curl -N -X POST http://localhost:4000/api/chat/stream \
  -H "Authorization: Bearer TOKEN" \
  -d '{"message": "Write a poem", "stream": true}'
```

SSE event format:

```
data: {"type": "text", "content": "Here is"}
data: {"type": "text", "content": " a poem..."}
data: {"type": "done", "usage": {"input_tokens": 10, "output_tokens": 50}}
```

### GET /api/chat/conversations

List all conversations.

### POST /api/chat/conversations

Create a new conversation.

### GET /api/chat/conversations/{id}/messages

Get messages in a conversation.

---

## Agent

### POST /api/agent/query

Execute an agent query with tool calling.

```bash
curl -X POST http://localhost:4000/api/agent/query \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "message": "Create a Python script that downloads a webpage",
    "session_id": "optional-session-id"
  }'
```

### POST /api/agent/stream

Streaming agent execution (SSE).

Events: `TextChunk`, `ThinkingChunk`, `ToolCall`, `ToolResult`, `Done`, `Error`.

### GET /api/agent/sessions/{session_id}/permissions

Get pending permission requests for a session.

### POST /api/agent/sessions/{session_id}/permissions/respond

Approve or deny a pending permission.

```bash
curl -X POST http://localhost:4000/api/agent/sessions/SESSION_ID/permissions/respond \
  -H "Authorization: Bearer TOKEN" \
  -d '{"permission_id": "perm-123", "approved": true}'
```

---

## Tools

### GET /api/tools

List all available tools (built-in + MCP + plugins).

```bash
curl http://localhost:4000/api/tools -H "Authorization: Bearer TOKEN"
```

### POST /api/tools/{namespace}/{name}/call

Execute a specific tool.

```bash
curl -X POST http://localhost:4000/api/tools/agent/bash/call \
  -H "Authorization: Bearer TOKEN" \
  -d '{"command": "echo hello"}'
```

---

## MCP Servers

### GET /api/mcp/servers

List all connected MCP servers.

### POST /api/mcp/servers

Add a new MCP server dynamically.

```bash
curl -X POST http://localhost:4000/api/mcp/servers \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "name": "my-server",
    "transport": "stdio",
    "command": "npx",
    "args": ["-y", "my-mcp-package"]
  }'
```

### GET /api/mcp/servers/{name}/tools

List tools from a specific MCP server.

### DELETE /api/mcp/servers/{name}

Remove an MCP server.

---

## Code Execution

### POST /api/code/execute

Execute code in a sandboxed environment.

```bash
curl -X POST http://localhost:4000/api/code/execute \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "language": "python",
    "code": "import math\nprint(math.pi)"
  }'
```

Supported languages: `python`, `bash`, `nodejs`, `golang`, `rust`, `codeact`.

### POST /api/code/execute/stream

Stream execution output (SSE).

### GET /api/code/languages

List supported languages with their capabilities.

---

## Filesystem

### POST /api/filesystem/read

```bash
curl -X POST http://localhost:4000/api/filesystem/read \
  -H "Authorization: Bearer TOKEN" \
  -d '{"path": "/path/to/file.txt"}'
```

### POST /api/filesystem/write

```bash
curl -X POST http://localhost:4000/api/filesystem/write \
  -H "Authorization: Bearer TOKEN" \
  -d '{"path": "/path/to/file.txt", "content": "Hello World"}'
```

### POST /api/filesystem/search

```bash
curl -X POST http://localhost:4000/api/filesystem/search \
  -H "Authorization: Bearer TOKEN" \
  -d '{"pattern": "*.rs", "directory": "/path/to/project"}'
```

---

## Git

### POST /api/git/clone

```bash
curl -X POST http://localhost:4000/api/git/clone \
  -H "Authorization: Bearer TOKEN" \
  -d '{"url": "https://github.com/user/repo.git"}'
```

### GET /api/git/status

### GET /api/git/diff

### POST /api/git/commit

```bash
curl -X POST http://localhost:4000/api/git/commit \
  -H "Authorization: Bearer TOKEN" \
  -d '{"message": "feat: add new feature", "files": ["src/main.rs"]}'
```

### POST /api/git/branch

### GET /api/git/branches

### POST /api/git/pull

### POST /api/git/push

---

## Workflows

### POST /api/workflows

Create a workflow.

### POST /api/workflows/{id}/execute

Execute a workflow.

### POST /api/workflows/record/start

Start recording user actions into a workflow.

### POST /api/workflows/record/stop

Stop recording and save the workflow.

### GET /api/workflows/templates

List available workflow templates.

### POST /api/workflows/templates/{id}/execute

Execute a saved template.

---

## Graph Execution (feature: `orchestration`)

### POST /api/graph/execute/auto

Auto-select the best collaboration mode and execute.

```bash
curl -X POST http://localhost:4000/api/graph/execute/auto \
  -H "Authorization: Bearer TOKEN" \
  -d '{"task": "Research and summarize AI trends in 2025"}'
```

### POST /api/graph/execute/direct

Single-agent execution.

### POST /api/graph/execute/swarm

Multi-agent parallel execution with handoff.

```bash
curl -X POST http://localhost:4000/api/graph/execute/swarm \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "task": "Build a landing page",
    "agents": ["designer", "developer", "reviewer"]
  }'
```

### POST /api/graph/execute/expert

Hierarchical expert dispatch.

---

## Memory

### POST /api/memory/search/{user_id}

Full-text search memory.

### POST /api/memory/semantic-search/{user_id}

Semantic similarity search.

```bash
curl -X POST http://localhost:4000/api/memory/semantic-search/USER_ID \
  -H "Authorization: Bearer TOKEN" \
  -d '{"query": "How to deploy to production", "limit": 5}'
```

### POST /api/memory/entries/{user_id}

Create a memory entry.

```bash
curl -X POST http://localhost:4000/api/memory/entries/USER_ID \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "key": "deployment-steps",
    "category": "fact",
    "content": "Production deployment requires...",
    "confidence": 0.9
  }'
```

### GET /api/memory/entries/{user_id}

List memory entries.

### GET /api/memory/stats/{user_id}

Memory statistics.

---

## Sessions

### GET /api/sessions

List all sessions.

### POST /api/sessions/{session_id}/checkpoints

Create a session checkpoint.

### POST /api/sessions/{session_id}/checkpoints/{checkpoint_id}/restore

Restore a session to a checkpoint.

---

## Async Jobs (feature: `jobs`)

### POST /api/jobs

Submit a new async job.

```bash
curl -X POST http://localhost:4000/api/jobs \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "task": "Generate a comprehensive report on...",
    "webhook_url": "https://your-server.com/webhook"
  }'
```

### GET /api/jobs/{id}/stream

Stream job progress (SSE).

### POST /api/jobs/{id}/input

Submit human-in-the-loop input.

---

## Connectors

### GET /api/connectors/catalog

Browse all available connectors.

### POST /api/connectors/{name}/install

Install a connector.

### GET /api/connectors/installed

List installed connectors.

---

## Admin

### GET /api/admin/status

Platform status overview.

### POST /api/admin/keys

Create an API key.

```bash
curl -X POST http://localhost:4000/api/admin/keys \
  -H "Authorization: Bearer ADMIN_TOKEN" \
  -d '{"name": "my-api-key", "scopes": ["chat", "tools"]}'
```

### GET /api/admin/config/providers

List configured LLM providers.

---

## DevTools (feature: `devtools`)

### GET /api/devtools/v1/traces

List execution traces.

### GET /api/devtools/v1/traces/{id}

Get a specific trace with all spans and generations.

### GET /api/devtools/v1/sse/global

SSE stream of all trace events (real-time monitoring).

---

## Health

### GET /api/health

Basic health check.

### GET /api/health/ready

Kubernetes readiness probe.

### GET /api/health/live

Kubernetes liveness probe.

---

## gRPC Services

### AgentService

```protobuf
service AgentService {
  rpc Chat(AgentChatRequest) returns (stream AgentEvent);
  rpc Health(HealthRequest) returns (HealthResponse);
}
```

### LlmService

```protobuf
service LlmService {
  rpc Chat(ChatRequest) returns (ChatResponse);
  rpc ChatStream(ChatRequest) returns (stream ChatStreamEvent);
  rpc ListProviders(ListProvidersRequest) returns (ListProvidersResponse);
}
```

### ToolService

```protobuf
service ToolService {
  rpc Execute(ExecuteRequest) returns (ExecuteResponse);
  rpc ListTools(ListToolsRequest) returns (ListToolsResponse);
}
```

### MemoryService

```protobuf
service MemoryService {
  rpc Store(StoreRequest) returns (StoreResponse);
  rpc Query(QueryRequest) returns (QueryResponse);
  rpc Delete(DeleteRequest) returns (DeleteResponse);
}
```

See `proto/` directory for full schema definitions.
