# Agent 工具

Canal Engine 为 Agent 提供丰富的内置工具，并通过 MCP 和插件支持扩展。

## 内置工具

### 文件操作

| 工具 | 描述 |
|------|------|
| `Read` | 读取文件内容，支持行数限制 |
| `Write` | 创建或覆盖文件 |
| `Edit` | 编辑文件的指定区域 |
| `Glob` | 按模式匹配查找文件 |
| `Grep` | 用正则表达式搜索文件内容 |

### Shell 与代码

| 工具 | 描述 |
|------|------|
| `Bash` | 执行 Shell 命令（Linux/macOS） |
| `Computer` | CodeAct 引擎，有状态的多步骤代码执行 |

### 代码执行（通过 `/api/code/execute`）

| 语言 | 沙箱 | 备注 |
|------|------|------|
| Python | Docker | 完整标准库，pip 包 |
| Bash | Docker | Shell 脚本 |
| Node.js | Docker / 子进程 | 支持 TypeScript |
| Go | Docker / 子进程 | Feature 门控（`unsafe-executors`） |
| Rust | Docker / 子进程 | Feature 门控（`unsafe-executors`） |

### 浏览器自动化

| 工具 | 描述 |
|------|------|
| `Navigate` | 在浏览器中打开 URL |
| `Click` | 点击 DOM 元素 |
| `Type` | 填充文本输入 |
| `Screenshot` | 捕获页面截图 |
| `Snapshot` | 完整页面 DOM 快照 |

### 计算机视觉

| 工具 | 描述 |
|------|------|
| `TakeScreenshot` | 捕获桌面屏幕 |
| `FindElement` | 通过描述定位 UI 元素 |
| `OcrText` | 从屏幕区域提取文本 |
| `MouseClick` | 按屏幕坐标点击 |
| `KeyboardType` | 向聚焦元素输入文本 |
| `Scroll` | 滚动 |
| `WaitForElement` | 等待元素出现 |

### Git

| 工具 | 描述 |
|------|------|
| `GitClone` | 克隆仓库 |
| `GitStatus` | 获取工作树状态 |
| `GitDiff` | 显示差异 |
| `GitCommit` | 创建提交 |
| `GitBranch` | 创建/切换分支 |
| `GitPush` / `GitPull` | 与远程同步 |

### 搜索与研究

| 工具 | 描述 |
|------|------|
| `Search` | 网页搜索 |
| `WebSearch` | Google 兼容搜索 |
| `Research` | 多来源深度研究 |

### 任务管理

| 工具 | 描述 |
|------|------|
| `Task` | 为复杂工作创建子任务 |

## 工具注册

工具通过元数据注册到工具注册表：

```rust
pub struct ToolMetadata {
    pub namespace: String,     // 如 "agent", "fs", "browser"
    pub name: String,          // 如 "bash", "read_file"
    pub description: String,
    pub input_schema: Value,   // JSON Schema
    pub requires_confirmation: bool,
}
```

## 工具权限

### 权限模式

Agent 在不同权限模式下运行：

| 模式 | 行为 |
|------|------|
| `AllowAll` | 所有工具无需确认即可运行 |
| `RequireConfirmation` | 危险工具需要用户批准 |
| `Restricted` | 仅白名单工具可用 |

### 逐工具确认

部分工具始终需要确认：

- 文件写入/删除操作
- Git push/force 操作
- 包含破坏性模式的 Shell 命令
- 生产环境中的代码执行

### 基于角色的工具过滤

角色系统（`roles/`）按用户角色过滤可用工具：

```yaml
# config/constraints/coding_assistant.yaml
capabilities:
  - code
  - browser
  - file
  - shell
  - research
```

## 扩展自定义工具

### 实现 AgentTool Trait

```rust
use gateway_tool_types::{AgentTool, ToolContext, ToolResult};

pub struct MyTool;

#[async_trait]
impl AgentTool for MyTool {
    type Input = MyInput;
    type Output = MyOutput;

    fn metadata(&self) -> ToolMetadata {
        ToolMetadata {
            namespace: "custom".into(),
            name: "my_tool".into(),
            description: "做一些有用的事".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "param": {"type": "string"}
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn execute(&self, input: Self::Input, ctx: &ToolContext) -> ToolResult<Self::Output> {
        // 你的实现
    }
}
```

### 注册工具

```rust
tool_registry.register(Box::new(MyTool));
```

### 或实现 DynamicTool

类型擦除的工具，使用 JSON 值：

```rust
#[async_trait]
impl DynamicTool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn namespace(&self) -> &str { "custom" }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        // 你的实现
    }
}
```
