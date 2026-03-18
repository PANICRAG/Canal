<p align="center">
  <h1 align="center">Canal Engine</h1>
  <p align="center">
    基于 Rust 的开源 AI 编排引擎。<br>
    LLM 路由 · Agent 循环 · MCP · 多智能体协作 · 计算机视觉。
  </p>
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="docs/getting-started_cn.md">快速开始</a> ·
  <a href="docs/architecture_cn.md">架构设计</a> ·
  <a href="docs/api-reference_cn.md">API 参考</a> ·
  <a href="docs/configuration_cn.md">配置指南</a> ·
  <a href="docs/contributing_cn.md">贡献指南</a>
</p>

<p align="center">
  <img alt="License" src="https://img.shields.io/badge/license-MIT-blue.svg">
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.80%2B-orange.svg">
  <img alt="LOC" src="https://img.shields.io/badge/代码行数-160K%2B-brightgreen.svg">
</p>

---

## 为什么选择 Canal Engine？

大多数 AI 框架要么是某个 LLM 的薄封装，要么是从认证到计费什么都包的重型平台。Canal Engine 在两者之间 — 它是你嵌入自己技术栈的 **AI 能力层**。

- **多 Provider 原生支持** — 在 Anthropic、OpenAI、Google、Qwen、OpenRouter、Ollama 之间路由，支持级联降级、A/B 测试、成本优化策略。无厂商锁定。
- **真正的 Agent 执行** — 不是简单的 "循环调用 LLM"。Canal 的 Agent 具备意图识别、任务规划、工具权限、会话检查点和流式响应 — 72K 行生产级逻辑。
- **MCP 原生** — 完整的 Model Context Protocol 客户端和服务端。连接任意 MCP 工具（文件系统、浏览器、视频编辑），也可将自身工具暴露为 MCP。
- **多智能体编排** — Swarm（并行移交）、Expert（层级分派）和基于图的执行引擎（LangGraph 风格）。不是 stub — 8.7K 行真实协作逻辑。
- **Rust 性能** — 基于 Tokio 的异步优先架构。每节点开销 < 100ms，冷启动 < 3 秒。

### 适合谁？

- **AI 平台开发者** — 用 Canal 作为 AI 产品的编排后端，自带认证、计费和前端。
- **Agent 开发者** — 构建复杂的多步骤 Agent，支持工具调用、代码执行和多智能体协作。
- **LLM 基础设施团队** — 跨 Provider 路由、成本追踪、预算管控、Langfuse 风格的可观测性。

---

## 快速开始

### 作为 Rust 库使用

```bash
git clone https://github.com/Aurumbach/canal-engine.git
cd canal-engine

# 构建核心引擎
cargo build -p gateway-core

# 带全部功能构建
cargo build -p gateway-core --features "graph,collaboration,cache,learning,jobs"
```

### 在你的项目中使用

在 `Cargo.toml` 中添加：

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
    // 用你的 Provider 配置创建 LLM 路由器
    let router = LlmRouter::from_config(&config).await?;

    // 路由一个聊天请求 — Canal 自动选择最佳 Provider
    let request = ChatRequest {
        messages: vec![Message::user("用一段话解释量子计算")],
        ..Default::default()
    };
    let response = router.chat(request).await?;
    println!("{}", response.content);
    Ok(())
}
```

完整设置指南（含数据库、向量库、Docker）见 [快速开始](docs/getting-started_cn.md)。

---

## 核心功能

### LLM 路由

6 个 Provider、7 种路由策略：

| Provider | 模型 | 能力 |
|----------|------|------|
| **Anthropic** | Claude 4.x (Opus, Sonnet, Haiku) | 流式、视觉、工具调用、扩展思考 |
| **OpenAI** | GPT-4o, GPT-4, GPT-3.5 | 流式、视觉、函数调用 |
| **Google** | Gemini 3 Pro / Flash | 流式、多模态 |
| **Qwen（通义千问）** | Qwen 3 Max, QwQ, Qwen VL | 流式、视觉、推理、工具调用 |
| **OpenRouter** | UI-TARS + 100 余模型 | 通过 OpenRouter 访问任意模型 |
| **Ollama** | 任意本地模型 | 本地推理，无需 API Key |

**路由策略：**

| 策略 | 工作方式 |
|------|---------|
| 主备降级 | 先用 Provider A，失败后切换到 B |
| 级联 | 从便宜的模型开始，质量不够就升级 |
| A/B 测试 | 70/30 分流对比 |
| 任务类型规则 | 代码任务 → Anthropic，翻译 → Qwen |
| 多模态 | 文本 → Qwen，图像 → Claude，混合 → Claude |
| AI 自动选模 | 用轻量模型为每个请求选择最佳模型 |
| 轮询 | 在 Provider 间均匀分配 |

**内置成本控制：** 每日预算（默认 $100）、逐模型追踪、80% 告警阈值、不健康 Provider 自动熔断。

详见 [LLM Provider](docs/llm-providers_cn.md)。

---

### Agent 执行

Agent 循环支持自主工具使用 + 人在回路权限控制：

```
用户请求
  → 意图识别（关键词启发 + LLM 分类）
  → 任务规划（单步或多步分解）
  → 工具选择 + 权限检查
  → 执行（MCP 工具、代码、文件系统、浏览器、Git）
  → 结果观察
  → 继续或完成
  → 会话检查点
