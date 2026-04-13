use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{AgentRunner, SessionConfig, SessionHandle};
use crate::domain::AgentEvent;
use crate::error::{HiveError, Result};

pub struct GeminiRunner {
    command: String,
    default_model: String,
}

impl GeminiRunner {
    pub fn new(command: String, default_model: String) -> Self {
        Self {
            command,
            default_model,
        }
    }
}

#[async_trait]
impl AgentRunner for GeminiRunner {
    async fn start_session(&self, _config: SessionConfig) -> Result<SessionHandle> {
        Err(HiveError::Agent(
            "Gemini runner not yet implemented".into(),
        ))
    }

    async fn send_prompt(&self, _session: &SessionHandle, _prompt: &str) -> Result<()> {
        Err(HiveError::Agent(
            "Gemini runner not yet implemented".into(),
        ))
    }

    fn output_stream(
        &self,
        _session: &SessionHandle,
    ) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
        let (_tx, rx) = mpsc::channel(1);
        Box::pin(ReceiverStream::new(rx))
    }

    async fn cancel(&self, _session: &SessionHandle) -> Result<()> {
        Ok(())
    }

    async fn resume(&self, _session: &SessionHandle) -> Result<()> {
        Ok(())
    }

    async fn is_alive(&self, _session: &SessionHandle) -> bool {
        false
    }

    fn name(&self) -> &str {
        "gemini"
    }
}
