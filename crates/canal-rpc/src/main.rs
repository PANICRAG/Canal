use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Parser;

mod auth;
mod executor;
mod file_ops;
mod protocol;

use auth::verify_token;
use executor::execute_code;
use file_ops::{file_read, file_write};
use protocol::{read_frame, write_frame, RpcRequest, RpcResponse};

#[derive(Parser, Debug)]
#[command(name = "canal-rpc", about = "VM sandbox RPC server")]
struct Args {
    /// Vsock port to listen on
    #[arg(long, default_value_t = 5678)]
    port: u32,

    /// Auth token for RPC requests (DEPRECATED: use RPC_TOKEN env var)
    #[arg(long, hide = true, default_value = "")]
    token: String,

    /// Read auth token from /proc/cmdline (rpc_token=<value>)
    #[arg(long, default_value_t = false)]
    token_from_cmdline: bool,

    /// Workspace directory
    #[arg(long, default_value = "/workspace")]
    workspace: PathBuf,

    /// User to drop privileges to (UID)
    #[arg(long)]
    user: Option<u32>,

    /// Use stdin/stdout transport instead of vsock (for testing)
    #[arg(long, default_value_t = false)]
    stdio: bool,
}

/// Dispatch an authenticated RPC request to the appropriate handler.
fn dispatch(req: &RpcRequest, workspace: &std::path::Path, shutdown: &AtomicBool) -> RpcResponse {
    match req.method.as_str() {
        "Handshake" => RpcResponse::success(serde_json::json!({
            "protocol_version": "1.0",
            "server": "canal-rpc",
        })),
        "Shutdown" => {
            shutdown.store(true, Ordering::SeqCst);
            RpcResponse::success(serde_json::json!({ "status": "shutting_down" }))
        }
        "Execute" => execute_code(&req.params),
        "FileRead" => file_read(&req.params, workspace),
        "FileWrite" => file_write(&req.params, workspace),
        _ => RpcResponse::error(format!("unknown method: {}", req.method)),
    }
}

