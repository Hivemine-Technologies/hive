pub mod claude;

use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::domain::AgentEvent;
use crate::error::Result;

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub working_dir: PathBuf,
    pub system_prompt: String,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub session_id: String,
    pub runner_name: String,
    pub pid: Option<u32>,
}

#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle>;
    async fn send_prompt(&self, session: &SessionHandle, prompt: &str) -> Result<()>;
    fn output_stream(
        &self,
        session: &SessionHandle,
    ) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>>;
    async fn cancel(&self, session: &SessionHandle) -> Result<()>;
    async fn resume(&self, session: &SessionHandle) -> Result<()>;
    async fn is_alive(&self, session: &SessionHandle) -> bool;
    fn name(&self) -> &str;
}
