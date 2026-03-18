# 架构设计

## 设计原则

1. **纯 AI 层** — Canal Engine 是纯粹的 AI 能力层，不涉及基础设施管理、容器编排或计费逻辑。
2. **基于 Trait 的抽象** — 每个子系统都使用 trait 实现可插拔。自定义 LLM Provider、存储后端、执行器和观察器可以自由替换。
3. **Feature 门控模块化** — 高级功能通过 Cargo feature flag 控制。默认构建最小化，按需启用。
4. **零破坏性变更** — 无论启用哪些 feature，已有端点始终可用。
5. **异步优先** — 基于 Tokio 构建，全链路流式支持。

## Crate 依赖图

```
                    gateway-api（HTTP 服务）
                         │
                    gateway-core（AI 引擎）
                   ╱     │     ╲        ╲
          gateway-llm  gateway-memory  gateway-plugins
              │           │
     gateway-tool-types   │
              │           │
         gateway-tools    │
                          │
                    canal-cv（视觉）

    gateway-mcp-server ──── gateway-core
    gateway-service-traits ─ gateway-core
    canal-module ─────────── （独立）
    canal-proto ──────────── （独立）
    canal-rpc ────────────── （独立）
    devtools-core ────────── （独立）
```

## 核心 Crate

### gateway-core

引擎核心，包含：

- **Agent 循环** (`agent/`) — 自主 Agent，支持工具调用、意图识别、任务规划和步骤执行。支持流式和权限管理。
- **对话引擎** (`chat/`) — 对话管理，会话持久化、产物提取和多轮上下文。
- **MCP 网关** (`mcp/`) — 完整 MCP 客户端，连接外部服务器。跨来源工具聚合（内置 + MCP + 插件）。
- **工作流引擎** (`workflow/`) — DAG 工作流执行，检查点/恢复、录制和模板生成。
- **图执行器** (`graph/`, feature 门控) — LangGraph 风格的状态图执行。节点、边、条件路由、并行执行和检查点。
- **协作** (`collaboration/`, feature 门控) — 多智能体模式：Direct、Swarm（并行移交）、Expert（层级分派）。
- **学习系统** (`learning/`, feature 门控) — 闭环学习：经验收集、模式挖掘、知识蒸馏。
- **会话管理** (`session/`) — 用户记忆，文件/内存后端，上下文压缩。
- **角色系统** (`roles/`) — 基于角色的工具过滤、权限模式、约束配置。
- **计算机使用** (`computer_use/`) — 通过 CV 引擎集成的屏幕自动化。
- **上下文管理** (`context/`) — Token 预算和摘要。

### gateway-api

基于 Axum 的 HTTP 服务。按路由模块组织：

```
routes/
├── chat.rs          # /api/chat/*
├── agent.rs         # /api/agent/*
├── tools.rs         # /api/tools/*
├── mcp.rs           # /api/mcp/*
├── code.rs          # /api/code/*
├── filesystem.rs    # /api/filesystem/*
├── git.rs           # /api/git/*
├── workflow.rs      # /api/workflows/*
├── graph.rs         # /api/graph/*（feature 门控）
├── memory.rs        # /api/memory/*
├── sessions.rs      # /api/sessions/*
├── artifacts.rs     # /api/artifacts/*
├── connectors.rs    # /api/connectors/*
├── admin.rs         # /api/admin/*
├── devtools.rs      # /api/devtools/*（feature 门控）
└── ...
```

中间件栈：认证（JWT/API Key）→ RBAC → 限流 → 日志 → 安全头。

### gateway-llm

多 Provider LLM 抽象：

```
providers/
├── anthropic.rs     # Claude 模型
├── openai.rs        # GPT 模型
├── google.rs        # Gemini 模型
├── openrouter.rs    # OpenRouter 代理
├── qwen.rs          # Qwen/通义千问
└── ollama.rs        # 本地模型
```

**路由引擎**根据配置策略选择最优 Provider/模型：主备降级、级联、A/B 测试、任务类型规则、AI 自动选择、多模态内容路由。

**健康追踪**采用熔断器模式 — 不健康的 Provider 自动从路由中移除。

**成本追踪**按模型/Provider 记录，支持每日预算配置。

### gateway-tools

代码执行和文件系统访问：

