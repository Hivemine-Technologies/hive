use async_trait::async_trait;

use super::Notifier;
use crate::domain::NotifyEvent;
use crate::error::{HiveError, Result};

pub struct SlackNotifier {
    webhook_url: String,
}

impl SlackNotifier {
    pub fn new(webhook_url: String) -> Self {
        Self { webhook_url }
    }
}

#[async_trait]
impl Notifier for SlackNotifier {
    async fn notify(&self, _event: NotifyEvent) -> Result<()> {
        Err(HiveError::Notification(
            "Slack notifier not yet implemented".into(),
        ))
    }

    fn name(&self) -> &str {
        "slack"
    }
}
