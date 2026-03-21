# MCP Integration

Canal Engine provides full [Model Context Protocol](https://modelcontextprotocol.io/) support as both a client (connecting to MCP servers) and a server (exposing tools via MCP).

## As MCP Client

### Pre-configured Servers

Canal ships with several MCP servers in `config/mcp-servers.yaml`:

| Server | Namespace | Transport | Description |
|--------|-----------|-----------|-------------|
| filesystem | `fs` | stdio | File read/write/delete/search |
| browser | `browser` | SSE | Chrome extension browser automation |
| macOS | `mac` | stdio | AppleScript automation |
| windows | `win` | stdio | Windows GUI automation |
| videocli | `videocli` | stdio | Video creation |
| davinci | `davinci` | stdio | DaVinci Resolve editing |

### Adding MCP Servers

#### Via Configuration

Add to `config/mcp-servers.yaml`:

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

#### Via API (Runtime)

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

### Transport Types

| Transport | Use Case | Configuration |
|-----------|----------|---------------|
| `stdio` | Local processes | `command` + `args` |
| `sse` | Remote HTTP servers | `url` |

### Tool Permissions

Control which MCP tools can run without confirmation:

```yaml
tool_permissions:
  # Tools requiring user confirmation before execution
  require_confirmation:
    - "fs.delete"
    - "fs.write_file"
    - "davinci.render"

  # Tools that are completely blocked
  blocked:
    - "fs.execute"

  # Rate limits per namespace (requests/minute)
  rate_limits:
    fs: 100
    videocli: 30
    davinci: 10
    browser: 60
```

### Listing Tools

```bash
# All tools from all sources
curl http://localhost:4000/api/tools -H "Authorization: Bearer TOKEN"

# Tools from a specific MCP server
curl http://localhost:4000/api/mcp/servers/filesystem/tools \
  -H "Authorization: Bearer TOKEN"
```

### Calling MCP Tools

```bash
curl -X POST http://localhost:4000/api/tools/fs/read_file/call \
  -H "Authorization: Bearer TOKEN" \
  -d '{"path": "/tmp/test.txt"}'
```

### Health Monitoring

```bash
# Check specific server health
curl http://localhost:4000/api/mcp/servers/filesystem/health \
  -H "Authorization: Bearer TOKEN"
```

## As MCP Server

Canal Engine can expose its tools as an MCP server, allowing other MCP clients to use Canal's capabilities.

The `gateway-mcp-server` crate provides:

- Tool catalog exposure
- Request dispatching
- HTTP and stdio transports

This means you can connect Claude Desktop, Cursor, or any MCP-compatible client to Canal Engine and use all its tools (code execution, file operations, git, etc.) through the standard MCP protocol.

## Tool Aggregation

The agent sees a unified tool list from three sources:

1. **Built-in tools** — File operations, code execution, bash, git, search
2. **MCP tools** — All tools from connected MCP servers
3. **Plugin tools** — Tools from installed connector bundles

All tools are namespaced to avoid conflicts (e.g., `fs.read_file`, `agent.bash`, `browser.click`).
