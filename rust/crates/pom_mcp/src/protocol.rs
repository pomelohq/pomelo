//! MCP over stdio: one JSON-RPC 2.0 message per line in, one per line out. Only the tools capability is
//! offered. A failing tool is a normal result with `isError`, so the agent reads the message.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{json, Map, Value};

const DEFAULT_PROTOCOL: &str = "2025-06-18";

pub type ToolArgs = Map<String, Value>;
pub type ToolFn = Box<dyn Fn(&ToolArgs) -> Result<String, String>>;

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// `None` means no arguments.
    pub schema: Option<Value>,
    pub read_only: bool,
    pub destructive: bool,
    /// Longer results are saved to a file and only their head is returned.
    pub max_result_chars: usize,
    pub run: ToolFn,
}

pub struct Server {
    pub name: String,
    pub version: String,
    pub tools: Vec<Tool>,
    /// Where results over a tool's limit are saved in full.
    pub overflow_dir: PathBuf,
}

impl Server {
    /// Answers requests until the input ends. Lines that are not JSON are skipped, as are replies to
    /// notifications (no `id`).
    pub fn serve(&self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let Ok(request) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(reply) = self.handle(&request) {
                serde_json::to_writer(&mut output, &reply)?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    pub fn handle(&self, request: &Value) -> Option<Value> {
        let id = request.get("id").filter(|id| !id.is_null())?.clone();
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let outcome = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.list() })),
            "tools/call" => self.call(&params),
            other => Err((-32601, format!("method not found: {other}"))),
        };
        Some(match outcome {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message },
            }),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let protocol = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .filter(|version| !version.is_empty())
            .unwrap_or(DEFAULT_PROTOCOL);
        json!({
            "protocolVersion": protocol,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": self.name, "version": self.version },
        })
    }

    fn list(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool
                        .schema
                        .clone()
                        .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
                    "annotations": {
                        "readOnlyHint": tool.read_only,
                        "destructiveHint": tool.destructive && !tool.read_only,
                        "idempotentHint": tool.read_only,
                    },
                })
            })
            .collect()
    }

    fn call(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = match params.get("name") {
            None | Some(Value::Null) => "",
            Some(Value::String(name)) => name.as_str(),
            Some(_) => return Err((-32602, "invalid params".to_string())),
        };
        let args = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(args)) => args.clone(),
            Some(_) => return Err((-32602, "invalid params".to_string())),
        };
        let Some(tool) = self.tools.iter().find(|tool| tool.name == name) else {
            return Ok(text_result(&format!("unknown tool: {name}"), true));
        };
        Ok(match (tool.run)(&args) {
            Ok(text) if tool.max_result_chars > 0 && text.len() > tool.max_result_chars => {
                text_result(&self.cap(tool.name, &text, tool.max_result_chars), false)
            }
            Ok(text) => text_result(&text, false),
            Err(message) => text_result(&message, true),
        })
    }

    /// The head of `text` (cut at a line break when one is near the limit) plus where the rest went.
    fn cap(&self, tool: &str, text: &str, max: usize) -> String {
        let path = self
            .overflow_dir
            .join(format!("{tool}-{:08x}.txt", fnv1a(text.as_bytes())));
        let saved =
            std::fs::create_dir_all(&self.overflow_dir).and_then(|_| std::fs::write(&path, text));
        let mut cut = max;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut head = &text[..cut];
        if let Some(newline) = head.rfind('\n').filter(|at| *at > max / 2) {
            head = &head[..newline];
        }
        let lines = text.matches('\n').count() + 1;
        let summary = format!(
            "{head}\n\n... [truncated: {} of {} chars shown, {lines} lines total]\n",
            head.len(),
            text.len()
        );
        match saved {
            Ok(()) => format!(
                "{summary}Full output saved to {} - read it (Read tool / `cat`) if you need the rest.",
                path.display()
            ),
            Err(error) => format!("{summary}(the full output could not be saved: {error})"),
        }
    }
}

fn text_result(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(2_166_136_261u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(16_777_619)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(dir: PathBuf) -> Server {
        Server {
            name: "pomelo".into(),
            version: "test".into(),
            overflow_dir: dir,
            tools: vec![
                Tool {
                    name: "echo",
                    description: "echo",
                    schema: None,
                    read_only: true,
                    destructive: false,
                    max_result_chars: 0,
                    run: Box::new(|args| {
                        args.get("fail")
                            .map_or(Ok("ok".to_string()), |_| Err("boom".to_string()))
                    }),
                },
                Tool {
                    name: "long",
                    description: "long",
                    schema: None,
                    read_only: false,
                    destructive: true,
                    max_result_chars: 40,
                    run: Box::new(|_| Ok(format!("{}\n{}", "a".repeat(30), "b".repeat(30)))),
                },
            ],
        }
    }

    fn exchange(server: &Server, lines: &[&str]) -> Vec<Value> {
        let mut out = Vec::new();
        server
            .serve(lines.join("\n").as_bytes(), &mut out)
            .expect("serve");
        String::from_utf8(out)
            .expect("utf8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("json line"))
            .collect()
    }

    #[test]
    fn handshake_list_and_errors() {
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path().into());
        let replies = exchange(
            &server,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                "not json",
                "",
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#,
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"fail":true}}}"#,
                r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"ghost"}}"#,
                r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"echo","arguments":[1]}}"#,
                r#"{"jsonrpc":"2.0","id":"s","method":"ping"}"#,
            ],
        );
        assert_eq!(
            replies.len(),
            7,
            "the notification and junk lines get no reply"
        );
        assert_eq!(replies[0]["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(replies[0]["result"]["serverInfo"]["name"], "pomelo");
        let tools = &replies[1]["result"]["tools"];
        assert_eq!(
            tools[0]["inputSchema"],
            json!({"type":"object","properties":{}})
        );
        assert_eq!(
            tools[1]["annotations"],
            json!({"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false})
        );
        assert_eq!(replies[2]["error"]["code"], -32601);
        assert_eq!(replies[3]["result"]["isError"], true);
        assert_eq!(replies[3]["result"]["content"][0]["text"], "boom");
        assert_eq!(
            replies[4]["result"]["content"][0]["text"],
            "unknown tool: ghost"
        );
        assert_eq!(replies[5]["error"]["code"], -32602);
        assert_eq!(replies[6]["id"], "s");
    }

    #[test]
    fn long_results_are_cut_and_saved() {
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path().into());
        let replies = exchange(
            &server,
            &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"long"}}"#],
        );
        let text = replies[0]["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        assert!(
            text.starts_with(&format!("{}\n\n... [truncated: 30 of 61", "a".repeat(30))),
            "{text}"
        );
        let saved = std::fs::read_dir(temp.path())
            .expect("dir")
            .flatten()
            .next()
            .expect("saved file");
        assert!(saved.file_name().to_string_lossy().starts_with("long-"));
        assert_eq!(
            std::fs::read_to_string(saved.path()).expect("read").len(),
            61
        );
    }
}
