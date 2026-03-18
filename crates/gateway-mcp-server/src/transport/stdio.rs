//! STDIO transport for MCP server
//!
//! Reads JSON-RPC requests from stdin and writes responses to stdout.
//! Logging goes to stderr (MCP convention).

use super::RequestHandler;
use crate::protocol::JsonRpcRequest;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::error;

/// STDIO transport — reads from stdin, writes to stdout
pub struct StdioTransport;

impl StdioTransport {
    pub fn new() -> Self {
        Self
    }

    /// Run the STDIO loop with the given request handler
    pub async fn run<H>(&self, handler: H) -> anyhow::Result<()>
    where
        H: RequestHandler + Send + Sync + 'static,
    {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();

        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line).await?;
            if bytes_read == 0 {
                break; // EOF
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            match serde_json::from_str::<JsonRpcRequest>(trimmed) {
                Ok(request) => {
                    let response = handler.handle(request).await;
                    let response_json = serde_json::to_string(&response)?;
                    stdout.write_all(response_json.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                }
                Err(e) => {
                    error!("Failed to parse request: {}", e);
                    let error_response = crate::protocol::JsonRpcResponse::error(
                        serde_json::Value::Null,
                        -32700,
                        "Parse error".to_string(),
                    );
                    let response_json = serde_json::to_string(&error_response)?;
                    stdout.write_all(response_json.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                }
            }
        }

        Ok(())
    }
}

impl Default for StdioTransport {
    fn default() -> Self {
        Self::new()
    }
}
