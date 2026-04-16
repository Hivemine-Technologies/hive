use std::collections::HashMap;

use crate::config::PhaseConfig;
use crate::domain::phase::{next_enabled_phase, Phase};

pub fn advance(current: Phase, phases_config: &HashMap<String, PhaseConfig>) -> Phase {
    if matches!(current, Phase::Queued) {
        let all = Phase::all_in_order();
        for phase in &all {
            let key = phase.config_key();
            let enabled = phases_config.get(key).map(|c| c.enabled).unwrap_or(true);
            if enabled {
                return phase.clone();
            }
        }
        return Phase::Complete;
    }
    match next_enabled_phase(&current, phases_config) {
        Some(phase) => phase,
        None => Phase::Complete,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::PhaseConfig;
    use crate::domain::Phase;

    fn enabled_config() -> PhaseConfig {
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
        }
    }

    fn disabled_config() -> PhaseConfig {
        PhaseConfig {
            enabled: false,
            ..enabled_config()
        }
    }

    #[test]
    fn test_advance_from_queued() {
        let phases = HashMap::new();
        let next = advance(Phase::Queued, &phases);
        assert_eq!(next, Phase::Understand);
    }

    #[test]
    fn test_advance_skips_disabled() {
        let mut phases = HashMap::new();
        phases.insert("cross-review".to_string(), disabled_config());
        let next = advance(Phase::SelfReview { attempt: 0 }, &phases);
        assert_eq!(next, Phase::RaisePr);
    }

    #[test]
    fn test_advance_past_handoff_is_pr_watch() {
        let phases = HashMap::new();
        let next = advance(Phase::Handoff, &phases);
        assert_eq!(next, Phase::PrWatch);
    }

    #[test]
    fn test_advance_all_remaining_disabled() {
        let mut phases = HashMap::new();
        phases.insert("follow-ups".to_string(), disabled_config());
        phases.insert("handoff".to_string(), disabled_config());
        phases.insert("pr-watch".to_string(), disabled_config());
        let next = advance(Phase::BotReviews { cycle: 0 }, &phases);
        assert_eq!(next, Phase::Complete);
    }

    #[test]
    fn test_advance_from_pr_watch_to_complete() {
        let phases = HashMap::new();
        let next = advance(Phase::PrWatch, &phases);
        assert_eq!(next, Phase::Complete);
    }

    #[test]
    fn test_advance_skips_disabled_pr_watch() {
        let mut phases = HashMap::new();
        phases.insert("pr-watch".to_string(), disabled_config());
        let next = advance(Phase::Handoff, &phases);
        assert_eq!(next, Phase::Complete);
    }
}
