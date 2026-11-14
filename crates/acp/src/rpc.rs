use serde_json::{Value, json};

use crate::AcpError;

const MAX_LINE: usize = 1024 * 1024;

pub fn encode(message: &Value) -> Vec<u8> {
    let mut bytes = message.to_string().into_bytes();
    bytes.push(b'\n');
    bytes
}

pub fn decode_line(line: &str) -> Result<Value, AcpError> {
    if line.len() > MAX_LINE {
        return Err(AcpError::TooLarge);
    }
    serde_json::from_str(line).map_err(|_| AcpError::Protocol)
}

/// ACP `initialize`. Protocol version 1. Writes come through the client; reads do not.
pub fn initialize(id: i64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": { "readTextFile": false, "writeTextFile": true },
                "terminal": false
            },
            "clientInfo": { "name": "pen", "title": "Pen", "version": "0.1.0" }
        }
    })
}

pub fn session_new(id: i64, cwd: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/new",
        "params": { "cwd": cwd, "mcpServers": [] }
    })
}

/// ACP `session/prompt`. Recorded in the phase-5 panel commit.
pub fn session_prompt(id: i64, session_id: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": text }]
        }
    })
}

pub fn write_rejected(id: i64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32002, "message": "write rejected" }
    })
}

pub fn write_ok(id: i64) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": {} })
}

pub fn permission_selected(id: i64, option_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "outcome": { "outcome": "selected", "optionId": option_id } }
    })
}

pub fn permission_cancelled(id: i64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "outcome": { "outcome": "cancelled" } }
    })
}

/// Text from a `session/update` agent message chunk, if this frame has one.
pub fn agent_text(message: &Value) -> Option<String> {
    if message.get("method").and_then(Value::as_str) != Some("session/update") {
        return None;
    }
    message
        .pointer("/params/update/content/text")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub fn session_id(message: &Value) -> Option<String> {
    message
        .pointer("/result/sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub fn request_id(message: &Value) -> Option<i64> {
    message.get("id").and_then(Value::as_i64)
}

pub fn method(message: &Value) -> Option<&str> {
    message.get("method").and_then(Value::as_str)
}

pub fn write_request(message: &Value) -> Option<(String, String)> {
    if method(message) != Some("fs/write_text_file") {
        return None;
    }
    let path = message.pointer("/params/path")?.as_str()?.to_owned();
    let content = message.pointer("/params/content")?.as_str()?.to_owned();
    Some((path, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_one_line() {
        let v = serde_json::json!({"jsonrpc":"2.0","method":"initialize","id":1});
        let bytes = encode(&v);
        let line = std::str::from_utf8(&bytes).unwrap().trim_end();
        assert_eq!(decode_line(line).unwrap(), v);
    }

    #[test]
    fn oversized_line_is_rejected() {
        let line = "x".repeat(MAX_LINE + 1);
        assert!(matches!(decode_line(&line), Err(AcpError::TooLarge)));
    }
}
