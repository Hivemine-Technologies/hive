use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::config::PhaseConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Phase {
    Queued,
    Understand,
    Implement,
    SelfReview { attempt: u8 },
    CrossReview,
    RaisePr,
    CiWatch { attempt: u8 },
    BotReviews { cycle: u8 },
    FollowUps,
    Handoff,
    Complete,
    NeedsAttention { reason: String },
}

const PIPELINE_PHASES: &[fn() -> Phase] = &[
    || Phase::Understand,
    || Phase::Implement,
    || Phase::SelfReview { attempt: 0 },
    || Phase::CrossReview,
    || Phase::RaisePr,
    || Phase::CiWatch { attempt: 0 },
    || Phase::BotReviews { cycle: 0 },
    || Phase::FollowUps,
    || Phase::Handoff,
];

impl Phase {
    pub fn all_in_order() -> Vec<Phase> {
        PIPELINE_PHASES.iter().map(|f| f()).collect()
    }

    pub fn config_key(&self) -> &'static str {
        match self {
            Phase::Queued => "queued",
            Phase::Understand => "understand",
            Phase::Implement => "implement",
            Phase::SelfReview { .. } => "self-review",
            Phase::CrossReview => "cross-review",
            Phase::RaisePr => "raise-pr",
            Phase::CiWatch { .. } => "ci-watch",
            Phase::BotReviews { .. } => "bot-reviews",
            Phase::FollowUps => "follow-ups",
            Phase::Handoff => "handoff",
            Phase::Complete => "complete",
            Phase::NeedsAttention { .. } => "needs-attention",
        }
    }

    pub fn is_agent_phase(&self) -> bool {
        matches!(
            self,
            Phase::Understand
                | Phase::Implement
                | Phase::SelfReview { .. }
                | Phase::CrossReview
                | Phase::FollowUps
        )
    }

    pub fn is_polling_phase(&self) -> bool {
        matches!(self, Phase::CiWatch { .. } | Phase::BotReviews { .. })
    }

    pub fn is_direct_phase(&self) -> bool {
        matches!(self, Phase::RaisePr | Phase::Handoff)
    }

    pub fn pipeline_index(&self) -> Option<usize> {
        let all = Self::all_in_order();
        all.iter().position(|p| p.config_key() == self.config_key())
    }

}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Phase::Queued => write!(f, "Queued"),
            Phase::Understand => write!(f, "Understand"),
            Phase::Implement => write!(f, "Implement"),
            Phase::SelfReview { attempt } => write!(f, "Self-Review (attempt {})", attempt),
            Phase::CrossReview => write!(f, "Cross-Review"),
            Phase::RaisePr => write!(f, "Raise PR"),
            Phase::CiWatch { attempt } => write!(f, "CI Watch (attempt {})", attempt),
            Phase::BotReviews { cycle } => write!(f, "Bot Reviews (cycle {})", cycle),
            Phase::FollowUps => write!(f, "Follow-Ups"),
            Phase::Handoff => write!(f, "Handoff"),
            Phase::Complete => write!(f, "Complete"),
            Phase::NeedsAttention { reason } => write!(f, "Needs Attention: {}", reason),
        }
    }
}

pub fn next_enabled_phase(
    current: &Phase,
    phases_config: &HashMap<String, PhaseConfig>,
) -> Option<Phase> {
    let all = Phase::all_in_order();
    let current_idx = all
        .iter()
        .position(|p| p.config_key() == current.config_key())?;

    for candidate in &all[current_idx + 1..] {
        let key = candidate.config_key();
        let enabled = phases_config
            .get(key)
            .map(|c| c.enabled)
            .unwrap_or(true);
        if enabled {
            return Some(candidate.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::PhaseConfig;

    #[test]
    fn test_all_phases_in_order() {
        let phases = Phase::all_in_order();
        assert_eq!(phases[0], Phase::Understand);
        assert_eq!(phases[1], Phase::Implement);
        assert_eq!(phases[2], Phase::SelfReview { attempt: 0 });
        assert_eq!(phases[3], Phase::CrossReview);
        assert_eq!(phases[4], Phase::RaisePr);
        assert_eq!(phases[5], Phase::CiWatch { attempt: 0 });
        assert_eq!(phases[6], Phase::BotReviews { cycle: 0 });
        assert_eq!(phases[7], Phase::FollowUps);
        assert_eq!(phases[8], Phase::Handoff);
    }

    #[test]
    fn test_next_phase_skips_disabled() {
        let mut phases_config = HashMap::new();
        phases_config.insert(
            "understand".to_string(),
            PhaseConfig {
                enabled: true,
                runner: None,
                model: None,
                max_attempts: None,
                poll_interval: None,
                max_fix_attempts: None,
                max_fix_cycles: None,
                fix_runner: None,
                fix_model: None,
                wait_for: None,
            },
        );
        phases_config.insert(
            "implement".to_string(),
            PhaseConfig {
                enabled: true,
                runner: None,
                model: None,
                max_attempts: None,
                poll_interval: None,
                max_fix_attempts: None,
                max_fix_cycles: None,
                fix_runner: None,
                fix_model: None,
                wait_for: None,
            },
        );
        phases_config.insert(
            "self-review".to_string(),
            PhaseConfig {
                enabled: true,
                runner: None,
                model: None,
                max_attempts: None,
                poll_interval: None,
                max_fix_attempts: None,
                max_fix_cycles: None,
                fix_runner: None,
                fix_model: None,
                wait_for: None,
            },
        );
        phases_config.insert(
            "cross-review".to_string(),
            PhaseConfig {
                enabled: false,
                runner: None,
                model: None,
                max_attempts: None,
                poll_interval: None,
                max_fix_attempts: None,
                max_fix_cycles: None,
                fix_runner: None,
                fix_model: None,
                wait_for: None,
            },
        );
        phases_config.insert(
            "raise-pr".to_string(),
            PhaseConfig {
                enabled: true,
                runner: None,
                model: None,
                max_attempts: None,
                poll_interval: None,
                max_fix_attempts: None,
                max_fix_cycles: None,
                fix_runner: None,
                fix_model: None,
                wait_for: None,
            },
        );
        let next = next_enabled_phase(&Phase::SelfReview { attempt: 0 }, &phases_config);
        assert_eq!(next, Some(Phase::RaisePr));
    }

    #[test]
    fn test_next_phase_after_handoff_is_complete() {
        let phases_config = HashMap::new();
        let next = next_enabled_phase(&Phase::Handoff, &phases_config);
        assert_eq!(next, None);
    }

    #[test]
    fn test_phase_config_key() {
        assert_eq!(Phase::Understand.config_key(), "understand");
        assert_eq!(Phase::SelfReview { attempt: 2 }.config_key(), "self-review");
        assert_eq!(Phase::CiWatch { attempt: 1 }.config_key(), "ci-watch");
        assert_eq!(Phase::BotReviews { cycle: 3 }.config_key(), "bot-reviews");
    }
}
