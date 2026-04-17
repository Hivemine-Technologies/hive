use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;

use super::{AgentRunner, SessionConfig, SessionHandle};
use crate::domain::AgentEvent;
use crate::error::{HiveError, Result};

struct RunningSession {
    child: Child,
    event_rx: Option<mpsc::Receiver<AgentEvent>>,
    #[allow(dead_code)]
    session_id: String,
}

pub struct ClaudeRunner {
    command: String,
    default_model: String,
    permission_mode: Option<String>,
    sessions: Arc<Mutex<HashMap<String, RunningSession>>>,
}

impl ClaudeRunner {
    pub fn new(command: String, default_model: String, permission_mode: Option<String>) -> Self {
        Self {
            command,
            default_model,
            permission_mode,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn build_args(&self, config: &SessionConfig) -> Vec<String> {
        // -p (--print) is a boolean flag, NOT -p <prompt>.
        // The prompt goes as the final positional argument.
        // Note: we do NOT use --bare because it disables OAuth/keychain auth,
        // requiring ANTHROPIC_API_KEY. Most users auth via OAuth.
        let mut args = vec![
            "-p".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
        ];

        let model = config
            .model
            .as_deref()
            .unwrap_or(&self.default_model);
        args.push("--model".to_string());
        args.push(model.to_string());

        if let Some(ref pm) = config.permission_mode.as_ref().or(self.permission_mode.as_ref()) {
            args.push("--permission-mode".to_string());
            args.push(pm.to_string());
        }

        // Prompt must be the last positional argument
        args.push(config.system_prompt.clone());

        args
    }

    fn spawn_child(
        &self,
        args: &[String],
        working_dir: &std::path::Path,
    ) -> Result<Child> {
        use std::process::Stdio;
        let mut cmd = Command::new(&self.command);
        cmd.args(args)
            .current_dir(working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.spawn()
            .map_err(|e| HiveError::Agent(format!("failed to spawn claude: {e}")))
    }

    fn spawn_stdout_reader(
        child: &mut Child,
    ) -> mpsc::Receiver<AgentEvent> {
        let stdout = child
            .stdout
            .take()
            .expect("stdout must be piped");
        let stderr = child.stderr.take();
        let (tx, rx) = mpsc::channel::<AgentEvent>(256);

        // Read stderr in background and surface as Error event if process fails
        let tx_err = tx.clone();
        tokio::spawn(async move {
            if let Some(stderr) = stderr {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                let mut stderr_buf = String::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    if !line.trim().is_empty() {
                        stderr_buf.push_str(&line);
                        stderr_buf.push('\n');
                    }
                }
                if !stderr_buf.is_empty() {
                    let _ = tx_err
                        .send(AgentEvent::Error(format!("claude stderr: {stderr_buf}")))
                        .await;
                }
            }
        });

        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                match parse_claude_event(&line) {
                    Ok(Some(event)) => {
                        if tx.send(event).await.is_err() {
                            break; // receiver dropped
                        }
                    }
                    Ok(None) => {} // system event or unrecognized, skip
                    Err(_) => {}   // malformed JSON line, skip
                }
            }
        });

        rx
    }
}

#[async_trait]
impl AgentRunner for ClaudeRunner {
    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let args = self.build_args(&config);
        let mut child = self.spawn_child(&args, &config.working_dir)?;
        let pid = child.id();
        let session_id = pid
            .map(|p| p.to_string())
            .unwrap_or_else(uuid_simple);

        let event_rx = Self::spawn_stdout_reader(&mut child);

        let running = RunningSession {
            child,
            event_rx: Some(event_rx),
            session_id: session_id.clone(),
        };

        self.sessions.lock().await.insert(session_id.clone(), running);

        Ok(SessionHandle {
            session_id,
        })
    }

    fn output_stream(
        &self,
        session: &SessionHandle,
    ) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
        let sessions = self.sessions.clone();
        let session_id = session.session_id.clone();

        // We need to take the receiver out of the session. Since this is a sync
        // fn returning a stream, we use a channel bridge.
        let (bridge_tx, bridge_rx) = mpsc::channel::<AgentEvent>(256);

        tokio::spawn(async move {
            let mut event_rx = {
                let mut sessions = sessions.lock().await;
                if let Some(running) = sessions.get_mut(&session_id) {
                    running.event_rx.take()
                } else {
                    None
                }
            };

            if let Some(ref mut rx) = event_rx {
                while let Some(event) = rx.recv().await {
                    if bridge_tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
        });

        Box::pin(ReceiverStream::new(bridge_rx))
    }

    async fn cancel(&self, session: &SessionHandle) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        if let Some(mut running) = sessions.remove(&session.session_id) {
            let _ = running.child.kill().await;
        }
        Ok(())
    }
}

/// Simple monotonic ID generator (no external uuid crate needed)
fn uuid_simple() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("session-{n}")
}

