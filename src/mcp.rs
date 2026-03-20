//! MCP (Model Context Protocol) server over stdio.
//!
//! Implements the minimal MCP subset: `initialize`, `tools/list`, `tools/call`,
//! and `notifications/initialized`. Communicates via newline-delimited JSON-RPC
//! over stdin/stdout.

use crate::{RustBash, RustBashBuilder};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

const MAX_OUTPUT_LEN: usize = 100_000;

/// Run the MCP server loop, reading JSON-RPC messages from stdin and writing
/// responses to stdout. Each line is one JSON-RPC message.
pub fn run_mcp_server() -> Result<(), Box<dyn std::error::Error>> {
    let builder = RustBashBuilder::new()
        .env(HashMap::from([
            ("HOME".to_string(), "/home".to_string()),
            ("USER".to_string(), "user".to_string()),
            ("PWD".to_string(), "/".to_string()),
        ]))
        .cwd("/");
    let mut shell = builder.build()?;

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let error_response = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {e}")
                    }
                });
                write_response(&mut stdout, &error_response)?;
                continue;
            }
        };

        if let Some(response) = handle_message(&mut shell, &request) {
            write_response(&mut stdout, &response)?;
        }
        // Notifications (no "id") that we don't respond to just get dropped
    }

    Ok(())
}

fn write_response(stdout: &mut impl Write, response: &Value) -> io::Result<()> {
    let serialized = serde_json::to_string(response).expect("JSON serialization should not fail");
    writeln!(stdout, "{serialized}")?;
    stdout.flush()
}

fn handle_message(shell: &mut RustBash, request: &Value) -> Option<Value> {
    let id = request.get("id");

    // Notifications have no "id" — we don't respond to them
    if id.is_none() || id == Some(&Value::Null) {
        return None;
    }

    let id = id.unwrap().clone();

    let method = match request.get("method").and_then(|v| v.as_str()) {
        Some(m) => m,
        None => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32600,
                    "message": "Invalid Request: missing or non-string method"
                }
            }));
        }
    };

    let result = match method {
        "initialize" => handle_initialize(),
        "tools/list" => handle_tools_list(),
        "tools/call" => handle_tools_call(shell, request.get("params")),
        _ => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("Method not found: {method}")
                }
            }));
        }
    };

    match result {
        Ok(value) => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": value
        })),
        Err(e) => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32603,
                "message": e
            }
        })),
    }
}

fn handle_initialize() -> Result<Value, String> {
    Ok(json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "rust-bash",
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
}

fn handle_tools_list() -> Result<Value, String> {
    Ok(json!({
        "tools": [
            {
                "name": "bash",
                "description": "Execute bash commands in a sandboxed environment with an in-memory virtual filesystem. Supports standard Unix utilities including grep, sed, awk, jq, cat, echo, and more. All file operations are isolated within the sandbox. State persists between calls.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The bash command to execute"
                        }
                    },
                    "required": ["command"]
                }
            },
            {
                "name": "write_file",
                "description": "Write content to a file in the sandboxed virtual filesystem. Creates parent directories automatically.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The absolute path to write to"
                        },
                        "content": {
                            "type": "string",
                            "description": "The content to write"
                        }
                    },
                    "required": ["path", "content"]
                }
            },
            {
                "name": "read_file",
                "description": "Read the contents of a file from the sandboxed virtual filesystem.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The absolute path to read"
                        }
                    },
                    "required": ["path"]
                }
            },
            {
                "name": "list_directory",
                "description": "List the contents of a directory in the sandboxed virtual filesystem.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The absolute path of the directory to list"
                        }
                    },
                    "required": ["path"]
                }
            }
        ]
    }))
}

fn truncate_output(s: &str) -> String {
    if s.len() <= MAX_OUTPUT_LEN {
        return s.to_string();
    }
    // Find a valid UTF-8 char boundary at or before MAX_OUTPUT_LEN
    let mut end = MAX_OUTPUT_LEN;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n... (truncated, {} total chars)", &s[..end], s.len())
}

