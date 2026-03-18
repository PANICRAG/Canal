# API 参考

Canal Engine 在 4000 端口暴露 REST API，分布式部署时还提供 gRPC 服务。

**认证：** 所有受保护端点需要 `Authorization: Bearer <JWT_TOKEN>` 请求头。

---

## 对话

### POST /api/chat

发送聊天消息。

```bash
curl -X POST http://localhost:4000/api/chat \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "message": "你好",
    "stream": false,
    "conversation_id": "可选ID",
    "profile_id": "default",
    "task_type": "general"
  }'
```

### POST /api/chat/stream

流式聊天响应（SSE）。

```bash
curl -N -X POST http://localhost:4000/api/chat/stream \
  -H "Authorization: Bearer TOKEN" \
  -d '{"message": "写一首诗", "stream": true}'
```

SSE 事件格式：

```
data: {"type": "text", "content": "这是"}
data: {"type": "text", "content": "一首诗..."}
data: {"type": "done", "usage": {"input_tokens": 10, "output_tokens": 50}}
```

### GET /api/chat/conversations

列出所有对话。

### POST /api/chat/conversations

创建新对话。

### GET /api/chat/conversations/{id}/messages

获取对话中的消息。

---

## Agent

### POST /api/agent/query

执行 Agent 查询，支持工具调用。

```bash
curl -X POST http://localhost:4000/api/agent/query \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "message": "创建一个下载网页的 Python 脚本",
    "session_id": "可选会话ID"
  }'
```

### POST /api/agent/stream

流式 Agent 执行（SSE）。

事件类型：`TextChunk`、`ThinkingChunk`、`ToolCall`、`ToolResult`、`Done`、`Error`。

### GET /api/agent/sessions/{session_id}/permissions

获取会话的待处理权限请求。

### POST /api/agent/sessions/{session_id}/permissions/respond

批准或拒绝待处理权限。

```bash
curl -X POST http://localhost:4000/api/agent/sessions/SESSION_ID/permissions/respond \
  -H "Authorization: Bearer TOKEN" \
  -d '{"permission_id": "perm-123", "approved": true}'
```

---

## 工具

### GET /api/tools

列出所有可用工具（内置 + MCP + 插件）。

### POST /api/tools/{namespace}/{name}/call

执行指定工具。

```bash
curl -X POST http://localhost:4000/api/tools/agent/bash/call \
  -H "Authorization: Bearer TOKEN" \
  -d '{"command": "echo hello"}'
```

---

## MCP 服务器

### GET /api/mcp/servers

列出所有已连接的 MCP 服务器。

### POST /api/mcp/servers

动态添加新 MCP 服务器。

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

列出指定 MCP 服务器的工具。

### DELETE /api/mcp/servers/{name}

移除 MCP 服务器。

---

## 代码执行

### POST /api/code/execute

在沙箱环境中执行代码。

```bash
curl -X POST http://localhost:4000/api/code/execute \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "language": "python",
    "code": "import math\nprint(math.pi)"
  }'
```

支持语言：`python`、`bash`、`nodejs`、`golang`、`rust`、`codeact`。

### POST /api/code/execute/stream

流式执行输出（SSE）。

### GET /api/code/languages

列出支持的语言及其能力。

---

## 文件系统

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

### GET /api/git/status | GET /api/git/diff

### POST /api/git/commit

```bash
curl -X POST http://localhost:4000/api/git/commit \
  -H "Authorization: Bearer TOKEN" \
  -d '{"message": "feat: 新增功能", "files": ["src/main.rs"]}'
```

### POST /api/git/branch | GET /api/git/branches | POST /api/git/pull | POST /api/git/push

---

## 工作流

### POST /api/workflows — 创建工作流
### POST /api/workflows/{id}/execute — 执行工作流
### POST /api/workflows/record/start — 开始录制
### POST /api/workflows/record/stop — 停止录制并保存
### GET /api/workflows/templates — 列出模板
### POST /api/workflows/templates/{id}/execute — 执行模板

---

## 图执行（feature: `orchestration`）

### POST /api/graph/execute/auto

自动选择最佳协作模式并执行。

```bash
curl -X POST http://localhost:4000/api/graph/execute/auto \
  -H "Authorization: Bearer TOKEN" \
  -d '{"task": "调研并总结 2025 年 AI 趋势"}'
```

### POST /api/graph/execute/direct — 单 Agent 执行
### POST /api/graph/execute/swarm — 多 Agent 并行执行

```bash
curl -X POST http://localhost:4000/api/graph/execute/swarm \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "task": "构建着陆页",
    "agents": ["designer", "developer", "reviewer"]
  }'
```

### POST /api/graph/execute/expert — 层级专家分派

---

## 记忆

### POST /api/memory/semantic-search/{user_id}

语义相似度搜索。

```bash
curl -X POST http://localhost:4000/api/memory/semantic-search/USER_ID \
  -H "Authorization: Bearer TOKEN" \
  -d '{"query": "如何部署到生产环境", "limit": 5}'
```

### POST /api/memory/entries/{user_id} — 创建记忆条目
### GET /api/memory/entries/{user_id} — 列出记忆条目
### GET /api/memory/stats/{user_id} — 记忆统计

---

## 会话

### GET /api/sessions — 列出会话
### POST /api/sessions/{session_id}/checkpoints — 创建检查点
### POST /api/sessions/{session_id}/checkpoints/{checkpoint_id}/restore — 恢复检查点

---

## 异步任务（feature: `jobs`）

### POST /api/jobs — 提交新任务

```bash
curl -X POST http://localhost:4000/api/jobs \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "task": "生成一份综合报告...",
    "webhook_url": "https://your-server.com/webhook"
  }'
```

### GET /api/jobs/{id}/stream — 流式任务进度
### POST /api/jobs/{id}/input — 提交人在回路输入

---

## 连接器

### GET /api/connectors/catalog — 浏览所有连接器
### POST /api/connectors/{name}/install — 安装连接器
### GET /api/connectors/installed — 列出已安装连接器

---

## 管理

### GET /api/admin/status — 平台状态
### POST /api/admin/keys — 创建 API Key

```bash
curl -X POST http://localhost:4000/api/admin/keys \
  -H "Authorization: Bearer ADMIN_TOKEN" \
  -d '{"name": "my-api-key", "scopes": ["chat", "tools"]}'
```

---

## DevTools（feature: `devtools`）

### GET /api/devtools/v1/traces — 列出执行追踪
### GET /api/devtools/v1/traces/{id} — 获取追踪详情
### GET /api/devtools/v1/sse/global — SSE 实时追踪流

---

## 健康检查

### GET /api/health — 基础健康检查
### GET /api/health/ready — Kubernetes 就绪探针
### GET /api/health/live — Kubernetes 存活探针

---

## gRPC 服务

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

完整 schema 定义见 `proto/` 目录。
