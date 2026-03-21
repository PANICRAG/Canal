//! Tool Code Generator - Generates SDK preamble code for sandbox execution
//!
//! Creates the Python/JavaScript SDK code that is prepended to LLM-generated
//! code, providing a `tools` object that proxies tool calls via HTTP to
//! the ToolProxyBridge.

use crate::agent::tools::ToolMetadata;

/// Generates SDK preamble code for sandbox tool access
pub struct ToolCodeGenerator;

impl ToolCodeGenerator {
    /// Generate Python SDK preamble code
    ///
    /// The generated code provides a `tools` object and a `context` dict
    /// that the LLM-generated code can use to call registered tools via
    /// the HTTP proxy bridge.
    pub fn generate_python_preamble(
        port: u16,
        context_json: &str,
        available_tools: &[ToolMetadata],
    ) -> String {
        let tool_methods = Self::generate_python_tool_methods(available_tools);

        format!(
            r#"# === Canal Tool SDK (auto-generated) ===
import json
import urllib.request
import urllib.error
import sys

class _ToolProxy:
    """Proxy for calling registered tools via the HTTP bridge."""

    def __init__(self, base_url):
        self._url = base_url
        self._call_count = 0

    def call(self, tool_name, **kwargs):
        """Call any registered tool by name.

        Args:
            tool_name: Name of the tool (e.g., "Read", "Bash", "Glob")
            **kwargs: Tool-specific arguments

        Returns:
            Tool result as a Python object (dict, list, string, etc.)

        Raises:
            RuntimeError: If the tool call fails
        """
        self._call_count += 1
        data = json.dumps({{"tool_name": tool_name, "arguments": kwargs}})
        req = urllib.request.Request(
            f"{{self._url}}/call_tool",
            data.encode("utf-8"),
            headers={{"Content-Type": "application/json"}},
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=120) as resp:
                result = json.loads(resp.read())
                if result.get("error"):
                    raise RuntimeError(f"Tool {{tool_name}} failed: {{result['error']}}")
                return result.get("result")
        except urllib.error.URLError as e:
            raise RuntimeError(f"Tool {{tool_name}} connection error: {{e}}")
        except urllib.error.HTTPError as e:
            body = e.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"Tool {{tool_name}} HTTP error {{e.code}}: {{body}}")

    # === Convenience methods ===
{tool_methods}

tools = _ToolProxy("http://host.docker.internal:{port}")
context = {context_json}

# === End Canal Tool SDK ===

"#
        )
    }

    /// Generate JavaScript SDK preamble code
    pub fn generate_javascript_preamble(
        port: u16,
        context_json: &str,
        available_tools: &[ToolMetadata],
    ) -> String {
        let tool_methods = Self::generate_js_tool_methods(available_tools);

        format!(
            r#"// === Canal Tool SDK (auto-generated) ===
const http = require('http');

class _ToolProxy {{
    constructor(baseUrl) {{
        this._url = baseUrl;
        this._callCount = 0;
    }}

    async call(toolName, kwargs = {{}}) {{
        this._callCount++;
        const data = JSON.stringify({{ tool_name: toolName, arguments: kwargs }});

        return new Promise((resolve, reject) => {{
            const url = new URL(`${{this._url}}/call_tool`);
            const options = {{
                hostname: url.hostname,
                port: url.port,
                path: url.pathname,
                method: 'POST',
                headers: {{
                    'Content-Type': 'application/json',
                    'Content-Length': Buffer.byteLength(data),
                }},
                timeout: 120000,
            }};

            const req = http.request(options, (res) => {{
                let body = '';
                res.on('data', (chunk) => body += chunk);
                res.on('end', () => {{
                    try {{
                        const result = JSON.parse(body);
                        if (result.error) {{
                            reject(new Error(`Tool ${{toolName}} failed: ${{result.error}}`));
                        }} else {{
                            resolve(result.result);
                        }}
                    }} catch (e) {{
                        reject(new Error(`Tool ${{toolName}} parse error: ${{e.message}}`));
                    }}
                }});
            }});

            req.on('error', (e) => reject(new Error(`Tool ${{toolName}} error: ${{e.message}}`)));
            req.on('timeout', () => {{
                req.destroy();
                reject(new Error(`Tool ${{toolName}} timed out`));
            }});

            req.write(data);
            req.end();
        }});
    }}

    // === Convenience methods ===
{tool_methods}
}}

const tools = new _ToolProxy('http://host.docker.internal:{port}');
const context = {context_json};

// === End Canal Tool SDK ===

"#
        )
    }

    /// Generate the preamble for the specified language
    pub fn generate_preamble(
        language: &str,
        port: u16,
        context_json: &str,
        available_tools: &[ToolMetadata],
    ) -> Result<String, String> {
        match language {
            "python" => Ok(Self::generate_python_preamble(
                port,
                context_json,
                available_tools,
            )),
            "javascript" => Ok(Self::generate_javascript_preamble(
                port,
                context_json,
                available_tools,
            )),
            _ => Err(format!(
                "Unsupported language for code orchestration: {}",
                language
            )),
        }
    }

    /// Generate Python convenience methods for common tools
    fn generate_python_tool_methods(_tools: &[ToolMetadata]) -> String {
        let mut methods = Vec::new();

        // Always include standard convenience methods
        methods.push(
            r#"    def read(self, path):
        """Read a file and return its content."""
        return self.call("Read", file_path=path)"#
                .to_string(),
        );

        methods.push(
            r#"    def write(self, path, content):
        """Write content to a file."""
        return self.call("Write", file_path=path, content=content)"#
                .to_string(),
        );

        methods.push(
            r#"    def edit(self, path, old_string, new_string):
        """Edit a file by replacing old_string with new_string."""
        return self.call("Edit", file_path=path, old_string=old_string, new_string=new_string)"#
                .to_string(),
        );

        methods.push(
            r#"    def bash(self, cmd, timeout=None):
        """Execute a bash command."""
        kwargs = {"command": cmd}
        if timeout is not None:
            kwargs["timeout"] = timeout
        return self.call("Bash", **kwargs)"#
                .to_string(),
        );

        methods.push(
            r#"    def glob(self, pattern, path=None):
        """Find files matching a glob pattern."""
        kwargs = {"pattern": pattern}
        if path is not None:
            kwargs["path"] = path
        return self.call("Glob", **kwargs)"#
                .to_string(),
        );

        methods.push(
            r#"    def grep(self, pattern, path=None, glob_filter=None):
        """Search file contents with regex."""
        kwargs = {"pattern": pattern}
        if path is not None:
            kwargs["path"] = path
        if glob_filter is not None:
            kwargs["glob"] = glob_filter
        return self.call("Grep", **kwargs)"#
                .to_string(),
        );

        methods.push(
            r#"    def mcp(self, server, tool, **kwargs):
        """Call an MCP tool (namespace_toolname format)."""
        return self.call(f"{server}_{tool}", **kwargs)"#
                .to_string(),
        );

        methods.join("\n\n")
    }

    /// Generate JavaScript convenience methods for common tools
    fn generate_js_tool_methods(_tools: &[ToolMetadata]) -> String {
        let methods = vec![
            r#"    async read(path) {
        return this.call('Read', { file_path: path });
    }"#,
            r#"    async write(path, content) {
        return this.call('Write', { file_path: path, content });
    }"#,
            r#"    async edit(path, oldString, newString) {
        return this.call('Edit', { file_path: path, old_string: oldString, new_string: newString });
    }"#,
            r#"    async bash(cmd, timeout) {
        const kwargs = { command: cmd };
        if (timeout !== undefined) kwargs.timeout = timeout;
        return this.call('Bash', kwargs);
    }"#,
            r#"    async glob(pattern, path) {
        const kwargs = { pattern };
        if (path !== undefined) kwargs.path = path;
        return this.call('Glob', kwargs);
    }"#,
            r#"    async grep(pattern, path, globFilter) {
        const kwargs = { pattern };
        if (path !== undefined) kwargs.path = path;
        if (globFilter !== undefined) kwargs.glob = globFilter;
        return this.call('Grep', kwargs);
    }"#,
            r#"    async mcp(server, tool, kwargs = {}) {
        return this.call(`${server}_${tool}`, kwargs);
    }"#,
        ];

        methods.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_python_preamble_generation() {
        let preamble = ToolCodeGenerator::generate_python_preamble(8080, "{}", &[]);

        assert!(preamble.contains("class _ToolProxy"));
        assert!(preamble.contains("http://host.docker.internal:8080"));
        assert!(preamble.contains("tools = _ToolProxy"));
        assert!(preamble.contains("context = {}"));
        assert!(preamble.contains("def read(self, path)"));
        assert!(preamble.contains("def bash(self, cmd"));
        assert!(preamble.contains("def glob(self, pattern"));
        assert!(preamble.contains("def grep(self, pattern"));
    }

    #[test]
    fn test_javascript_preamble_generation() {
        let preamble = ToolCodeGenerator::generate_javascript_preamble(9090, "{}", &[]);

        assert!(preamble.contains("class _ToolProxy"));
        assert!(preamble.contains("http://host.docker.internal:9090"));
        assert!(preamble.contains("const tools = new _ToolProxy"));
        assert!(preamble.contains("const context = {}"));
        assert!(preamble.contains("async read(path)"));
        assert!(preamble.contains("async bash(cmd"));
    }

    #[test]
    fn test_generate_preamble_unsupported_language() {
        let result = ToolCodeGenerator::generate_preamble("ruby", 8080, "{}", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported"));
    }

    #[test]
    fn test_python_preamble_with_context() {
        let context = r#"{"step_1_output": "hello", "data": [1, 2, 3]}"#;
        let preamble = ToolCodeGenerator::generate_python_preamble(8080, context, &[]);

        assert!(preamble.contains(context));
    }
}