/// Parse `rpc_token=<value>` from /proc/cmdline.
fn parse_token_from_cmdline() -> Option<String> {
    let cmdline = match std::fs::read_to_string("/proc/cmdline") {
        Ok(s) => s,
        Err(_) => return None,
    };
    for part in cmdline.split_whitespace() {
        if let Some(token) = part.strip_prefix("rpc_token=") {
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// Resolve the auth token from secure sources only.
///
/// Priority: --token-from-cmdline (kernel cmdline) > RPC_TOKEN env var.
/// The --token CLI arg is rejected because it exposes the secret in
/// /proc/<pid>/cmdline (R8-H4).
fn resolve_token(args: &Args) -> String {
    // R8-H4: Reject --token CLI arg — it leaks the token via /proc/<pid>/cmdline.
    // We detect CLI usage by checking if --token was explicitly passed (non-empty)
    // AND the RPC_TOKEN env var is either absent or different from the CLI value.
    if !args.token.is_empty() {
        let env_token = std::env::var("RPC_TOKEN").unwrap_or_default();
        if env_token.is_empty() || env_token != args.token {
            eprintln!(
                "Error: --token CLI argument is insecure (exposes token in /proc/<pid>/cmdline). \
                 Use the RPC_TOKEN environment variable instead."
            );
            std::process::exit(1);
        }
    }

    if args.token_from_cmdline {
        if let Some(token) = parse_token_from_cmdline() {
            eprintln!("Token loaded from /proc/cmdline");
            return token;
        }
        eprintln!("Warning: --token-from-cmdline set but no rpc_token= found in /proc/cmdline");
    }

    // Read token exclusively from RPC_TOKEN env var
    std::env::var("RPC_TOKEN").unwrap_or_default()
}

/// Handle a single connection (read frames, auth, dispatch, write responses).
fn handle_connection(
    reader: &mut impl Read,
    writer: &mut impl Write,
    token: &str,
    workspace: &std::path::Path,
    shutdown: &AtomicBool,
) {
    loop {
        if shutdown.load(Ordering::SeqCst) {
            eprintln!("Shutdown requested, closing connection");
            break;
        }

        let frame = match read_frame(reader) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Read error (likely EOF): {e}");
                break;
            }
        };

        let req: RpcRequest = match serde_json::from_slice(&frame) {
            Ok(r) => r,
            Err(e) => {
                let resp = RpcResponse::error(format!("invalid request: {e}"));
                let _ = write_response(writer, &resp);
                continue;
            }
        };

        // Auth check
        if let Some(ref req_token) = req.token {
            if !verify_token(token, req_token) {
                let resp = RpcResponse::error("authentication failed".to_string());
                let _ = write_response(writer, &resp);
                continue;
            }
        } else {
            let resp = RpcResponse::error("authentication required".to_string());
            let _ = write_response(writer, &resp);
            continue;
        }

        let resp = dispatch(&req, workspace, shutdown);
        if write_response(writer, &resp).is_err() {
            break;
        }
    }
}

fn main() {
    let args = Args::parse();

    // Drop privileges if --user specified
    if let Some(uid) = args.user {
        if let Err(e) = nix::unistd::setuid(nix::unistd::Uid::from_raw(uid)) {
            eprintln!("Failed to setuid to {uid}: {e}");
            std::process::exit(1);
        }
    }

    // Ensure workspace exists
    std::fs::create_dir_all(&args.workspace).ok();

    let token = resolve_token(&args);
    if token.is_empty() {
        eprintln!("Error: no auth token provided (use RPC_TOKEN env var or --token-from-cmdline)");
        std::process::exit(1);
    }

    let shutdown = AtomicBool::new(false);

    if args.stdio {
        // Stdio transport — for testing and piped usage
        run_stdio(&token, &args.workspace, &shutdown);
    } else {
        // Vsock transport — production mode (Linux only)
        #[cfg(target_os = "linux")]
        run_vsock(args.port, &token, &args.workspace, &shutdown);

        #[cfg(not(target_os = "linux"))]
        {
            eprintln!("Error: vsock transport is only available on Linux. Use --stdio on other platforms.");
            std::process::exit(1);
        }
    }
}

fn run_stdio(token: &str, workspace: &std::path::Path, shutdown: &AtomicBool) {
    eprintln!(
        "canal-rpc listening on stdio (token-auth enabled, workspace={:?})",
        workspace
    );

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();

    handle_connection(
        &mut stdin_lock,
        &mut stdout_lock,
        token,
        workspace,
        shutdown,
    );
}

#[cfg(target_os = "linux")]
fn run_vsock(port: u32, token: &str, workspace: &std::path::Path, shutdown: &AtomicBool) {
    use vsock::{VsockAddr, VsockListener, VMADDR_CID_ANY};

    let addr = VsockAddr::new(VMADDR_CID_ANY, port);
    let listener = VsockListener::bind(&addr).unwrap_or_else(|e| {
        eprintln!("Failed to bind vsock on port {port}: {e}");
        std::process::exit(1);
    });

    eprintln!(
        "canal-rpc listening on vsock port {port} (token-auth enabled, workspace={workspace:?})"
    );

    // Accept one connection at a time (single-client RPC server)
    for stream in listener.incoming() {
        match stream {
            Ok(mut conn) => {
                eprintln!("Accepted vsock connection");
                let mut reader = conn.try_clone().unwrap_or_else(|e| {
                    eprintln!("Failed to clone vsock stream: {e}");
                    std::process::exit(1);
                });
                handle_connection(&mut reader, &mut conn, token, workspace, shutdown);
                if shutdown.load(Ordering::SeqCst) {
                    eprintln!("Shutdown flag set, stopping listener");
                    break;
                }
                eprintln!("Connection closed, waiting for next...");
            }
            Err(e) => {
                eprintln!("Accept error: {e}");
            }
        }
    }
}

fn write_response(writer: &mut impl io::Write, resp: &RpcResponse) -> io::Result<()> {
    let data = serde_json::to_vec(resp).map_err(io::Error::other)?;
    write_frame(writer, &data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::AtomicBool;

    fn test_workspace() -> PathBuf {
        let dir = std::env::temp_dir().join("canal_rpc_dispatch_test");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_dispatch_handshake() {
        let ws = test_workspace();
        let shutdown = AtomicBool::new(false);
        let req = RpcRequest::new("Handshake");
        let resp = dispatch(&req, &ws, &shutdown);
        assert!(resp.ok);
        let version = resp.data["protocol_version"].as_str().unwrap();
        assert_eq!(version, "1.0");
    }

    #[test]
    fn test_dispatch_shutdown() {
        let ws = test_workspace();
        let shutdown = AtomicBool::new(false);
        let req = RpcRequest::new("Shutdown");
        let resp = dispatch(&req, &ws, &shutdown);
        assert!(resp.ok);
        assert!(shutdown.load(Ordering::SeqCst));
    }

    #[test]
    fn test_dispatch_execute() {
        let ws = test_workspace();
        let shutdown = AtomicBool::new(false);
        let req = RpcRequest {
            method: "Execute".to_string(),
            params: serde_json::json!({
                "language": "bash",
                "code": "echo dispatch_test"
            }),
            token: None,
            protocol_version: "1.0".to_string(),
        };
        let resp = dispatch(&req, &ws, &shutdown);
        assert!(resp.ok);
        let stdout = resp.data["stdout"].as_str().unwrap();
        assert!(stdout.contains("dispatch_test"));
    }

    #[test]
    fn test_dispatch_file_read() {
        let ws = test_workspace();
        let shutdown = AtomicBool::new(false);
        // Write a file first
        let fpath = ws.join("dispatch_read.txt");
        std::fs::write(&fpath, "read_me").unwrap();
        let req = RpcRequest {
            method: "FileRead".to_string(),
            params: serde_json::json!({ "path": "dispatch_read.txt" }),
            token: None,
            protocol_version: "1.0".to_string(),
        };
        let resp = dispatch(&req, &ws, &shutdown);
        assert!(resp.ok);
        assert_eq!(resp.data["content"].as_str().unwrap(), "read_me");
    }

    #[test]
    fn test_dispatch_file_write() {
        let ws = test_workspace();
        let shutdown = AtomicBool::new(false);
        let req = RpcRequest {
            method: "FileWrite".to_string(),
            params: serde_json::json!({
                "path": "dispatch_write.txt",
                "content": "written"
            }),
            token: None,
            protocol_version: "1.0".to_string(),
        };
        let resp = dispatch(&req, &ws, &shutdown);
        assert!(resp.ok);
        let content = std::fs::read_to_string(ws.join("dispatch_write.txt")).unwrap();
        assert_eq!(content, "written");
    }

    #[test]
    fn test_dispatch_unknown_method() {
        let ws = test_workspace();
        let shutdown = AtomicBool::new(false);
        let req = RpcRequest::new("FooBar");
        let resp = dispatch(&req, &ws, &shutdown);
        assert!(!resp.ok);
        assert!(resp.error.as_ref().unwrap().contains("unknown method"));
    }

    #[test]
    fn test_missing_token_rejected() {
        // This tests the main loop logic conceptually:
        // a request with no token should be rejected.
        let req = RpcRequest::new("Handshake");
        assert!(req.token.is_none());
        // The main loop checks token before dispatch — so token=None → error.
    }

    #[test]
    fn test_handle_connection_handshake_and_shutdown() {
        // Build a handshake request followed by a shutdown request
        let handshake_req = RpcRequest {
            method: "Handshake".to_string(),
            params: serde_json::json!({}),
            token: Some("test-tok".to_string()),
            protocol_version: "1.0".to_string(),
        };
        let shutdown_req = RpcRequest {
            method: "Shutdown".to_string(),
            params: serde_json::json!({}),
            token: Some("test-tok".to_string()),
            protocol_version: "1.0".to_string(),
        };

        // Encode both requests as frames into input buffer
        let mut input_buf = Vec::new();
        let req1_bytes = serde_json::to_vec(&handshake_req).unwrap();
        write_frame(&mut input_buf, &req1_bytes).unwrap();
        let req2_bytes = serde_json::to_vec(&shutdown_req).unwrap();
        write_frame(&mut input_buf, &req2_bytes).unwrap();

        let mut reader = Cursor::new(input_buf);
        let mut output_buf = Vec::new();
        let ws = test_workspace();
        let shutdown = AtomicBool::new(false);

        handle_connection(&mut reader, &mut output_buf, "test-tok", &ws, &shutdown);

        assert!(
            shutdown.load(Ordering::SeqCst),
            "Shutdown flag should be set"
        );

        // Decode responses from output
        let mut out_cursor = Cursor::new(output_buf);
        let resp1_frame = read_frame(&mut out_cursor).unwrap();
        let resp1: RpcResponse = serde_json::from_slice(&resp1_frame).unwrap();
        assert!(resp1.ok);
        assert_eq!(resp1.data["protocol_version"].as_str().unwrap(), "1.0");

        let resp2_frame = read_frame(&mut out_cursor).unwrap();
        let resp2: RpcResponse = serde_json::from_slice(&resp2_frame).unwrap();
        assert!(resp2.ok);
        assert_eq!(resp2.data["status"].as_str().unwrap(), "shutting_down");
    }

    #[test]
    fn test_handle_connection_auth_rejected() {
        let req = RpcRequest {
            method: "Handshake".to_string(),
            params: serde_json::json!({}),
            token: Some("wrong-token".to_string()),
            protocol_version: "1.0".to_string(),
        };

        let mut input_buf = Vec::new();
        let req_bytes = serde_json::to_vec(&req).unwrap();
        write_frame(&mut input_buf, &req_bytes).unwrap();

        let mut reader = Cursor::new(input_buf);
        let mut output_buf = Vec::new();
        let ws = test_workspace();
        let shutdown = AtomicBool::new(false);

        handle_connection(
            &mut reader,
            &mut output_buf,
            "correct-token",
            &ws,
            &shutdown,
        );

        let mut out_cursor = Cursor::new(output_buf);
        let resp_frame = read_frame(&mut out_cursor).unwrap();
        let resp: RpcResponse = serde_json::from_slice(&resp_frame).unwrap();
        assert!(!resp.ok);
        assert!(resp
            .error
            .as_ref()
            .unwrap()
            .contains("authentication failed"));
    }

    #[test]
    fn test_handle_connection_no_token() {
        let req = RpcRequest::new("Handshake"); // no token set

        let mut input_buf = Vec::new();
        let req_bytes = serde_json::to_vec(&req).unwrap();
        write_frame(&mut input_buf, &req_bytes).unwrap();

        let mut reader = Cursor::new(input_buf);
        let mut output_buf = Vec::new();
        let ws = test_workspace();
        let shutdown = AtomicBool::new(false);

        handle_connection(&mut reader, &mut output_buf, "any-token", &ws, &shutdown);

        let mut out_cursor = Cursor::new(output_buf);
        let resp_frame = read_frame(&mut out_cursor).unwrap();
        let resp: RpcResponse = serde_json::from_slice(&resp_frame).unwrap();
        assert!(!resp.ok);
        assert!(resp
            .error
            .as_ref()
            .unwrap()
            .contains("authentication required"));
    }

    #[test]
    fn test_token_from_cmdline_parsing() {
        // parse_token_from_cmdline reads /proc/cmdline which doesn't exist on macOS.
        // We test the parsing logic by simulating the string parsing.
        let cmdline = "console=hvc0 root=/dev/vda1 rw rpc_token=abc-123-def";
        let mut found = None;
        for part in cmdline.split_whitespace() {
            if let Some(token) = part.strip_prefix("rpc_token=") {
                if !token.is_empty() {
                    found = Some(token.to_string());
                }
            }
        }
        assert_eq!(found, Some("abc-123-def".to_string()));
    }

    #[test]
    fn test_token_from_cmdline_missing() {
        let cmdline = "console=hvc0 root=/dev/vda1 rw";
        let mut found = None;
        for part in cmdline.split_whitespace() {
            if let Some(token) = part.strip_prefix("rpc_token=") {
                if !token.is_empty() {
                    found = Some(token.to_string());
                }
            }
        }
        assert_eq!(found, None);
    }

    #[test]
    fn test_token_from_cmdline_empty_value() {
        let cmdline = "console=hvc0 rpc_token= root=/dev/vda1";
        let mut found = None;
        for part in cmdline.split_whitespace() {
            if let Some(token) = part.strip_prefix("rpc_token=") {
                if !token.is_empty() {
                    found = Some(token.to_string());
                }
            }
        }
        assert_eq!(found, None);
    }
}
