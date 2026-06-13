use anyhow::Result;
use std::io::{self, BufRead, Write};

pub async fn run_server() -> Result<()> {
    let stdin = io::stdin();
    let mut out = io::BufWriter::new(io::stdout().lock());
    let cwd = std::env::current_dir()?;

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let id = match request.get("id").cloned() {
            Some(id) => id,
            None => continue, // notifications have no id
        };
        let method = request
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        let result = match method {
            "initialize" => serde_json::json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": { "name": "heldout", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "tools": {} }
            }),

            "tools/list" => serde_json::json!({
                "tools": [
                    {
                        "name": "integrity_status",
                        "description": "Get the active heldout session — task, agent, base commit, and start time.",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "integrity_check",
                        "description": "Run integrity checks on the current git working tree. Returns verdict and findings.",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "integrity_start",
                        "description": "Start a new heldout session for an agent task.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "task": { "type": "string", "description": "Task description" },
                                "agent": { "type": "string", "description": "Agent name (optional)" }
                            },
                            "required": ["task"]
                        }
                    }
                ]
            }),

            "tools/call" => {
                let tool = request
                    .pointer("/params/name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let args = request
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_default();

                let text = match tool {
                    "integrity_status" => match crate::session::load(&cwd) {
                        Ok(Some(s)) => format!(
                            "task: {}\nagent: {}\nbase: {}\nstarted: {}",
                            s.mission,
                            s.agent.as_deref().unwrap_or("unknown"),
                            &s.git_baseline[..7.min(s.git_baseline.len())],
                            s.started_at
                        ),
                        Ok(None) => "No active session. Run: heldout start \"<task>\"".to_string(),
                        Err(e) => format!("error: {e}"),
                    },

                    "integrity_check" => {
                        let config = crate::config::load(&cwd).unwrap_or_default();
                        let session = crate::session::load(&cwd).ok().flatten();
                        match crate::report::run_check(
                            &cwd,
                            &config,
                            session.as_ref(),
                            false,
                            false,
                        ) {
                            Ok(report) => serde_json::to_string_pretty(&report)
                                .unwrap_or_else(|_| "serialize error".to_string()),
                            Err(e) => format!("error: {e}"),
                        }
                    }

                    "integrity_start" => {
                        let task = args
                            .get("task")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let agent = args
                            .get("agent")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        if task.is_empty() {
                            "error: 'task' is required".to_string()
                        } else {
                            match crate::session::start(&cwd, task.clone(), agent, vec![]) {
                                Ok(s) => format!(
                                    "started: {}\nbase: {}",
                                    s.mission,
                                    &s.git_baseline[..7.min(s.git_baseline.len())]
                                ),
                                Err(e) => format!("error: {e}"),
                            }
                        }
                    }

                    other => format!("unknown tool: {other}"),
                };

                serde_json::json!({
                    "content": [{ "type": "text", "text": text }]
                })
            }

            _ => serde_json::json!({ "error": { "code": -32601, "message": "Method not found" } }),
        };

        writeln!(
            out,
            "{}",
            serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
        )?;
        out.flush()?;
    }

    Ok(())
}
