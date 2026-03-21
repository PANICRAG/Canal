# 配置指南

Canal Engine 通过环境变量（`.env`）和 YAML 文件（`config/`）进行配置。

## 环境变量

### 必需

| 变量 | 描述 | 示例 |
|------|------|------|
| `DATABASE_URL` | PostgreSQL 连接字符串 | `postgresql://user:pass@localhost:5432/canal` |
| `JWT_SECRET` | 256 位 JWT 签名密钥 | 用 `openssl rand -hex 32` 生成 |
| `API_KEY_SALT` | API Key 哈希盐值 | 随机字符串 |

加上至少一个 LLM Provider Key：

| 变量 | Provider |
|------|----------|
| `ANTHROPIC_API_KEY` | Anthropic Claude |
| `OPENAI_API_KEY` | OpenAI GPT |
| `GOOGLE_API_KEY` | Google Gemini |
| `QWEN_API_KEY` | Qwen / 通义千问 |
| `OPENROUTER_API_KEY` | OpenRouter |

### 可选

| 变量 | 描述 | 默认值 |
|------|------|--------|
| `PORT` | 服务端口 | `4000` |
| `HOST` | 绑定地址 | `0.0.0.0` |
| `ENVIRONMENT` | `development` / `staging` / `production` | `development` |
| `RUST_LOG` | 日志级别 | `info` |
| `DEV_MODE` | 启用调试端点 | `false` |
| `QDRANT_URL` | Qdrant 向量数据库 URL | — |
| `QDRANT_API_KEY` | Qdrant API Key | — |
| `REDIS_URL` | Redis URL | — |
| `SENTRY_DSN` | Sentry 错误追踪 | — |
| `SLACK_WEBHOOK_URL` | Slack 告警 | — |
| `OMNIPARSER_MODEL_DIR` | ONNX 模型目录 | — |
| `QWEN_BASE_URL` | 自定义 Qwen 地址 | `https://dashscope.aliyuncs.com/compatible-mode` |
| `QWEN_DEFAULT_MODEL` | 默认 Qwen 模型 | `qwen3-max-2026-01-23` |

## YAML 配置

### config/gateway.yaml

服务器基础配置：

```yaml
server:
  port: 4000
  host: "0.0.0.0"
  timeout_secs: 120
  max_body_size: "10MB"

cors:
  allowed_origins:
    - "http://localhost:3000"
    - "http://localhost:5173"
  allow_credentials: true

database:
  max_connections: 10
  connect_timeout_secs: 30
  idle_timeout_secs: 600

vector_db:
  collection: "gateway_embeddings"
  dimension: 1536
  distance: "cosine"

llm:
  default_provider: "qwen"
  cache_enabled: true
  cache_ttl_secs: 3600

logging:
  level: "info"
  format: "json"      # "json" 或 "pretty"

rate_limit:
  requests_per_minute: 60
  burst: 10

health:
  check_interval_secs: 30
```

环境覆盖：`config/gateway.development.yaml`、`config/gateway.staging.yaml`、`config/gateway.production.yaml`。

### config/llm-providers.yaml

LLM Provider 配置：

```yaml
routing:
  strategy: "round_robin"

health_check:
  interval_secs: 30
  timeout_secs: 10
  failure_threshold: 3

retry:
  max_attempts: 3
  backoff:
    initial_ms: 100
    max_ms: 5000
    multiplier: 2.0

cost_control:
  daily_budget_usd: 100.0
  alert_threshold: 0.8
  hard_limit: false

providers:
  qwen:
    enabled: true
    priority: 0
    base_url: "${QWEN_BASE_URL}"
    api_key: "${QWEN_API_KEY}"
    models:
      - name: "qwen-turbo"
        max_tokens: 8192
      - name: "qwen3-max-2026-01-23"
        max_tokens: 32768
        capabilities: [vision, function_calling, thinking]

  anthropic:
    enabled: true
    priority: 1
    api_key: "${ANTHROPIC_API_KEY}"
    models:
      - name: "claude-sonnet-4-6"
        max_tokens: 8192
        capabilities: [vision, function_calling, thinking]
```

### config/model-profiles.yaml

不同场景的路由配置：

```yaml
profiles:
  default:
    strategy: "primary_fallback"
    primary:
      provider: "qwen"
      model: "qwen3-max-2026-01-23"
    fallbacks:
      - provider: "anthropic"
        model: "claude-sonnet-4-6"

  code-optimized:
    strategy: "task_type_rules"
    rules:
      - task_pattern: "code|debug|refactor"
        provider: "anthropic"
        model: "claude-sonnet-4-6"
      - task_pattern: "*"
        provider: "qwen"
        model: "qwen3-max-2026-01-23"

  cost-cascade:
    strategy: "cascade"
    tiers:
      - provider: "qwen"
        model: "qwen-turbo"
        quality_threshold: 0.7
      - provider: "qwen"
        model: "qwen3-max-2026-01-23"
        quality_threshold: 0.85
      - provider: "anthropic"
        model: "claude-sonnet-4-6"
```

### config/mcp-servers.yaml

MCP 服务器连接：

```yaml
connection:
  timeout_secs: 30
  max_retries: 3
  max_connections_per_server: 5

servers:
  - name: "filesystem"
    namespace: "fs"
    transport: "stdio"
    command: "npx"
    args: ["-y", "@anthropic/mcp-filesystem"]
    enabled: true

  - name: "browser"
    namespace: "browser"
    transport: "sse"
    url: "http://localhost:3100/sse"
    enabled: true

tool_permissions:
  require_confirmation:
    - "fs.delete"
    - "fs.write_file"
  blocked:
    - "fs.execute"
  rate_limits:
    fs: 100        # 每分钟
    browser: 60
```

### config/memory.yaml

记忆与缓存：

```yaml
enabled: true

embedding:
  model: "text-embedding-3-small"
  dimension: 1536

extraction:
  min_messages: 4
  max_prompt_chars: 6000
  model: "qwen-turbo"

recall:
  max_memories: 5
  min_similarity: 0.3
```

### config/constraints/

提示词约束配置：

```yaml
# config/constraints/default.yaml
name: "default"

security:
  blocked_patterns:
    - "*.env"
    - "*.pem"
    - "*.key"
  blocked_commands:
    - "rm -rf"
    - "sudo"
    - "mkfs"
  require_confirmation:
    - "git push --force"
    - "git reset --hard"

token_limits:
  system_prompt: 8000
  response: 4000
```

## 配置优先级

1. 环境变量（最高优先级）
2. 环境特定 YAML（`gateway.production.yaml`）
3. 基础 YAML（`gateway.yaml`）
4. 代码默认值（最低优先级）
