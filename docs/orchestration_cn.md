# 编排

Canal Engine 通过 feature 门控模块提供高级多智能体编排。启用方式：`--features orchestration`（或 `full-orchestration`）。

## 协作模式

### Direct 模式

单 Agent 线性推理，适合简单任务。

```bash
curl -X POST http://localhost:4000/api/graph/execute/direct \
  -H "Authorization: Bearer TOKEN" \
  -d '{"task": "解释 Rust 中 async/await 的工作原理"}'
```

### Swarm 模式

多个 Agent 并行工作并进行移交。灵感来自 OpenAI Swarm。

每个 Agent 专注一个领域。当一个 Agent 判断另一个更适合当前子任务时，会带着上下文移交。

```bash
curl -X POST http://localhost:4000/api/graph/execute/swarm \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "task": "构建一个带认证的博客",
    "agents": ["architect", "backend-dev", "frontend-dev", "reviewer"]
  }'
```

核心概念：
- **移交规则** — 定义 Agent A 何时移交给 Agent B
- **上下文传递** — 共享状态在 Agent 间传递
- **最大移交次数** — 可配置限制（默认：10），防止循环

### Expert 模式

主管 Agent 分派任务给专家池，然后通过质量门评估结果。

```bash
curl -X POST http://localhost:4000/api/graph/execute/expert \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "task": "分析代码库的安全漏洞",
    "specialists": ["security-expert", "code-reviewer", "dependency-auditor"]
  }'
```

核心概念：
- **主管** — 规划调用哪些专家
- **专家池** — 拥有特定工具/提示词的领域专家
- **质量门** — 阈值或组合验证
- **最大分派次数** — 可配置限制（默认：5）

### Auto 模式

让 Canal 根据任务分析选择最佳模式。

```bash
curl -X POST http://localhost:4000/api/graph/execute/auto \
  -H "Authorization: Bearer TOKEN" \
  -d '{"task": "你的任务描述"}'
```

任务分类器会考虑复杂度、领域广度和风险级别来选择 Direct、Swarm 或 Expert。

## 图执行引擎

图引擎（feature: `graph`）提供 LangGraph 风格的状态图执行。

### 核心概念

- **StateGraph** — 定义节点和边
- **节点** — 一个计算步骤（LLM 调用、工具执行、自定义函数）
- **边** — 节点间的转移（无条件或有条件）
- **并行节点** — 并发执行多个节点
- **检查点** — 保存/恢复执行状态

### 构建图

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

### DAG 调度

图引擎自动并行化独立节点：

```
   analyze
   /     \
plan_a  plan_b    ← 并行执行
   \     /
   merge
     |
   execute
```

### 检查点

保存执行状态用于恢复：

```rust
// 基于内存（快速，重启后丢失）
let checkpointer = MemoryCheckpointer::new();

// 基于文件（持久化）
let checkpointer = FileCheckpointer::new("/tmp/checkpoints");
```

### 预算控制

节点级和图级预算限制：

| 资源 | 默认限制 |
|------|---------|
| 最大并发节点 | 10 |
| 最大图深度 | 5 层 |
| 图编译时间 | < 500ms |
| 节点执行开销 | < 100ms |

## 闭环学习

启用 `learning` feature 后，Canal 从执行结果中学习：

1. **经验收集** — 记录成功和失败
2. **模式挖掘** — 发现重复的成功/失败模式
3. **知识蒸馏** — 将模式转化为可复用技能
4. **置信度衰减** — 知识随时间老化并最终遗忘

```bash
# 查看学习状态
curl http://localhost:4000/api/learning/status \
  -H "Authorization: Bearer TOKEN"

# 查询已学知识
curl http://localhost:4000/api/learning/knowledge \
  -H "Authorization: Bearer TOKEN"
```

资源限制：
- 最大学习经验：每用户 10,000 条
- 置信度衰减：可配置速率

## 异步任务

启用 `jobs` feature 后，长时间任务异步运行：

```bash
# 提交任务
curl -X POST http://localhost:4000/api/jobs \
  -H "Authorization: Bearer TOKEN" \
  -d '{
    "task": "生成一份综合分析...",
    "webhook_url": "https://your-server.com/webhook"
  }'

# 流式进度
curl -N http://localhost:4000/api/jobs/JOB_ID/stream \
  -H "Authorization: Bearer TOKEN"

# 人在回路输入
curl -X POST http://localhost:4000/api/jobs/JOB_ID/input \
  -H "Authorization: Bearer TOKEN" \
  -d '{"input": "同意，选择方案 B"}'
```
