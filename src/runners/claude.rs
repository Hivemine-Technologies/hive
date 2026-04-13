use std::pin::Pin;
use std::process::Stdio;

use async_trait::async_trait;
use futures::Stream;
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{AgentRunner, SessionConfig, SessionHandle};
use crate::domain::AgentEvent;
use crate::error::{HiveError, Result};

pub struct ClaudeRunner {
    command: String,
    default_model: String,
    permission_mode: Option<String>,
}

impl ClaudeRunner {
    pub fn new(command: String, default_model: String, permission_mode: Option<String>) -> Self {
        Self {
            command,
            default_model,
            permission_mode,
        }
    }

    fn build_command(&self, config: &SessionConfig) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.arg("--bare")
            .arg("-p")
            .arg(&config.system_prompt)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose");

        let model = config.model.as_deref().unwrap_or(&self.default_model);
        cmd.arg("--model").arg(model);

        if let Some(ref pm) = config.permission_mode.as_ref().or(self.permission_mode.as_ref()) {
            cmd.arg("--permission-mode").arg(pm);
        }

        cmd.current_dir(&config.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        cmd
    }
}

#[async_trait]
impl AgentRunner for ClaudeRunner {
    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let child = self
            .build_command(&config)
            .spawn()
            .map_err(|e| HiveError::Agent(format!("failed to spawn claude: {e}")))?;

        let pid = child.id();
        let session_id = pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Child process handle will be managed by orchestrator in later tasks
        Ok(SessionHandle {
            session_id,
            runner_name: "claude".to_string(),
            pid,
        })
    }

    async fn send_prompt(&self, _session: &SessionHandle, _prompt: &str) -> Result<()> {
        Ok(()) // Will use --resume in orchestrator
    }

    fn output_stream(
        &self,
        _session: &SessionHandle,
    ) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
        let (_tx, rx) = mpsc::channel(1);
        Box::pin(ReceiverStream::new(rx))
    }

    async fn cancel(&self, session: &SessionHandle) -> Result<()> {
        if let Some(pid) = session.pid {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        Ok(())
    }

    async fn resume(&self, _session: &SessionHandle) -> Result<()> {
        Ok(())
    }

    async fn is_alive(&self, session: &SessionHandle) -> bool {
        if let Some(pid) = session.pid {
            unsafe { libc::kill(pid as i32, 0) == 0 }
        } else {
            false
        }
    }

    fn name(&self) -> &str {
        "claude"
    }
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
        "system" => Ok(None),
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
}
