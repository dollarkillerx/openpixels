//! `openpixels-mcp`: the OpenPixels engine as a Model Context Protocol server, so AI agents
//! such as Claude Code and Codex can open, edit, look at and export photos.
//!
//! It speaks JSON-RPC over stdin and stdout, one message per line, as the MCP stdio
//! transport describes. Logs go to stderr. Run `openpixels-mcp --help` for setup.

mod edit;
mod pixels;
mod studio;

use std::io::{BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Map, Value, json};

/// Protocol revisions this server can speak, newest first. The tools, text and image
/// results it uses mean the same in all of them.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "OpenPixels edits photos non-destructively, like Lightroom. Typical workflow: \
list_settings once; open_photo; preview (and analyze for numbers); auto_tone or set_edit to adjust; \
preview with show: compare to check; export_photo to write a JPEG/PNG/TIFF, or save_edit to keep the edit next to the photo. \
batch_export applies one edit to a folder. Originals are never modified. Positions are fractions of the framed photo as preview shows it.";

const HELP: &str = "openpixels-mcp: the OpenPixels photo engine as an MCP server (stdio).

Claude Code:
  claude mcp add --scope user openpixels -- /path/to/openpixels-mcp

Codex:
  codex mcp add openpixels -- /path/to/openpixels-mcp
  Codex asks before tools that write files. To let it export without asking, add to
  ~/.codex/config.toml:
    [mcp_servers.openpixels.tools.export_photo]
    approval_mode = \"approve\"

Claude Desktop (claude_desktop_config.json):
  { \"mcpServers\": { \"openpixels\": { \"command\": \"/path/to/openpixels-mcp\" } } }

Options:
  --help      Show this text
  --version   Show the version
  --tools     Print the tool definitions as JSON
";

struct Server {
    studio: studio::Studio,
}

fn reply(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn failure(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

impl Server {
    /// Answers one message, or returns `None` for notifications and responses.
    fn handle(&mut self, message: Value) -> Option<Value> {
        let method = message.get("method").and_then(Value::as_str)?;
        // Notifications have no id and get no answer; there are none this server acts on.
        let id = message.get("id")?.clone();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        Some(match method {
            "initialize" => {
                let asked = params["protocolVersion"].as_str().unwrap_or("");
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOL_VERSIONS[1]);
                reply(
                    &id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": { "name": "openpixels", "title": "OpenPixels", "version": env!("CARGO_PKG_VERSION") },
                        "instructions": INSTRUCTIONS
                    }),
                )
            }
            "ping" => reply(&id, json!({})),
            "tools/list" => reply(&id, json!({ "tools": studio::tool_list() })),
            "tools/call" => {
                let Some(name) = params["name"].as_str() else {
                    return Some(failure(&id, -32602, "tools/call needs a tool name"));
                };
                let args = match &params["arguments"] {
                    Value::Object(map) => map.clone(),
                    Value::Null => Map::new(),
                    _ => return Some(failure(&id, -32602, "arguments must be an object")),
                };
                if !studio::tool_list().as_array().is_some_and(|tools| tools.iter().any(|t| t["name"] == name)) {
                    return Some(failure(&id, -32602, format!("unknown tool {name}")));
                }
                let outcome = catch_unwind(AssertUnwindSafe(|| self.studio.call_tool(name, &args))).unwrap_or_else(|_| {
                    // The engine may be half way through a change; start again rather than trust it.
                    self.studio = studio::Studio::default();
                    Err(format!("{name} failed unexpectedly; the engine was restarted, so open the photo again"))
                });
                match outcome {
                    Ok(content) => reply(&id, json!({ "content": content, "isError": false })),
                    Err(error) => reply(&id, json!({ "content": [{ "type": "text", "text": error }], "isError": true })),
                }
            }
            _ => failure(&id, -32601, format!("method {method} is not supported")),
        })
    }
}

fn main() {
    if let Some(flag) = std::env::args().nth(1) {
        match flag.as_str() {
            "--help" | "-h" => print!("{HELP}"),
            "--version" | "-V" => println!("openpixels-mcp {}", env!("CARGO_PKG_VERSION")),
            "--tools" => println!("{}", serde_json::to_string_pretty(&studio::tool_list()).unwrap_or_default()),
            other => {
                eprintln!("unknown option {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
        return;
    }
    let mut server = Server { studio: studio::Studio::default() };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let answers: Vec<Value> = batch.into_iter().filter_map(|message| server.handle(message)).collect();
                (!answers.is_empty()).then_some(Value::Array(answers))
            }
            Ok(message) => server.handle(message),
            Err(error) => Some(failure(&Value::Null, -32700, format!("not JSON: {error}"))),
        };
        if let Some(answer) = answer {
            // One line per message: serde_json never writes raw newlines inside a value.
            if writeln!(stdout, "{answer}").and_then(|()| stdout.flush()).is_err() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server { studio: studio::Studio::default() }
    }

    #[test]
    fn the_handshake_agrees_a_version() {
        let mut server = server();
        let answer = server.handle(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } } })).unwrap();
        assert_eq!(answer["result"]["protocolVersion"], "2025-03-26");
        assert!(answer["result"]["capabilities"]["tools"].is_object());
        let unknown = server.handle(json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } })).unwrap();
        assert_eq!(unknown["result"]["protocolVersion"], PROTOCOL_VERSIONS[1]);
        assert!(server.handle(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none());
    }

    #[test]
    fn tool_errors_are_results_and_protocol_errors_are_errors() {
        let mut server = server();
        let listed = server.handle(json!({ "jsonrpc": "2.0", "id": "a", "method": "tools/list" })).unwrap();
        assert!(listed["result"]["tools"].as_array().unwrap().len() >= 10);
        let no_photo = server.handle(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "get_edit", "arguments": {} } })).unwrap();
        assert_eq!(no_photo["result"]["isError"], true);
        let unknown = server.handle(json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "nope" } })).unwrap();
        assert_eq!(unknown["error"]["code"], -32602);
        let method = server.handle(json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/list" })).unwrap();
        assert_eq!(method["error"]["code"], -32601);
    }
}