```

**内置工具：**

| 类别 | 工具 |
|------|------|
| 文件操作 | `Read`、`Write`、`Edit`、`Glob`、`Grep` |
| Shell | `Bash`（支持后台 Shell） |
| 代码 | `CodeAct` 引擎（有状态多步会话） |
| 浏览器 | `Navigate`、`Click`、`Type`、`Screenshot`、`Snapshot` |
| 视觉 | `TakeScreenshot`、`FindElement`、`OcrText`、`MouseClick` |
| Git | `Clone`、`Status`、`Diff`、`Commit`、`Push`、`Pull`、`Branch` |
| 搜索 | `WebSearch`、`Research` |

**权限模式：** `AllowAll`（无需确认）、`RequireConfirmation`（危险工具需审批）、`Restricted`（仅白名单）。

详见 [Agent 工具](docs/agent-tools_cn.md)。

---

### 多智能体协作

三种编排模式，全部完整实现（8.7K 行）：

#### Direct 模式
单 Agent 线性推理，适合简单任务。

#### Swarm 模式
多 Agent 并行移交，灵感来自 [OpenAI Swarm](https://github.com/openai/swarm)。

```
Agent A（架构师）
  → 带上下文移交 → Agent B（开发者）
  → 带上下文移交 → Agent C（审查者）
  → 完成
```

#### Expert 模式
主管分派给专家池，然后评估结果：

```
主管
  → 分派 → 专家 1（安全专家）
  → 分派 → 专家 2（代码审查）
  → 质量门评估
  → 综合最终结果
```

#### 图执行引擎
LangGraph 风格的状态图，支持节点、边、条件路由、并行执行、DAG 调度、检查点。

详见 [编排](docs/orchestration_cn.md)。

---

### MCP 网关

完整的 [Model Context Protocol](https://modelcontextprotocol.io/) 支持 — 客户端 + 服务端。

| 预配置服务器 | 命名空间 | 传输 |
|-------------|---------|------|
| 文件系统 | `fs` | stdio |
| 浏览器（Chrome 扩展） | `browser` | SSE |
| macOS（AppleScript） | `mac` | stdio |
| Windows（GUI） | `win` | stdio |
| 视频 CLI | `videocli` | stdio |
| DaVinci Resolve | `davinci` | stdio |

**工具聚合：** Agent 看到来自三个来源（内置工具 + MCP 工具 + 插件工具）的统一工具列表，通过命名空间隔离。

详见 [MCP 集成](docs/mcp-integration_cn.md)。

---

### 计算机视觉

通过 OmniParser（本地 ONNX）或 UI-TARS（云端 OpenRouter）实现基于屏幕的自动化：

- **元素检测** — 识别 UI 元素及其边界框
- **操作流水线** — 观察 → 检测 → 执行循环
- **操作类型** — 点击、双击、右击、输入、滚动、拖拽、按键
- **工作流录制** — 录制用户操作并回放
- **模板** — 将录制泛化为参数化的可复用工作流
- **屏幕监控** — 实时变化检测用于验证

详见 [计算机视觉](docs/computer-vision_cn.md)。

---

### 语义记忆

基于向量的缓存和长期记忆：

- **语义缓存** — Qdrant 向量相似度搜索，避免重复 LLM 调用（0.92 相似度阈值，1 小时 TTL）
- **计划缓存** — 重用执行计划（< 1ms 查找）
- **统一记忆存储** — 类型化条目（事实、学习、偏好、反思），置信度评分 + 时间衰减

---

### 闭环学习

启用 `learning` feature 后，Canal 从执行结果中学习：

1. **经验收集** — 记录工具序列、成功/失败、耗时
2. **模式挖掘** — 发现重复的成功模式和常见失败模式
3. **知识蒸馏** — 将模式转化为可复用技能
4. **置信度衰减** — 知识随时间老化

---

### 插件系统

连接器包将 MCP 服务器 + 技能 + 提示词打包为领域工作流：

客户支持 · 企业搜索 · 生物研究 · 产品管理 · 金融 · 法律 · 销售 · 营销 · 生产力

文档处理插件：PDF、DOCX、XLSX、PPTX。

---

## Feature Flag

| Feature | 启用功能 | 代码量 |
|---------|---------|-------|
| `graph` | StateGraph 执行引擎 | 8.4K 行 |
| `collaboration` | Swarm、Expert、Plan-Execute | 8.7K 行 |
| `orchestration` | `graph` + `collaboration` | 17.1K 行 |
| `cache` | 语义缓存 + 计划缓存 | 3.5K 行 |
| `learning` | 闭环学习系统 | 3.2K 行 |
| `jobs` | 异步任务队列 + HITL | 3K 行 |
| `multimodal` | 内容类型检测、视觉路由 | — |
| `prompt-constraints` | 安全约束配置 | — |
| `context-engineering` | 上下文优化、相关性评分 | — |
| `devtools` | LLM 可观测性 | — |
| `database` | PostgreSQL 持久化 | — |

```bash
# 默认构建（LLM 路由 + 对话 + MCP + Agent + 工具）
cargo build -p gateway-core

