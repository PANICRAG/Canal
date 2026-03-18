# LLM Providers

Canal Engine supports multiple LLM providers with intelligent routing between them.

## Supported Providers

### Anthropic (Claude)

```bash
ANTHROPIC_API_KEY=sk-ant-your-key
```

Models: `claude-opus-4-6`, `claude-sonnet-4-6`, `claude-haiku-4-5-20251001`

Features: Streaming, vision, tool use, extended thinking.

### OpenAI (GPT)

```bash
OPENAI_API_KEY=sk-your-key
```

Models: `gpt-4o`, `gpt-4`, `gpt-3.5-turbo`

Features: Streaming, vision, function calling.

### Google (Gemini)

```bash
GOOGLE_API_KEY=your-key
```

Models: `gemini-3-pro`, `gemini-3-flash`

Features: Streaming, multimodal.

### Qwen / DashScope

```bash
QWEN_API_KEY=your-key
QWEN_BASE_URL=https://dashscope.aliyuncs.com/compatible-mode  # optional
```

Models: `qwen3-max-2026-01-23`, `qwen-turbo`, `qwq-plus` (reasoning), `qwen3-vl-plus` (vision)

Features: Streaming, vision, function calling, thinking mode (QwQ).

### OpenRouter

```bash
OPENROUTER_API_KEY=sk-or-v1-your-key
```

Access to 100+ models including UI-TARS for GUI automation.

### Ollama (Local)

```bash
OLLAMA_URL=http://localhost:11434  # optional, auto-detected
```

Run any model locally. No API key needed.

## Routing Strategies

Configure in `config/model-profiles.yaml`:

### Primary Fallback

Try the primary provider, fall back to alternatives on failure.

```yaml
strategy: "primary_fallback"
primary:
  provider: "qwen"
  model: "qwen3-max-2026-01-23"
fallbacks:
  - provider: "anthropic"
    model: "claude-sonnet-4-6"
```

### Cascade (Cost Optimization)

Start with the cheapest model, escalate if quality is insufficient.

```yaml
strategy: "cascade"
tiers:
  - provider: "qwen"
    model: "qwen-turbo"           # cheapest
    quality_threshold: 0.7
  - provider: "qwen"
    model: "qwen3-max-2026-01-23" # mid-tier
    quality_threshold: 0.85
  - provider: "anthropic"
    model: "claude-sonnet-4-6"    # premium
```

### A/B Test

Split traffic between providers for comparison.

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

### Task-Type Rules

Route based on task classification.

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

### Multimodal Content Routing

Route based on input content type (text, image, mixed).

```yaml
strategy: "multimodal"
text_provider:
  provider: "qwen"
  model: "qwen3-max-2026-01-23"
vision_provider:
  provider: "anthropic"
  model: "claude-sonnet-4-6"
hybrid_provider:
  provider: "anthropic"
  model: "claude-sonnet-4-6"
```

### AI-Powered Auto-Selection

An AI agent selects the best model for each request.

```yaml
strategy: "router_agent"
router_model: "qwen-turbo"  # fast, cheap model for routing decisions
candidates:
  - provider: "qwen"
    model: "qwen3-max-2026-01-23"
  - provider: "anthropic"
    model: "claude-sonnet-4-6"
  - provider: "google"
    model: "gemini-3-pro"
```

## Health Monitoring

Providers are monitored with circuit breaker pattern:

```yaml
health_check:
  interval_secs: 30      # check every 30s
  timeout_secs: 10       # timeout per check
  failure_threshold: 3   # failures before circuit opens
```

When a provider fails the threshold, it's removed from routing until it recovers.

## Cost Control

```yaml
cost_control:
  daily_budget_usd: 100.0    # daily spending limit
  alert_threshold: 0.8       # alert at 80% of budget
  hard_limit: false           # false = alert only, true = block requests
```

## Adding a Custom Provider

Implement the `LlmProvider` trait:

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