- **执行器** — Python（Docker/子进程）、Bash、Node.js、Go、Rust、CodeAct（有状态会话）
- **文件系统** — 安全的文件读写搜索，权限控制
- **安全验证** — 危险模式检测、资源限制

### gateway-memory

语义缓存和统一记忆：

- **语义缓存** — 基于向量的相似度搜索，避免重复 LLM 调用
- **计划缓存** — 缓存执行计划用于重复任务，LRU + 可配置 TTL
- **统一记忆存储** — 类型化记忆条目（事实、学习、偏好、反思），置信度评分和衰减
- **嵌入 Provider** — 可插拔的嵌入生成（测试用本地 mock，生产用远程）

### canal-cv

计算机视觉引擎：

- **屏幕捕获** — `ScreenController` trait，桌面和浏览器
- **元素检测** — `VisionDetector` trait，OmniParser（ONNX）和 Molmo 实现
- **操作流水线** — `ComputerUsePipeline`，观察 → 检测 → 执行循环
- **工作流录制** — 录制用户操作、回放、泛化为模板
- **屏幕监控** — 实时变化检测，用于自动化验证

### canal-module

可组合部署架构：

- `CanalModule` trait — 每个模块实现 `routes()`、`health()`、`shutdown()`
- `SharedContext` — 跨模块的最小共享状态
- `ModuleFlags` — 部署配置：`platform`、`engine-full`、`engine-lite`、`all`

支持单体二进制部署或分布式多服务部署。

## Feature Flag 架构

```
full-orchestration
├── orchestration
│   ├── graph           # StateGraph 执行器
│   └── collaboration   # Swarm、Expert、Direct 模式
├── intelligence
│   ├── multimodal      # 内容类型路由
│   ├── cache           # 语义 + 计划缓存
│   └── learning        # 闭环学习
├── jobs                # 异步任务队列
├── prompt-constraints  # 安全约束
├── context-engineering # 上下文优化
├── billing             # 用量追踪
├── devtools            # 可观测性
└── database            # PostgreSQL 持久化
```

## 数据流

### 聊天请求

```
客户端 → HTTP → 认证中间件 → Chat 路由
  → 上下文管理（Token 预算）
  → 语义缓存（检查相似历史查询）
  → LLM 路由（选择 Provider/模型）
  → Provider（Anthropic/OpenAI/Qwen/...）
  → 响应 → 产物提取 → 缓存更新
  → 流式返回客户端（SSE）
```

### Agent 执行

```
客户端 → Agent 路由 → AgentRunner
  → 意图识别
  → 任务规划（复杂任务时）
  → 循环：
      → 选择工具
      → 权限检查
      → 执行工具
      → 观察结果
      → 决策：继续或完成
  → 会话保存
  → 响应
```

### 图执行

```
客户端 → Graph 路由 → 模式选择
  → Direct / Swarm / Expert
  → StateGraph 编译
  → 循环：
      → 调度下一批节点（DAG）
      → 执行节点（可并行）
      → 评估边（条件路由）
      → 检查点（如启用）
  → 质量门（Expert 模式）
  → 学习记录（如启用）
  → 响应
```

## gRPC 服务

分布式部署时，Canal 通过 `proto/` 定义暴露 gRPC 服务：

| 服务 | 文件 | 用途 |
|------|------|------|
| AgentService | `agent_service.proto` | 流式 Agent 对话 |
| LlmService | `llm_service.proto` | LLM 路由 |
| ToolService | `tool_service.proto` | 工具执行 |
| MemoryService | `memory_service.proto` | 记忆存储 |

`gateway-service-traits` crate 定义了 trait 边界，可以由本地实现或远程 gRPC 调用支撑。

## 安全模型

- **认证** — JWT（RS256）、API Key、Supabase JWT
- **授权** — 基于角色的访问控制（RBAC），管理员/用户角色
- **IDOR 防护** — 记忆和会话端点验证所有权
- **工具权限** — 逐工具确认要求、工具黑名单、频率限制
- **代码执行** — Docker 沙箱，资源限制（CPU、内存、超时）
- **提示词约束** — 封锁模式（`.env`、`.pem`）、封锁命令（`rm -rf`、`sudo`）、确认要求（`git push --force`）
