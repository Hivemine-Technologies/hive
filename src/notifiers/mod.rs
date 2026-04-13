pub mod discord;

use async_trait::async_trait;

use crate::domain::NotifyEvent;
use crate::error::Result;

#[async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, event: NotifyEvent) -> Result<()>;
    fn name(&self) -> &str;
}
