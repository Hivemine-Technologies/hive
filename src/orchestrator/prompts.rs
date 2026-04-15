use crate::domain::Phase;

/// Common preamble instructing the agent to read project conventions.
const CONVENTIONS_PREAMBLE: &str = "\
Read CLAUDE.md at the repo root before doing anything else. If it references \
additional convention files (e.g. internal/CLAUDE.md, ui/CLAUDE.md), read those \
too. These define the tech stack, conventions, test commands, and commit patterns. \
Follow them as your source of truth — do NOT assume any stack or convention not \
stated there.";

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
             {CONVENTIONS_PREAMBLE}\n\n\
             Issue description:\n{issue_description}\n\n\
             1. Read the issue description and acceptance criteria carefully\n\
             2. Explore the codebase to understand what needs to change — identify affected \
             files, modules, and dependencies\n\
             3. Write a brief implementation plan as PLAN.md in the worktree root\n\
             4. Do NOT implement yet — planning only"
        ),
        Phase::Implement => format!(
            "You are implementing story {issue_id}: {issue_title}.\n\n\
             {CONVENTIONS_PREAMBLE}\n\n\
             Issue description:\n{issue_description}\n\n\
             1. Follow the plan in PLAN.md\n\
             2. Write tests alongside implementation using the project's test framework \
             and patterns\n\
             3. Use conventional commits: `feat({issue_id}): description` or \
             `fix({issue_id}): description`\n\
             4. One logical change per commit\n\
             5. Before committing, verify each changed file against the Pre-Commit Quality \
             Checklist in CLAUDE.md (if one exists). Fix any violations before creating \
             the commit.\n\
             6. Run the project's verification command (as defined in CLAUDE.md) — fix all \
             failures before finishing"
        ),
        Phase::SelfReview { .. } => format!(
            "You are reviewing your own implementation of story {issue_id}: {issue_title}.\n\n\
             {CONVENTIONS_PREAMBLE}\n\n\
             1. Run `git diff master...HEAD` to see all changes\n\
             2. Review the diff against CLAUDE.md conventions. Check for:\n\
                - Logic errors and nil/null risks\n\
                - Convention violations\n\
                - Missing tests or inadequate test coverage\n\
                - Security issues (injection, leaks, unsafe patterns)\n\
                - Missing edge case handling\n\
             3. If issues found: fix them, commit as `fix({issue_id}): address self-review - \
             <what>`, and run the project's verification command\n\
             4. If clean, report that the review passed with no issues"
        ),
        Phase::CrossReview => format!(
            "You are an independent reviewer examining the implementation of story \
             {issue_id}: {issue_title}.\n\n\
             {CONVENTIONS_PREAMBLE}\n\n\
             You did NOT write this code. Review it critically as a second pair of eyes.\n\n\
             1. Run `git diff master...HEAD` to see all changes\n\
             2. Check for: correctness, convention violations, missing tests, security \
             issues, performance concerns, and unclear code\n\
             3. Report issues but do NOT fix them — create a REVIEW.md with findings \
             categorized by severity (must-fix vs suggestion)\n\
             4. If the code is clean, write REVIEW.md with \"LGTM\" and any minor observations"
        ),
        Phase::FollowUps => format!(
            "Story {issue_id}: {issue_title} is complete.\n\n\
             Scan all commit messages, code comments, and the implementation for follow-up \
             work that was deferred. Look for phrases like \"TODO\", \"follow-up\", \
             \"out of scope\", \"will handle later\", \"in a future PR\".\n\n\
             For each follow-up found, draft an issue with:\n\
             - Title: clear description of the follow-up work\n\
             - Description: context on why it was deferred, link to source story {issue_id}\n\n\
             Before saving each follow-up issue, validate against these quality gates:\n\
             **Hard Gates (all must pass):**\n\
             - H1 Clear Objective: states a concrete outcome — what changes and why\n\
             - H2 Acceptance Criteria: at least 2 specific, testable conditions for \"done\"\n\
             - H3 Affected Areas: names at least one specific package, module, or component\n\
             - H4 Deployable Scope: describes a single deployable unit of work\n\n\
             If any hard gate fails, enrich the story from codebase context until it passes.\n\n\
             Save each validated follow-up issue via the issue tracker."
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
        "CI failed for story {issue_id}. Your job is to get CI green.\n\n\
         {CONVENTIONS_PREAMBLE}\n\n\
         ## Failing Checks\n\
         - {failure_text}\n\n\
         ## Approach\n\
         1. Diagnose each failure by type:\n\
            - **Build error**: compilation failure, missing import, type error\n\
            - **Test failure**: assertion failed, timeout, flaky test\n\
            - **Lint error**: formatting, static analysis violation\n\
            - **Infrastructure**: permissions, config issues (note but don't fix)\n\
         2. Triage: group related failures, identify root causes. A single root cause \
         (e.g. a type change) often cascades into multiple failures — fix the root first.\n\
         3. Fix each root cause, then run the project's verification command locally \
         to confirm the fix works before committing.\n\
         4. Commit fixes with descriptive messages: `fix({issue_id}): <what was broken and why>`\n\
         5. One commit per logical fix — do NOT bundle unrelated fixes."
    )
}

