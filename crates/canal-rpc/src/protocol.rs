//! RPC protocol — length-prefixed JSON framing over a byte stream.
//!
//! Wire format: `[4-byte big-endian length][JSON payload]`

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum frame size: 10 MB.
pub const MAX_FRAME_SIZE: u32 = 10 * 1024 * 1024;

/// An RPC request sent from the Swift host to the guest VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub method: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default = "default_protocol_version")]
    pub protocol_version: String,
}

fn default_protocol_version() -> String {
    "1.0".to_string()
}

impl RpcRequest {
    /// Create a new request with just a method name (convenience for tests).
    #[allow(dead_code)]
    pub fn new(method: &str) -> Self {
        Self {
            method: method.to_string(),
            params: Value::Null,
            token: None,
            protocol_version: "1.0".to_string(),
        }
    }
}

/// An RPC response sent from the guest VM back to the Swift host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub ok: bool,
    #[serde(default)]
    pub data: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RpcResponse {
    /// Create a success response with data.
    pub fn success(data: Value) -> Self {
        Self {
            ok: true,
            data,
            error: None,
        }
    }

    /// Create an error response.
    pub fn error(msg: String) -> Self {
        Self {
            ok: false,
            data: Value::Null,
            error: Some(msg),
        }
    }
}

/// Read a length-prefixed frame from a reader.
///
/// Returns the raw bytes of the payload.
pub fn read_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);

    if len == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty frame"));
    }
    if len > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame too large: {len} bytes (max {MAX_FRAME_SIZE})"),
        ));
    }

    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

/// Write a length-prefixed frame to a writer.
///
/// R8-M5: Validates payload size fits in u32 and respects MAX_FRAME_SIZE.
pub fn write_frame(writer: &mut impl Write, data: &[u8]) -> io::Result<()> {
    let len: u32 = data.len().try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "frame payload too large: {} bytes exceeds u32 max",
                data.len()
            ),
        )
    })?;
    if len > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("frame too large: {len} bytes (max {MAX_FRAME_SIZE})"),
        ));
    }
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(data)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_rpc_request_serialization() {
        let req = RpcRequest {
            method: "Execute".to_string(),
            params: serde_json::json!({"code": "print(1)"}),
            token: Some("tok".to_string()),
            protocol_version: "1.0".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"method\":\"Execute\""));
        assert!(json.contains("\"protocol_version\":\"1.0\""));
    }

    #[test]
    fn test_rpc_request_deserialization() {
        let json = r#"{"method":"Test","params":{"a":1},"token":"t","protocol_version":"1.0"}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "Test");
        assert_eq!(req.token, Some("t".to_string()));
        assert_eq!(req.protocol_version, "1.0");
        assert_eq!(req.params["a"], 1);
    }

    #[test]
    fn test_rpc_response_success() {
        let resp = RpcResponse::success(serde_json::json!({"out": "hi"}));
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: RpcResponse = serde_json::from_str(&json).unwrap();
        assert!(decoded.ok);
        assert_eq!(decoded.data["out"], "hi");
        assert!(decoded.error.is_none());
    }

    #[test]
    fn test_rpc_response_error() {
        let resp = RpcResponse::error("timeout".to_string());
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: RpcResponse = serde_json::from_str(&json).unwrap();
        assert!(!decoded.ok);
        assert_eq!(decoded.error.as_deref(), Some("timeout"));
    }

    #[test]
    fn test_read_write_frame_roundtrip() {
        let payload = b"hello world";
        let mut buf = Vec::new();
        write_frame(&mut buf, payload).unwrap();

        let mut cursor = Cursor::new(buf);
        let result = read_frame(&mut cursor).unwrap();
        assert_eq!(result, payload);
    }

    #[test]
    fn test_frame_length_encoding() {
        let payload = b"test";
        let mut buf = Vec::new();
        write_frame(&mut buf, payload).unwrap();
        // First 4 bytes should be big-endian length
        let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        assert_eq!(len, 4);
    }

    #[test]
    fn test_frame_max_size_rejected() {
        // Create a frame header claiming > MAX_FRAME_SIZE
        let fake_len: u32 = MAX_FRAME_SIZE + 1;
        let mut buf = Vec::new();
        buf.extend_from_slice(&fake_len.to_be_bytes());
        buf.extend_from_slice(&[0u8; 16]); // some data

        let mut cursor = Cursor::new(buf);
        let result = read_frame(&mut cursor);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn test_empty_frame() {
        // Length = 0
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes());

        let mut cursor = Cursor::new(buf);
        let result = read_frame(&mut cursor);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_protocol_version_default() {
        let req = RpcRequest::new("Test");
        assert_eq!(req.protocol_version, "1.0");
    }
}