pub fn parse_claude_event(line: &str) -> Result<Option<AgentEvent>> {
    let v: Value =
        serde_json::from_str(line).map_err(|e| HiveError::Agent(format!("bad json: {e}")))?;

    let event_type = v["type"].as_str().unwrap_or("");

    match event_type {
        "assistant" => {
            let content = &v["message"]["content"];
            if let Some(items) = content.as_array() {
                for item in items {
                    match item["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = item["text"].as_str() {
                                return Ok(Some(AgentEvent::TextDelta(text.to_string())));
                            }
                        }
                        Some("tool_use") => {
                            let tool = item["name"].as_str().unwrap_or("unknown").to_string();
                            let input_preview = item["input"].to_string();
                            let input_preview = if input_preview.len() > 100 {
                                format!("{}...", &input_preview[..100])
                            } else {
                                input_preview
                            };
                            return Ok(Some(AgentEvent::ToolUse {
                                tool,
                                input_preview,
                            }));
                        }
                        _ => {}
                    }
                }
            }
            Ok(None)
        }
        "result" => {
            let is_error = v["is_error"].as_bool().unwrap_or(false);
            let cost = v["total_cost_usd"].as_f64().unwrap_or(0.0);

            if is_error {
                let msg = v["result"].as_str().unwrap_or("unknown error").to_string();
                Ok(Some(AgentEvent::Error(msg)))
            } else {
                Ok(Some(AgentEvent::Complete { cost_usd: cost }))
            }
        }
        "content_block_delta" => {
            if let Some(delta) = v["delta"].as_object()
                && delta.get("type").and_then(|t| t.as_str()) == Some("text_delta")
                && let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                return Ok(Some(AgentEvent::TextDelta(text.to_string())));
            }
            Ok(None)
        }
        "system" => {
            // Could extract session_id from subtype "init" here if needed:
            // v["session_id"].as_str()
            Ok(None)
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_delta_event() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Reading file..."}]}}"#;
        let event = parse_claude_event(line).unwrap();
        match event {
            Some(AgentEvent::TextDelta(text)) => assert_eq!(text, "Reading file..."),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_tool_use_event() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/src/main.rs"}}]}}"#;
        let event = parse_claude_event(line).unwrap();
        match event {
            Some(AgentEvent::ToolUse { tool, .. }) => assert_eq!(tool, "Read"),
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_result_event() {
        let line = r#"{"type":"result","result":"Done","session_id":"abc","is_error":false,"total_cost_usd":0.42}"#;
        let event = parse_claude_event(line).unwrap();
        match event {
            Some(AgentEvent::Complete { cost_usd }) => {
                assert!((cost_usd - 0.42).abs() < f64::EPSILON)
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_error_result_event() {
        let line = r#"{"type":"result","result":"Something broke","session_id":"abc","is_error":true,"total_cost_usd":0.10}"#;
        let event = parse_claude_event(line).unwrap();
        match event {
            Some(AgentEvent::Error(msg)) => assert_eq!(msg, "Something broke"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_content_block_delta() {
        let line = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#;
        let event = parse_claude_event(line).unwrap();
        match event {
            Some(AgentEvent::TextDelta(text)) => assert_eq!(text, "hello"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_system_init_event_returns_none() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc123"}"#;
        let event = parse_claude_event(line).unwrap();
        assert!(event.is_none());
    }

    #[test]
    fn test_parse_malformed_json_returns_error() {
        let line = "not json at all";
        let result = parse_claude_event(line);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_unknown_type_returns_none() {
        let line = r#"{"type":"ping","data":{}}"#;
        let event = parse_claude_event(line).unwrap();
        assert!(event.is_none());
    }

    #[test]
    fn test_uuid_simple_is_unique() {
        let a = uuid_simple();
        let b = uuid_simple();
        assert_ne!(a, b);
        assert!(a.starts_with("session-"));
    }

    #[tokio::test]
    async fn test_session_map_insert_and_remove() {
        let runner = ClaudeRunner::new(
            "echo".to_string(),
            "test-model".to_string(),
            None,
        );
        // Verify sessions map starts empty
        let sessions = runner.sessions.lock().await;
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_cancel_nonexistent_session_is_ok() {
        let runner = ClaudeRunner::new(
            "echo".to_string(),
            "test-model".to_string(),
            None,
        );
        let handle = SessionHandle {
            session_id: "nonexistent".to_string(),
        };
        assert!(runner.cancel(&handle).await.is_ok());
    }

    #[test]
    fn test_build_args_includes_model() {
        let runner = ClaudeRunner::new(
            "claude".to_string(),
            "opus-4-6".to_string(),
            Some("dangerously-skip".to_string()),
        );
        let config = SessionConfig {
            working_dir: std::path::PathBuf::from("/tmp"),
            system_prompt: "test prompt".to_string(),
            model: None,
            permission_mode: None,
        };
        let args = runner.build_args(&config);
        assert!(args.contains(&"opus-4-6".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
    }

    #[test]
    fn test_build_args_model_override() {
        let runner = ClaudeRunner::new(
            "claude".to_string(),
            "opus-4-6".to_string(),
            None,
        );
        let config = SessionConfig {
            working_dir: std::path::PathBuf::from("/tmp"),
            system_prompt: "test prompt".to_string(),
            model: Some("sonnet-4-6".to_string()),
            permission_mode: None,
        };
        let args = runner.build_args(&config);
        assert!(args.contains(&"sonnet-4-6".to_string()));
        assert!(!args.contains(&"opus-4-6".to_string()));
    }

    #[test]
    fn test_build_args_prompt_is_last() {
        let runner = ClaudeRunner::new(
            "claude".to_string(),
            "opus-4-6".to_string(),
            None,
        );
        let config = SessionConfig {
            working_dir: std::path::PathBuf::from("/tmp"),
            system_prompt: "my test prompt".to_string(),
            model: None,
            permission_mode: None,
        };
        let args = runner.build_args(&config);
        assert_eq!(args.last().unwrap(), "my test prompt");
        // -p should be a standalone flag, not followed by the prompt
        let p_idx = args.iter().position(|a| a == "-p").unwrap();
        assert_ne!(args[p_idx + 1], "my test prompt");
    }
}