/// Build a prompt for addressing bot review comments.
pub fn build_bot_review_fix_prompt(
    issue_id: &str,
    issue_title: &str,
    comments: &[String],
) -> String {
    let comment_text = comments.join("\n---\n");
    format!(
        "You are working on story {issue_id}: {issue_title}.\n\n\
         {CONVENTIONS_PREAMBLE}\n\n\
         Bot reviewers have left feedback on the pull request. CI is passing — \
         these are code quality and style suggestions, not build failures.\n\n\
         ## Approach\n\
         1. Categorize each comment by severity: must-fix (correctness, nil-safety, \
         resource leaks) vs suggestion (style, complexity, naming)\n\
         2. Fix must-fix issues first, then address suggestions that improve the code\n\
         3. If a suggestion is incorrect or not applicable, skip it\n\
         4. Run the project's verification command after fixes to avoid regressions\n\
         5. Commit fixes: `fix({issue_id}): address review - <what changed>`\n\n\
         ## Review Comments\n\n{comment_text}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Phase;

    #[test]
    fn test_agent_phase_prompts_include_conventions_preamble() {
        for (phase, desc) in [
            (Phase::Understand, "Create the service"),
            (Phase::Implement, "Create the service"),
            (Phase::SelfReview { attempt: 0 }, ""),
            (Phase::CrossReview, ""),
        ] {
            let prompt = build_phase_prompt(&phase, "X-1", "Title", desc);
            assert!(
                prompt.contains("CLAUDE.md"),
                "{phase} prompt missing CLAUDE.md reference"
            );
        }
    }

    #[test]
    fn test_understand_prompt() {
        let prompt = build_phase_prompt(
            &Phase::Understand,
            "APX-245",
            "Add NumberSequence",
            "Create the service",
        );
        assert!(prompt.contains("APX-245"));
        assert!(prompt.contains("Add NumberSequence"));
        assert!(prompt.contains("PLAN.md"));
        assert!(prompt.contains("Do NOT implement yet"));
    }

    #[test]
    fn test_implement_prompt() {
        let prompt = build_phase_prompt(
            &Phase::Implement,
            "APX-245",
            "Add NumberSequence",
            "Create the service",
        );
        assert!(prompt.contains("PLAN.md"));
        assert!(prompt.contains("feat(APX-245)"));
        assert!(prompt.contains("verification command"));
    }

    #[test]
    fn test_self_review_prompt() {
        let prompt = build_phase_prompt(
            &Phase::SelfReview { attempt: 0 },
            "APX-245",
            "Add NumberSequence",
            "",
        );
        assert!(prompt.contains("reviewing your own"));
        assert!(prompt.contains("git diff master...HEAD"));
        assert!(prompt.contains("Logic errors"));
        assert!(prompt.contains("Security issues"));
    }

    #[test]
    fn test_cross_review_prompt() {
        let prompt = build_phase_prompt(
            &Phase::CrossReview,
            "APX-245",
            "Add NumberSequence",
            "",
        );
        assert!(prompt.contains("independent reviewer"));
        assert!(prompt.contains("did NOT write this code"));
        assert!(prompt.contains("REVIEW.md"));
    }

    #[test]
    fn test_follow_ups_prompt_has_quality_gates() {
        let prompt = build_phase_prompt(
            &Phase::FollowUps,
            "APX-245",
            "Add NumberSequence",
            "",
        );
        assert!(prompt.contains("follow-up"));
        assert!(prompt.contains("H1 Clear Objective"));
        assert!(prompt.contains("H2 Acceptance Criteria"));
        assert!(prompt.contains("H3 Affected Areas"));
        assert!(prompt.contains("H4 Deployable Scope"));
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
    fn test_ci_fix_prompt_has_diagnostic_framework() {
        let prompt = build_ci_fix_prompt(
            "APX-245",
            &["lint: failure".to_string(), "test: 3 failed".to_string()],
        );
        assert!(prompt.contains("lint: failure"));
        assert!(prompt.contains("test: 3 failed"));
        assert!(prompt.contains("CLAUDE.md"));
        assert!(prompt.contains("root cause"));
        assert!(prompt.contains("verification command"));
    }

    #[test]
    fn test_bot_review_fix_prompt() {
        let prompt = build_bot_review_fix_prompt(
            "APX-245",
            "Add NumberSequence",
            &["Consider using Option here".to_string()],
        );
        assert!(prompt.contains("Consider using Option"));
        assert!(prompt.contains("APX-245"));
        assert!(prompt.contains("Add NumberSequence"));
        assert!(prompt.contains("CLAUDE.md"));
        assert!(prompt.contains("must-fix"));
        assert!(prompt.contains("verification command"));
    }
}
