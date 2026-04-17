use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{HiveError, Result};

pub fn worktree_path(worktree_dir: &Path, issue_id: &str) -> PathBuf {
    worktree_dir.join(issue_id)
}

pub fn branch_name(issue_id: &str, suffix: &str) -> String {
    format!("{issue_id}/{suffix}")
}

pub fn create_worktree(
    repo_path: &Path,
    worktree_dir: &Path,
    issue_id: &str,
    branch: &str,
) -> Result<PathBuf> {
    let wt_path = worktree_path(worktree_dir, issue_id);
    if wt_path.exists() {
        return Ok(wt_path);
    }
    std::fs::create_dir_all(worktree_dir)?;
    let output = Command::new("git")
        .args(["worktree", "add", "-b", branch])
        .arg(&wt_path)
        .current_dir(repo_path)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(HiveError::Git(git2::Error::from_str(&format!(
            "failed to create worktree: {stderr}"
        ))));
    }
    Ok(wt_path)
}

pub fn list_worktrees(repo_path: &Path) -> Result<Vec<WorktreeInfo>> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()?;
    if !output.status.success() {
        return Err(HiveError::Git(git2::Error::from_str(
            "git worktree list failed",
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current: Option<WorktreeInfo> = None;
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            current = Some(WorktreeInfo {
                path: PathBuf::from(path),
                branch: None,
                is_bare: false,
            });
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if let Some(ref mut wt) = current {
                wt.branch = Some(branch.to_string());
            }
        } else if line == "bare"
            && let Some(ref mut wt) = current {
            wt.is_bare = true;
        }
    }
    if let Some(wt) = current {
        worktrees.push(wt);
    }
    Ok(worktrees)
}

pub fn remove_worktree(repo_path: &Path, issue_id: &str, worktree_dir: &Path) -> Result<()> {
    let wt_path = worktree_path(worktree_dir, issue_id);
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&wt_path)
        .current_dir(repo_path)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(HiveError::Git(git2::Error::from_str(&format!(
            "failed to remove worktree: {stderr}"
        ))));
    }
    Ok(())
}

pub fn rebase_worktree(worktree_path: &Path, default_branch: &str) -> Result<RebaseResult> {
    let fetch = Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(worktree_path)
        .output()?;
    if !fetch.status.success() {
        return Ok(RebaseResult::Failed);
    }
    let rebase = Command::new("git")
        .args(["rebase", &format!("origin/{default_branch}")])
        .current_dir(worktree_path)
        .output()?;
    if rebase.status.success() {
        Ok(RebaseResult::Success)
    } else {
        let _ = Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(worktree_path)
            .output();
        Ok(RebaseResult::Conflicts)
    }
}

#[derive(Debug)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_bare: bool,
}

#[derive(Debug, Clone)]
pub enum RebaseResult {
    Success,
    Conflicts,
    Failed,
}

/// Trait abstracting git worktree operations for testability.
pub trait GitOps: Send + Sync {
    fn rebase(&self, worktree_path: &Path, default_branch: &str) -> Result<RebaseResult>;
    fn remove(&self, repo_path: &Path, issue_id: &str, worktree_dir: &Path) -> Result<()>;
}

/// Default implementation that delegates to the free functions.
pub struct DefaultGitOps;

impl GitOps for DefaultGitOps {
    fn rebase(&self, worktree_path: &Path, default_branch: &str) -> Result<RebaseResult> {
        rebase_worktree(worktree_path, default_branch)
    }

    fn remove(&self, repo_path: &Path, issue_id: &str, worktree_dir: &Path) -> Result<()> {
        remove_worktree(repo_path, issue_id, worktree_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_path() {
        let base = PathBuf::from("/repo/.worktrees");
        let path = worktree_path(&base, "APX-245");
        assert_eq!(path, PathBuf::from("/repo/.worktrees/APX-245"));
    }

    #[test]
    fn test_branch_name() {
        let branch = branch_name("APX-245", "add-number-sequence");
        assert_eq!(branch, "APX-245/add-number-sequence");
    }
}
