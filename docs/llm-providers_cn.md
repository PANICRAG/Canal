# LLM Provider

Canal Engine 支持多个 LLM Provider，并在它们之间进行智能路由。

## 支持的 Provider

### Anthropic (Claude)

```bash
ANTHROPIC_API_KEY=sk-ant-your-key
```

模型：`claude-opus-4-6`、`claude-sonnet-4-6`、`claude-haiku-4-5-20251001`

特性：流式、视觉、工具调用、扩展思考。

### OpenAI (GPT)

```bash
OPENAI_API_KEY=sk-your-key
```

模型：`gpt-4o`、`gpt-4`、`gpt-3.5-turbo`

特性：流式、视觉、函数调用。

### Google (Gemini)

```bash
GOOGLE_API_KEY=your-key
```

模型：`gemini-3-pro`、`gemini-3-flash`

特性：流式、多模态。

### 通义千问 / DashScope

```bash
QWEN_API_KEY=your-key
QWEN_BASE_URL=https://dashscope.aliyuncs.com/compatible-mode  # 可选
```

模型：`qwen3-max-2026-01-23`、`qwen-turbo`、`qwq-plus`（推理）、`qwen3-vl-plus`（视觉）

特性：流式、视觉、函数调用、思考模式（QwQ）。

### OpenRouter

```bash
OPENROUTER_API_KEY=sk-or-v1-your-key
```

访问 100+ 模型，包括用于 GUI 自动化的 UI-TARS。

### Ollama（本地）

```bash
OLLAMA_URL=http://localhost:11434  # 可选，自动检测
```

本地运行任意模型，无需 API Key。

## 路由策略

在 `config/model-profiles.yaml` 中配置：

### 主备降级

尝试主 Provider，失败时自动降级。

```yaml
strategy: "primary_fallback"
primary:
  provider: "qwen"
  model: "qwen3-max-2026-01-23"
fallbacks:
  - provider: "anthropic"
    model: "claude-sonnet-4-6"
```

### 级联（成本优化）

从最便宜的模型开始，质量不够时逐级升级。

```yaml
strategy: "cascade"
tiers:
  - provider: "qwen"
    model: "qwen-turbo"           # 最便宜
    quality_threshold: 0.7
  - provider: "qwen"
    model: "qwen3-max-2026-01-23" # 中档
    quality_threshold: 0.85
  - provider: "anthropic"
    model: "claude-sonnet-4-6"    # 高端
```

### A/B 测试

按权重分配流量进行对比。

```yaml
strategy: "ab_test"
variants:
  - provider: "qwen"
    model: "qwen3-max-2026-01-23"
    weight: 70
  - provider: "anthropic"
    model: "claude-sonnet-4-6"
    weight: 30
```

### 任务类型规则

根据任务分类进行路由。

```yaml
strategy: "task_type_rules"
rules:
  - task_pattern: "code|debug|refactor"
    provider: "anthropic"
    model: "claude-sonnet-4-6"
  - task_pattern: "translate|summarize"
    provider: "qwen"
    model: "qwen-turbo"
  - task_pattern: "*"
    provider: "qwen"
    model: "qwen3-max-2026-01-23"
```

### 多模态内容路由

根据输入内容类型（文本、图像、混合）路由。

```yaml
strategy: "multimodal"
text_provider:
  provider: "qwen"
  model: "qwen3-max-2026-01-23"
vision_provider:
  provider: "anthropic"
  model: "claude-sonnet-4-6"
```

### AI 自动选模

AI Agent 为每个请求选择最佳模型。

```yaml
strategy: "router_agent"
router_model: "qwen-turbo"  # 快速便宜的模型做路由决策
candidates:
  - provider: "qwen"
    model: "qwen3-max-2026-01-23"
  - provider: "anthropic"
    model: "claude-sonnet-4-6"
  - provider: "google"
    model: "gemini-3-pro"
```

## 健康监控

Provider 采用熔断器模式监控：

```yaml
health_check:
  interval_secs: 30      # 每 30 秒检查
  timeout_secs: 10       # 检查超时
  failure_threshold: 3   # 连续失败次数阈值
```

Provider 失败达到阈值后自动从路由中移除，恢复后重新加入。

## 成本控制

```yaml
cost_control:
  daily_budget_usd: 100.0    # 每日预算
  alert_threshold: 0.8       # 80% 时告警
  hard_limit: false           # false = 仅告警，true = 拒绝请求
```

## 添加自定义 Provider

实现 `LlmProvider` trait：

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse>;
    async fn chat_stream(&self, request: ChatRequest) -> Result<StreamResponse>;
    fn supported_models(&self) -> Vec<ModelInfo>;
    async fn health_check(&self) -> Result<bool>;
}
```
