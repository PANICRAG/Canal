# Configuration

Canal Engine is configured through environment variables (`.env`) and YAML files (`config/`).

## Environment Variables

### Required

| Variable | Description | Example |
|----------|-------------|---------|
| `DATABASE_URL` | PostgreSQL connection string | `postgresql://user:pass@localhost:5432/canal` |
| `JWT_SECRET` | 256-bit secret for JWT signing | Generate with `openssl rand -hex 32` |
| `API_KEY_SALT` | Salt for API key hashing | Random string |

Plus at least one LLM provider key:

| Variable | Provider |
|----------|----------|
| `ANTHROPIC_API_KEY` | Anthropic Claude |
| `OPENAI_API_KEY` | OpenAI GPT |
| `GOOGLE_API_KEY` | Google Gemini |
| `QWEN_API_KEY` | Qwen / DashScope |
| `OPENROUTER_API_KEY` | OpenRouter |

### Optional

| Variable | Description | Default |
|----------|-------------|---------|
| `PORT` | Server port | `4000` |
| `HOST` | Bind address | `0.0.0.0` |
| `ENVIRONMENT` | `development` / `staging` / `production` | `development` |
| `RUST_LOG` | Log level | `info` |
| `DEV_MODE` | Enable debug endpoints | `false` |
| `QDRANT_URL` | Qdrant vector DB URL | — |
| `QDRANT_API_KEY` | Qdrant API key | — |
| `REDIS_URL` | Redis URL | — |
| `SENTRY_DSN` | Sentry error tracking | — |
| `SLACK_WEBHOOK_URL` | Slack alerts | — |
| `OMNIPARSER_MODEL_DIR` | ONNX model directory | — |
| `QWEN_BASE_URL` | Custom Qwen base URL | `https://dashscope.aliyuncs.com/compatible-mode` |
| `QWEN_DEFAULT_MODEL` | Default Qwen model | `qwen3-max-2026-01-23` |

## YAML Configuration

### config/gateway.yaml

Base configuration for the server:

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
  format: "json"      # "json" or "pretty"

rate_limit:
  requests_per_minute: 60
  burst: 10

health:
  check_interval_secs: 30
```

Environment overrides: `config/gateway.development.yaml`, `config/gateway.staging.yaml`, `config/gateway.production.yaml`.

### config/llm-providers.yaml

LLM provider configuration:

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

  google:
    enabled: true
    priority: 1
    api_key: "${GOOGLE_AI_API_KEY}"
    models:
      - name: "gemini-3-pro"
        max_tokens: 8192
```

### config/model-profiles.yaml

Routing profiles for different use cases:

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

MCP server connections:

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
    fs: 100        # per minute
    browser: 60
```

### config/memory.yaml

Memory and caching:

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

embedder:
  batch_size: 20
  poll_interval_secs: 10
```

### config/constraints/

Prompt constraint profiles:

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

## Configuration Precedence

1. Environment variables (highest priority)
2. Environment-specific YAML (`gateway.production.yaml`)
3. Base YAML (`gateway.yaml`)
4. Code defaults (lowest priority)
