# Agent Tools

Canal Engine provides a rich set of built-in tools for the agent, plus extensibility through MCP and plugins.

## Built-in Tools

### File Operations

| Tool | Description |
|------|-------------|
| `Read` | Read file contents with optional line limits |
| `Write` | Create or overwrite files |
| `Edit` | Edit specific regions of a file |
| `Glob` | Find files by pattern matching |
| `Grep` | Search file contents with regex |

### Shell & Code

| Tool | Description |
|------|-------------|
| `Bash` | Execute shell commands (Linux/macOS) |
| `Computer` | CodeAct engine for stateful multi-step code execution |

### Code Execution (via `/api/code/execute`)

| Language | Sandbox | Notes |
|----------|---------|-------|
| Python | Docker | Full stdlib, pip packages |
| Bash | Docker | Shell scripts |
| Node.js | Docker / subprocess | TypeScript supported |
| Go | Docker / subprocess | Feature-gated (`unsafe-executors`) |
| Rust | Docker / subprocess | Feature-gated (`unsafe-executors`) |

### Browser Automation

| Tool | Description |
|------|-------------|
| `Navigate` | Open URLs in browser |
| `Click` | Click DOM elements |
| `Type` | Fill text inputs |
| `Screenshot` | Capture page screenshot |
| `Snapshot` | Full page DOM snapshot |

### Computer Vision

| Tool | Description |
|------|-------------|
| `TakeScreenshot` | Capture desktop screen |
| `FindElement` | Locate UI elements by description |
| `OcrText` | Extract text from screen regions |
| `MouseClick` | Click at screen coordinates |
| `KeyboardType` | Type to focused element |
| `Scroll` | Scroll in a direction |
| `WaitForElement` | Wait for element appearance |

### Git

| Tool | Description |
|------|-------------|
| `GitClone` | Clone a repository |
| `GitStatus` | Get working tree status |
| `GitDiff` | Show changes |
| `GitCommit` | Create a commit |
| `GitBranch` | Create/switch branches |
| `GitPush` / `GitPull` | Sync with remote |

### Search & Research

| Tool | Description |
|------|-------------|
| `Search` | Web search |
| `WebSearch` | Google-compatible search |
| `Research` | Deep research with multiple sources |

### Task Management

| Tool | Description |
|------|-------------|
| `Task` | Create subtasks for complex work |

## Tool Registry

Tools are registered in the tool registry with metadata:

```rust
pub struct ToolMetadata {
    pub namespace: String,     // e.g., "agent", "fs", "browser"
    pub name: String,          // e.g., "bash", "read_file"
    pub description: String,
    pub input_schema: Value,   // JSON Schema
    pub requires_confirmation: bool,
}
```

## Tool Permissions

### Permission Modes

The agent operates under different permission modes:

| Mode | Behavior |
|------|----------|
| `AllowAll` | All tools run without confirmation |
| `RequireConfirmation` | Dangerous tools require user approval |
| `Restricted` | Only whitelisted tools available |

### Per-Tool Confirmation

Some tools always require confirmation:

- File write/delete operations
- Git push/force operations
- Shell commands with destructive patterns
- Code execution in production environments

### Tool Filtering by Role

The role system (`roles/`) filters available tools per user role:

```yaml
# config/constraints/coding_assistant.yaml
capabilities:
  - code
  - browser
  - file
  - shell
  - research
```

## Extending with Custom Tools

### Implement the AgentTool Trait

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
            description: "Does something useful".into(),
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
        // Your implementation
    }
}
```

### Register the Tool

```rust
tool_registry.register(Box::new(MyTool));
```

### Or Implement DynamicTool

For type-erased tools that work with JSON values:

```rust
#[async_trait]
impl DynamicTool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn namespace(&self) -> &str { "custom" }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        // Your implementation
    }
}
```
