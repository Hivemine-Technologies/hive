use async_trait::async_trait;
use serde_json::Value;

use super::Notifier;
use crate::domain::NotifyEvent;
use crate::error::{HiveError, Result};

const COLOR_GREEN: u32 = 3_066_993;
const COLOR_YELLOW: u32 = 15_844_367;
const COLOR_RED: u32 = 15_158_332;

pub struct DiscordNotifier {
    webhook_url: String,
    client: reqwest::Client,
}

impl DiscordNotifier {
    pub fn new(webhook_url: String) -> Self {
        Self {
            webhook_url,
            client: reqwest::Client::new(),
        }
    }

    pub fn format_message(&self, event: &NotifyEvent) -> Value {
        match event {
            NotifyEvent::StoryComplete {
                issue_id,
                pr_url,
                cost_usd,
                duration_secs,
            } => {
                let minutes = duration_secs / 60;
                serde_json::json!({
                    "embeds": [{
                        "title": format!("{issue_id} - PR Ready"),
                        "description": format!(
                            "Pull request is ready for review.\n\n**PR:** {pr_url}\n**Cost:** ${cost_usd:.2}\n**Duration:** {minutes}m"
                        ),
                        "color": COLOR_GREEN,
                    }]
                })
            }
            NotifyEvent::NeedsAttention { issue_id, reason } => {
                serde_json::json!({
                    "embeds": [{
                        "title": format!("{issue_id} - Needs Attention"),
                        "description": reason,
                        "color": COLOR_YELLOW,
                    }]
                })
            }
            NotifyEvent::CiFailedMaxRetries { issue_id } => {
                serde_json::json!({
                    "embeds": [{
                        "title": format!("{issue_id} - CI Failed"),
                        "description": "CI fix attempts exhausted. Manual intervention required.",
                        "color": COLOR_RED,
                    }]
                })
            }
        }
    }
}

#[async_trait]
impl Notifier for DiscordNotifier {
    async fn notify(&self, event: NotifyEvent) -> Result<()> {
        let body = self.format_message(&event);
        let resp = self
            .client
            .post(&self.webhook_url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(HiveError::Notification(format!(
                "Discord webhook error ({status}): {text}"
            )));
        }
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_complete_message() {
        let notifier = DiscordNotifier::new("https://example.com".to_string());
        let event = NotifyEvent::StoryComplete {
            issue_id: "APX-245".to_string(),
            pr_url: "https://github.com/hivemine/gemini-chatz/pull/847".to_string(),
            cost_usd: 1.42,
            duration_secs: 1800,
        };
        let msg = notifier.format_message(&event);
        let title = msg["embeds"][0]["title"].as_str().unwrap();
        assert!(title.contains("APX-245"));
        assert!(title.contains("PR Ready"));
    }

    #[test]
    fn test_format_needs_attention_message() {
        let notifier = DiscordNotifier::new("https://example.com".to_string());
        let event = NotifyEvent::NeedsAttention {
            issue_id: "APX-282".to_string(),
            reason: "Bot review fix attempts exhausted".to_string(),
        };
        let msg = notifier.format_message(&event);
        let desc = msg["embeds"][0]["description"].as_str().unwrap();
        assert!(desc.contains("Bot review fix attempts exhausted"));
    }

    #[test]
    fn test_format_ci_failed_message() {
        let notifier = DiscordNotifier::new("https://example.com".to_string());
        let event = NotifyEvent::CiFailedMaxRetries {
            issue_id: "APX-100".to_string(),
        };
        let msg = notifier.format_message(&event);
        let title = msg["embeds"][0]["title"].as_str().unwrap();
        assert!(title.contains("APX-100"));
        assert!(title.contains("CI Failed"));
        let color = msg["embeds"][0]["color"].as_u64().unwrap();
        assert_eq!(color, COLOR_RED as u64);
    }

    #[test]
    fn test_complete_message_includes_cost_and_duration() {
        let notifier = DiscordNotifier::new("https://example.com".to_string());
        let event = NotifyEvent::StoryComplete {
            issue_id: "APX-10".to_string(),
            pr_url: "https://github.com/test/repo/pull/1".to_string(),
            cost_usd: 3.50,
            duration_secs: 3600,
        };
        let msg = notifier.format_message(&event);
        let desc = msg["embeds"][0]["description"].as_str().unwrap();
        assert!(desc.contains("$3.50"));
        assert!(desc.contains("60m"));
    }
}
