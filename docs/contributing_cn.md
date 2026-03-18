# 贡献指南

感谢你有兴趣为 Canal Engine 做出贡献！

## 开发环境搭建

### 前置要求

- Rust 1.80+（`rustup update`）
- PostgreSQL 14+
- Docker（代码执行测试需要）

### 克隆与构建

```bash
git clone https://github.com/Aurumbach/canal-engine.git
cd canal-engine
cp .env.example .env
# 编辑 .env 配置

# 完整构建
cargo build -p gateway-api --features full-orchestration

# 运行测试
cargo test --workspace
```

### 开发工作流

```bash
# 修改后检查编译
cargo check --all

# Clippy 代码检查
cargo clippy --all

# 格式化代码
cargo fmt --all

# 热重载开发
cargo install cargo-watch
cargo watch -x "run -p gateway-api --features full-orchestration"
```

## 代码风格

- **语言**：所有源代码注释使用英文
- **格式化**：`cargo fmt`（配置在 `rustfmt.toml`）
- **代码检查**：`cargo clippy`（配置在 `clippy.toml`）
- **错误处理**：库错误用 `thiserror`，应用错误用 `anyhow`
- **异步**：使用 `tokio` 运行时，`async-trait` 用于 trait 方法

## 架构指南

### Crate 边界

- `gateway-core` — 仅 AI 逻辑。不含 HTTP，不含基础设施。
- `gateway-api` — HTTP 绑定。不含业务逻辑。
- `gateway-tools` — 执行实现。不含路由逻辑。
- `gateway-memory` — 仅存储。不含 AI 决策。
- `canal-cv` — 仅视觉。不含 Agent 逻辑。
- `devtools-core` — 零 gateway-core 依赖。

### Feature 门控

所有新功能必须在 feature flag 后面：

```toml
[features]
my-feature = []

# 代码中：
#[cfg(feature = "my-feature")]
mod my_feature;
```

默认编译路径中不能包含 feature 门控代码。

### 基于 Trait 的设计

每个新子系统都应该定义 trait：

```rust
#[async_trait]
pub trait MyService: Send + Sync {
    async fn do_something(&self, input: Input) -> Result<Output>;
}
```

这便于使用 mock 进行测试和替换实现。

## Pull Request 流程

1. **Fork** 仓库
2. **创建分支**：`git checkout -b feat/my-feature`
3. **按照指南修改代码**
4. **测试**：`cargo test --workspace`
5. **代码检查**：`cargo clippy --all`
6. **格式化**：`cargo fmt --all`
7. **提交**，使用约定格式：`feat(scope): description`
8. **提交 PR** 到 `main` 分支

### 提交信息格式

```
type(scope): description

类型：feat, fix, refactor, test, docs, chore
范围：core, api, llm, tools, memory, cv, mcp 等
```

示例：

```
feat(llm): add DeepSeek provider
fix(mcp): handle reconnection on transport failure
refactor(core): extract tool registry into separate module
test(graph): add parallel execution tests
docs: update API reference for memory endpoints
```

### PR 检查清单

- [ ] `cargo check --all` 通过
- [ ] `cargo test --workspace` 通过
- [ ] `cargo clippy --all` 无警告
- [ ] `cargo fmt --all` 已应用
- [ ] 新功能在 feature flag 后面
- [ ] 新公开 API 有文档
- [ ] 破坏性变更在 PR 描述中注明

## 添加新 LLM Provider

1. 创建 `crates/gateway-llm/src/providers/my_provider.rs`
2. 实现 `LlmProvider` trait
3. 在 provider 工厂中注册
4. 在 `config/llm-providers.yaml` 中添加配置
5. 添加测试

## 添加新工具

1. 在 `crates/gateway-core/src/agent/tools/my_tool.rs` 创建工具
2. 实现 `AgentTool` 或 `DynamicTool` trait
3. 在工具注册表中注册
4. 添加测试

## 添加新 MCP 服务器

1. 在 `config/mcp-servers.yaml` 中添加服务器定义
2. 测试连接和工具发现
3. 按需添加权限配置

## 测试

```bash
# 所有测试
cargo test --workspace

# 指定 crate
cargo test -p gateway-core

# 指定测试
cargo test -p gateway-core -- test_name

# 带 feature
cargo test --workspace --features full-orchestration
```

## 有问题？

在 GitHub 上提 issue，用于提问、报告 bug 或请求功能。
