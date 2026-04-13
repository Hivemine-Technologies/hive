use crate::domain::Phase;

/// Build the system prompt for an agent phase.
///
/// Each agent phase gets a tailored prompt that references the issue
/// details and guides the agent's behavior for that specific phase.
pub fn build_phase_prompt(
    phase: &Phase,
    issue_id: &str,
    issue_title: &str,
    issue_description: &str,
) -> String {
    match phase {
        Phase::Understand => format!(
            "You are analyzing story {issue_id}: {issue_title}.\n\n\
             Issue description:\n{issue_description}\n\n\
             Read the issue description and acceptance criteria. Explore the codebase to \
             understand what needs to change. Write a brief plan as a markdown file in the \
             worktree root (PLAN.md). Do not implement yet."
        ),
        Phase::Implement => format!(
            "You are implementing story {issue_id}: {issue_title}.\n\n\
             Issue description:\n{issue_description}\n\n\
             Follow the plan in PLAN.md. Write code, tests, and commit your work. \
             Use conventional commit messages prefixed with the issue ID."
        ),
        Phase::SelfReview { .. } => format!(
            "You are reviewing your own implementation of story {issue_id}: {issue_title}.\n\n\
             Read the diff of all changes. Check for bugs, missing edge cases, test coverage \
             gaps, and code quality issues. Fix anything you find and commit the fixes."
        ),
        Phase::CrossReview => format!(
            "You are cross-reviewing the implementation of story {issue_id}: {issue_title}.\n\n\
             Read all changes critically. Report issues but do not fix them -- create a \
             REVIEW.md with findings."
        ),
        Phase::FollowUps => format!(
            "Story {issue_id}: {issue_title} is complete.\n\n\
             Review the implementation and identify any follow-up work needed (tech debt, \
             documentation, related changes). Create follow-up issues via the provided tool."
        ),
        _ => format!(
            "Working on story {issue_id}: {issue_title}.\n\n{issue_description}"
        ),
    }
}

/// Build a recovery prompt for retrying a failed phase.
pub fn build_retry_prompt(
    phase: &Phase,
    issue_id: &str,
    issue_title: &str,
    failure_reason: &str,
    attempt: u8,
) -> String {
    format!(
        "You are retrying {phase} for story {issue_id}: {issue_title} (attempt {attempt}).\n\n\
         Previous attempt failed: {failure_reason}\n\n\
         Review the current state of the worktree and try again."
    )
}

/// Build a prompt for CI fix agents.
pub fn build_ci_fix_prompt(
    issue_id: &str,
    failures: &[String],
) -> String {
    let failure_text = failures.join("\n- ");
    format!(
        "CI failed for story {issue_id}. Fix the issues and commit.\n\n\
         Failures:\n- {failure_text}"
    )
}

/// Build a prompt for addressing bot review comments.
pub fn build_bot_review_fix_prompt(
    issue_id: &str,
    comments: &[String],
) -> String {
    let comment_text = comments.join("\n---\n");
    format!(
        "Address these review comments for story {issue_id}:\n\n{comment_text}"
    )
}

/// Build a prompt for crash recovery (resuming interrupted work).
pub fn build_resume_prompt(
    phase: &Phase,
    issue_id: &str,
    issue_title: &str,
) -> String {
    format!(
        "You are resuming work on story {issue_id}: {issue_title}.\n\n\
         Review the current state of the worktree and continue from where you left off. \
         The previous phase was {phase}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Phase;

    #[test]
    fn test_understand_prompt_contains_issue_id() {
        let prompt = build_phase_prompt(
            &Phase::Understand,
            "APX-245",
            "Add NumberSequence",
            "Create the service",
        );
        assert!(prompt.contains("APX-245"));
        assert!(prompt.contains("Add NumberSequence"));
        assert!(prompt.contains("PLAN.md"));
        assert!(prompt.contains("Do not implement yet"));
    }

    #[test]
    fn test_implement_prompt_references_plan() {
        let prompt = build_phase_prompt(
            &Phase::Implement,
            "APX-245",
            "Add NumberSequence",
            "Create the service",
        );
        assert!(prompt.contains("PLAN.md"));
        assert!(prompt.contains("commit"));
    }

    #[test]
    fn test_self_review_prompt() {
        let prompt = build_phase_prompt(
            &Phase::SelfReview { attempt: 0 },
            "APX-245",
            "Add NumberSequence",
            "Create the service",
        );
        assert!(prompt.contains("reviewing your own"));
        assert!(prompt.contains("diff"));
    }

    #[test]
    fn test_cross_review_prompt() {
        let prompt = build_phase_prompt(
            &Phase::CrossReview,
            "APX-245",
            "Add NumberSequence",
            "",
        );
        assert!(prompt.contains("cross-reviewing"));
        assert!(prompt.contains("REVIEW.md"));
    }

    #[test]
    fn test_follow_ups_prompt() {
        let prompt = build_phase_prompt(
            &Phase::FollowUps,
            "APX-245",
            "Add NumberSequence",
            "",
        );
        assert!(prompt.contains("follow-up"));
    }

    #[test]
    fn test_retry_prompt_includes_failure_reason() {
        let prompt = build_retry_prompt(
            &Phase::Implement,
            "APX-245",
            "Add NumberSequence",
            "compilation error",
            2,
        );
        assert!(prompt.contains("compilation error"));
        assert!(prompt.contains("attempt 2"));
    }

    #[test]
    fn test_ci_fix_prompt() {
        let prompt = build_ci_fix_prompt(
            "APX-245",
            &["lint: failure".to_string(), "test: 3 failed".to_string()],
        );
        assert!(prompt.contains("lint: failure"));
        assert!(prompt.contains("test: 3 failed"));
    }

    #[test]
    fn test_bot_review_fix_prompt() {
        let prompt = build_bot_review_fix_prompt(
            "APX-245",
            &["Consider using Option here".to_string()],
        );
        assert!(prompt.contains("Consider using Option"));
    }

    #[test]
    fn test_resume_prompt() {
        let prompt = build_resume_prompt(
            &Phase::Implement,
            "APX-245",
            "Add NumberSequence",
        );
        assert!(prompt.contains("resuming"));
        assert!(prompt.contains("Implement"));
    }
}