# 完整编排
cargo build -p gateway-core --features "graph,collaboration,cache,learning,jobs,prompt-constraints,context-engineering,devtools"
```

---

## 性能指标

| 指标 | 目标 |
|------|------|
| 冷启动 | < 3 秒 |
| 图编译 | < 500ms |
| 节点执行开销 | < 100ms |
| 语义缓存查找 | < 200ms |
| 计划缓存查找 | < 1ms |
| 模式选择 | < 50ms |

| 资源 | 限制 |
|------|------|
| 并发图节点 | 10 |
| 最大图深度 | 5 层 |
| Swarm 最大移交 | 10 |
| Expert 最大分派 | 5 |
| 缓存计划（LRU） | 1,000 |
| 学习经验/用户 | 10,000 |
| 语义缓存 TTL | 1 小时 |

---

## 平台集成

Canal Engine 是 **AI 能力层**。它做 LLM 路由、Agent 执行和编排，**不做**认证、计费和基础设施 — 这些由你提供。

### 可独立编译

| Crate | 功能 | 代码量 |
|-------|------|-------|
| `gateway-core` | Agent 循环、对话、MCP、工作流、图、协作、学习 | 72K 行 |
| `gateway-llm` | LLM 路由 — Anthropic、OpenAI、Google、Qwen、OpenRouter、Ollama | 9.7K 行 |
| `gateway-tools` | 代码执行 — Python、Bash、Node.js、Go、Rust + Docker 沙箱 | 16.5K 行 |
| `gateway-memory` | 语义缓存（Qdrant）、计划缓存、统一记忆 | 3.5K 行 |
| `gateway-plugins` | 插件和连接器包管理 | — |
| `canal-cv` | 计算机视觉 — OmniParser、UI-TARS、屏幕自动化 | — |
| `devtools-core` | LLM 可观测性 — Langfuse 风格追踪 | — |

### 需要外部 crate

`gateway-api`（HTTP 服务）和 `gateway-mcp-server` 源码包含在内，但需要：

- `canal-auth` — JWT 认证、Supabase 集成
- `canal-identity` — API Key 管理、Agent 身份

两种集成路径：

1. **实现 `gateway-service-traits` 中的 trait**，自带认证
2. **将核心 crate 作为库**引入你自己的 Axum/Actix 服务

---

## 配置

```
.env.example              # API Key、数据库、JWT 密钥
config/
├── gateway.yaml          # 服务器、CORS、数据库、限流
├── llm-providers.yaml    # Provider 配置、健康检查、成本控制
├── model-profiles.yaml   # 路由配置（7 个预设）
├── mcp-servers.yaml      # MCP 连接 + 工具权限
├── memory.yaml           # 嵌入模型、召回、提取
├── plugins.yaml          # 插件目录
├── constraints/          # 提示词安全配置
└── workflows/            # 工作流模板
```

详见 [配置指南](docs/configuration_cn.md)。

---

## 文档

| 文档 | 描述 |
|------|------|
| [快速开始](docs/getting-started_cn.md) | 安装、配置、首次构建 |
| [架构设计](docs/architecture_cn.md) | Crate 依赖图、数据流、安全模型 |
| [API 参考](docs/api-reference_cn.md) | 所有 REST 端点 + gRPC 服务 |
| [配置指南](docs/configuration_cn.md) | 环境变量 + YAML 参考 |
| [LLM Provider](docs/llm-providers_cn.md) | Provider 配置、路由策略、成本控制 |
| [MCP 集成](docs/mcp-integration_cn.md) | 客户端/服务端、工具权限 |
| [编排](docs/orchestration_cn.md) | Swarm、Expert、Graph、学习、异步任务 |
| [计算机视觉](docs/computer-vision_cn.md) | OmniParser、UI-TARS、工作流录制 |
| [Agent 工具](docs/agent-tools_cn.md) | 内置工具、权限、自定义工具 |
| [贡献指南](docs/contributing_cn.md) | 开发流程、代码风格、PR 流程 |

所有文档均有 [English](README.md) 版本。

---

## 参与贡献

欢迎贡献。查看 [贡献指南](docs/contributing_cn.md)。

```bash
cargo check --all          # 检查编译
cargo test --workspace     # 运行测试
cargo clippy --all         # 代码检查
cargo fmt --all            # 格式化
```

提交格式：`type(scope): description`（如 `feat(llm): add DeepSeek provider`）。

---

## 许可证

[MIT](LICENSE)