fn handle_tools_call(shell: &mut RustBash, params: Option<&Value>) -> Result<Value, String> {
    let params = params.ok_or("Missing params")?;
    let tool_name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing tool name")?;
    let empty_obj = Value::Object(Default::default());
    let arguments = params.get("arguments").unwrap_or(&empty_obj);

    match tool_name {
        "bash" => {
            let command = arguments
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or("Missing 'command' argument")?;

            match shell.exec(command) {
                Ok(result) => {
                    let stdout = truncate_output(&result.stdout);
                    let stderr = truncate_output(&result.stderr);
                    let text = format!(
                        "stdout:\n{stdout}\nstderr:\n{stderr}\nexit_code: {}",
                        result.exit_code
                    );
                    let is_error = result.exit_code != 0;
                    Ok(json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": is_error
                    }))
                }
                Err(e) => Ok(json!({
                    "content": [{ "type": "text", "text": format!("Error: {e}") }],
                    "isError": true
                })),
            }
        }
        "write_file" => {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("Missing 'path' argument")?;
            let content = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("Missing 'content' argument")?;

            match shell.write_file(path, content.as_bytes()) {
                Ok(()) => Ok(json!({
                    "content": [{ "type": "text", "text": format!("Written {path}") }]
                })),
                Err(e) => Ok(json!({
                    "content": [{ "type": "text", "text": format!("Error: {e}") }],
                    "isError": true
                })),
            }
        }
        "read_file" => {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("Missing 'path' argument")?;

            match shell.read_file(path) {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    Ok(json!({
                        "content": [{ "type": "text", "text": text }]
                    }))
                }
                Err(e) => Ok(json!({
                    "content": [{ "type": "text", "text": format!("Error: {e}") }],
                    "isError": true
                })),
            }
        }
        "list_directory" => {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("Missing 'path' argument")?;

            match shell.readdir(path) {
                Ok(entries) => {
                    let listing: Vec<String> = entries
                        .iter()
                        .map(|e| {
                            let suffix = match e.node_type {
                                crate::vfs::NodeType::Directory => "/",
                                _ => "",
                            };
                            format!("{}{suffix}", e.name)
                        })
                        .collect();
                    let text = listing.join("\n");
                    Ok(json!({
                        "content": [{ "type": "text", "text": text }]
                    }))
                }
                Err(e) => Ok(json!({
                    "content": [{ "type": "text", "text": format!("Error: {e}") }],
                    "isError": true
                })),
            }
        }
        _ => Err(format!("Unknown tool: {tool_name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize_response() {
        let result = handle_initialize().unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["serverInfo"]["name"].as_str().unwrap() == "rust-bash");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[test]
    fn test_tools_list_returns_four_tools() {
        let result = handle_tools_list().unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 4);

        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"list_directory"));
    }

    #[test]
    fn test_tools_list_schemas_have_required_fields() {
        let result = handle_tools_list().unwrap();
        let tools = result["tools"].as_array().unwrap();
        for tool in tools {
            assert!(tool["name"].is_string());
            assert!(tool["description"].is_string());
            assert!(tool["inputSchema"]["type"].as_str().unwrap() == "object");
            assert!(tool["inputSchema"]["properties"].is_object());
            assert!(tool["inputSchema"]["required"].is_array());
        }
    }

    fn create_test_shell() -> RustBash {
        RustBashBuilder::new()
            .cwd("/")
            .env(HashMap::from([
                ("HOME".to_string(), "/home".to_string()),
                ("USER".to_string(), "user".to_string()),
            ]))
            .build()
            .unwrap()
    }

    #[test]
    fn test_bash_tool_call() {
        let mut shell = create_test_shell();
        let params = json!({
            "name": "bash",
            "arguments": { "command": "echo hello" }
        });
        let result = handle_tools_call(&mut shell, Some(&params)).unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("hello"));
        assert!(text.contains("exit_code: 0"));
    }

    #[test]
    fn test_write_and_read_file_tool() {
        let mut shell = create_test_shell();

        // Write a file
        let write_params = json!({
            "name": "write_file",
            "arguments": { "path": "/test.txt", "content": "test content" }
        });
        let result = handle_tools_call(&mut shell, Some(&write_params)).unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Written"));

        // Read it back
        let read_params = json!({
            "name": "read_file",
            "arguments": { "path": "/test.txt" }
        });
        let result = handle_tools_call(&mut shell, Some(&read_params)).unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(text, "test content");
    }

    #[test]
    fn test_list_directory_tool() {
        let mut shell = create_test_shell();

        // Create a file first
        shell.write_file("/mydir/a.txt", b"a").unwrap();
        shell.write_file("/mydir/b.txt", b"b").unwrap();

        let params = json!({
            "name": "list_directory",
            "arguments": { "path": "/mydir" }
        });
        let result = handle_tools_call(&mut shell, Some(&params)).unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("a.txt"));
        assert!(text.contains("b.txt"));
    }

    #[test]
    fn test_read_nonexistent_file_returns_error() {
        let mut shell = create_test_shell();
        let params = json!({
            "name": "read_file",
            "arguments": { "path": "/nonexistent.txt" }
        });
        let result = handle_tools_call(&mut shell, Some(&params)).unwrap();
        assert_eq!(result["isError"], true);
    }

    #[test]
    fn test_unknown_tool_returns_error() {
        let mut shell = create_test_shell();
        let params = json!({
            "name": "unknown_tool",
            "arguments": {}
        });
        let result = handle_tools_call(&mut shell, Some(&params));
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_message_initialize() {
        let mut shell = create_test_shell();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        });
        let response = handle_message(&mut shell, &request).unwrap();
        assert_eq!(response["id"], 1);
        assert!(response["result"]["serverInfo"].is_object());
    }

    #[test]
    fn test_handle_message_notification_returns_none() {
        let mut shell = create_test_shell();
        let request = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let response = handle_message(&mut shell, &request);
        assert!(response.is_none());
    }

    #[test]
    fn test_handle_message_unknown_method() {
        let mut shell = create_test_shell();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "unknown/method",
            "params": {}
        });
        let response = handle_message(&mut shell, &request).unwrap();
        assert!(response["error"]["code"].as_i64().unwrap() == -32601);
    }

    #[test]
    fn test_bash_error_command_returns_is_error() {
        let mut shell = create_test_shell();
        let params = json!({
            "name": "bash",
            "arguments": { "command": "cat /nonexistent_file_404" }
        });
        let result = handle_tools_call(&mut shell, Some(&params)).unwrap();
        assert_eq!(result["isError"], true);
    }

    #[test]
    fn test_stateful_session() {
        let mut shell = create_test_shell();

        // Set a variable
        let params1 = json!({
            "name": "bash",
            "arguments": { "command": "export MY_VAR=hello123" }
        });
        handle_tools_call(&mut shell, Some(&params1)).unwrap();

        // Read it back
        let params2 = json!({
            "name": "bash",
            "arguments": { "command": "echo $MY_VAR" }
        });
        let result = handle_tools_call(&mut shell, Some(&params2)).unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("hello123"));
    }

    #[test]
    fn test_handle_message_missing_method_with_id() {
        let mut shell = create_test_shell();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7
        });
        let response = handle_message(&mut shell, &request).unwrap();
        assert_eq!(response["error"]["code"], -32600);
    }

    #[test]
    fn test_handle_message_non_string_method_with_id() {
        let mut shell = create_test_shell();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": 42
        });
        let response = handle_message(&mut shell, &request).unwrap();
        assert_eq!(response["error"]["code"], -32600);
    }

    #[test]
    fn test_truncate_output_short() {
        let s = "hello world";
        assert_eq!(truncate_output(s), s);
    }

    #[test]
    fn test_truncate_output_long() {
        let s = "x".repeat(MAX_OUTPUT_LEN + 100);
        let result = truncate_output(&s);
        assert!(result.len() < s.len());
        assert!(result.contains("truncated"));
    }
}
