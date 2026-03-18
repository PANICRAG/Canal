# 快速开始

本指南帮助你从零开始搭建 Canal Engine。

## 前置要求

| 要求 | 版本 | 必需 |
|------|------|------|
| Rust | 1.80+ | 是 |
| PostgreSQL | 14+ | 是 |
| Docker | 20+ | 否（沙箱代码执行用） |
| Qdrant | 1.7+ | 否（语义记忆用） |
| Redis | 7+ | 否（缓存用） |

## 第 1 步：安装 Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update
```

验证：

```bash
rustc --version  # 应为 1.80+
cargo --version
```

## 第 2 步：克隆与配置

```bash
git clone https://github.com/Aurumbach/canal-engine.git
cd canal-engine
```

创建环境配置文件：

```bash
cp .env.example .env
```

编辑 `.env`，至少需要：

```bash
# 必需：至少一个 LLM Provider
ANTHROPIC_API_KEY=sk-ant-your-key
# 或
OPENAI_API_KEY=sk-your-key
# 或
QWEN_API_KEY=your-qwen-key

# 必需：PostgreSQL 数据库
DATABASE_URL=postgresql://user:password@localhost:5432/canal

# 必需：JWT 认证
JWT_SECRET=your-256-bit-secret  # 生成方式: openssl rand -hex 32
API_KEY_SALT=your-random-salt
```

## 第 3 步：数据库配置

创建数据库并运行迁移：

```bash
# 创建数据库
createdb canal

# 首次启动时自动运行迁移
# 或手动运行：
cargo install sqlx-cli
sqlx migrate run
```

## 第 4 步：编译

### 最小构建

基础 LLM 路由、对话、MCP 和工具执行：

```bash
cargo build --release -p gateway-api
```

### 完整构建

包含图执行、多智能体协作、学习系统和可观测性：

```bash
cargo build --release -p gateway-api --features full-orchestration
```

### Feature 组合

```bash
# 仅图执行 + 协作
cargo build --release -p gateway-api --features orchestration

# 图 + 缓存 + 学习
cargo build --release -p gateway-api --features "graph,cache,learning"

# 带可观测性
cargo build --release -p gateway-api --features "orchestration,devtools"
```

## 第 5 步：运行

```bash
cargo run --release -p gateway-api --features full-orchestration
```

你应该会看到：

```
INFO  gateway_api > Canal Engine starting on 0.0.0.0:4000
INFO  gateway_api > LLM providers loaded: anthropic, qwen
INFO  gateway_api > MCP servers connected: 3
INFO  gateway_api > Ready to serve requests
```

## 第 6 步：验证

### 健康检查

```bash
curl http://localhost:4000/api/health
```

### 聊天请求

```bash
curl -X POST http://localhost:4000/api/chat \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -d '{
    "message": "你好！你能做什么？",
    "stream": false
  }'
```

### 流式聊天

```bash
curl -N -X POST http://localhost:4000/api/chat/stream \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -d '{
    "message": "写一个 Python 排序函数",
    "stream": true
  }'
```

### 列出可用工具

```bash
curl http://localhost:4000/api/tools \
  -H "Authorization: Bearer YOUR_JWT_TOKEN"
```

### 执行代码

```bash
curl -X POST http://localhost:4000/api/code/execute \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -d '{
    "language": "python",
    "code": "print(sum(range(100)))"
  }'
```

## 可选：Docker 代码执行

安装 Docker 以启用沙箱代码执行：

```bash
# macOS
brew install --cask docker

# Linux
curl -fsSL https://get.docker.com | sh
```

Docker 可用时，Canal Engine 会自动使用 Docker 执行 Python 和 Bash 代码。

## 可选：Qdrant 语义记忆

启用基于向量的语义缓存和记忆：

```bash
docker run -p 6333:6333 qdrant/qdrant
```

在 `.env` 中添加：

```bash
QDRANT_URL=http://localhost:6333
```

## 可选：Redis 缓存

```bash
docker run -p 6379:6379 redis
```

在 `.env` 中添加：

```bash
REDIS_URL=redis://localhost:6379
```

## 开发模式

开发模式带热重载：

```bash
# 安装 cargo-watch
cargo install cargo-watch

# 自动重载运行
cargo watch -x "run -p gateway-api --features full-orchestration"
```

启用调试面板：

```bash
# 在 .env 中
DEV_MODE=true
```

这会启用 `/api/debug/*` 端点，用于检查图执行、缓存统计等。

## 故障排查

### 端口占用

```bash
lsof -i :4000
kill -9 <PID>
```

### Feature Flag 切换后编译错误

```bash
cargo clean -p gateway-api -p gateway-core
cargo build -p gateway-api --features full-orchestration
```

### 数据库连接问题

检查 `DATABASE_URL` 格式：

```
postgresql://user:password@host:5432/database_name
```

### LLM Provider 错误

验证 API Key 是否有效：

```bash
# 测试 Anthropic
curl https://api.anthropic.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{"model":"claude-sonnet-4-6","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}'
```

## 下一步

- [架构设计](architecture_cn.md) — 了解 Canal Engine 的设计
- [配置指南](configuration_cn.md) — 完整配置参考
- [API 参考](api-reference_cn.md) — 完整端点文档
- [LLM Provider](llm-providers_cn.md) — Provider 配置与路由策略
- [MCP 集成](mcp-integration_cn.md) — 连接 MCP 服务器
