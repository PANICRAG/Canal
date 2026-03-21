# MCP 集成

Canal Engine 完整支持 [Model Context Protocol](https://modelcontextprotocol.io/)，同时作为客户端（连接 MCP 服务器）和服务端（通过 MCP 暴露工具）。

## 作为 MCP 客户端

### 预配置的服务器

Canal 在 `config/mcp-servers.yaml` 中预配置了多个 MCP 服务器：

| 服务器 | 命名空间 | 传输 | 描述 |
|--------|---------|------|------|
| filesystem | `fs` | stdio | 文件读写删除搜索 |
| browser | `browser` | SSE | Chrome 扩展浏览器自动化 |
| macOS | `mac` | stdio | AppleScript 自动化 |
| windows | `win` | stdio | Windows GUI 自动化 |
| videocli | `videocli` | stdio | 视频创建 |
| davinci | `davinci` | stdio | DaVinci Resolve 编辑 |

### 添加 MCP 服务器

#### 通过配置文件

在 `config/mcp-servers.yaml` 中添加：

```yaml
servers:
  - name: "my-server"
    namespace: "custom"
    transport: "stdio"
    command: "npx"
    args: ["-y", "my-mcp-package"]
    enabled: true
    env:
      MY_VAR: "value"
```

#### 通过 API（运行时）

```bash
curl -X POST http://localhost:4000/api/mcp/servers \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-server",
    "namespace": "custom",
    "transport": "stdio",
    "command": "npx",
    "args": ["-y", "my-mcp-package"],
    "env": {"MY_VAR": "value"}
  }'
```

### 传输类型

| 传输 | 使用场景 | 配置 |
|------|---------|------|
| `stdio` | 本地进程 | `command` + `args` |
| `sse` | 远程 HTTP 服务器 | `url` |

### 工具权限

控制哪些 MCP 工具需要确认才能执行：

```yaml
tool_permissions:
  # 执行前需要用户确认的工具
  require_confirmation:
    - "fs.delete"
    - "fs.write_file"
    - "davinci.render"

  # 完全封锁的工具
  blocked:
    - "fs.execute"

  # 每命名空间频率限制（请求/分钟）
  rate_limits:
    fs: 100
    videocli: 30
    davinci: 10
    browser: 60
```

### 列出工具

```bash
# 所有来源的所有工具
curl http://localhost:4000/api/tools -H "Authorization: Bearer TOKEN"

# 指定 MCP 服务器的工具
curl http://localhost:4000/api/mcp/servers/filesystem/tools \
  -H "Authorization: Bearer TOKEN"
```

### 调用 MCP 工具

```bash
curl -X POST http://localhost:4000/api/tools/fs/read_file/call \
  -H "Authorization: Bearer TOKEN" \
  -d '{"path": "/tmp/test.txt"}'
```

## 作为 MCP 服务端

Canal Engine 可以将自身工具暴露为 MCP 服务器，允许其他 MCP 客户端使用 Canal 的能力。

`gateway-mcp-server` crate 提供：

- 工具目录暴露
- 请求分派
- HTTP 和 stdio 传输

这意味着你可以将 Claude Desktop、Cursor 或任何 MCP 兼容客户端连接到 Canal Engine，通过标准 MCP 协议使用其所有工具（代码执行、文件操作、Git 等）。

## 工具聚合

Agent 看到的是来自三个来源的统一工具列表：

1. **内置工具** — 文件操作、代码执行、Bash、Git、搜索
2. **MCP 工具** — 来自已连接 MCP 服务器的所有工具
3. **插件工具** — 来自已安装连接器包的工具

所有工具通过命名空间隔离以避免冲突（如 `fs.read_file`、`agent.bash`、`browser.click`）。
