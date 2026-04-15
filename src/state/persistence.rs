use std::path::Path;

use crate::domain::story_run::StoryRun;
use crate::error::Result;

pub fn save_run(runs_dir: &Path, run: &StoryRun) -> Result<()> {
    std::fs::create_dir_all(runs_dir)?;
    let path = runs_dir.join(format!("{}.json", run.issue_id));
    let json = serde_json::to_string_pretty(run)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn load_all_runs(runs_dir: &Path) -> Result<Vec<StoryRun>> {
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut runs = Vec::new();
    for entry in std::fs::read_dir(runs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let content = std::fs::read_to_string(&path)?;
            let run: StoryRun = serde_json::from_str(&content)?;
            runs.push(run);
        }
    }
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::story_run::StoryRun;

    #[test]
    fn test_load_all_runs() {
        let dir = tempfile::tempdir().unwrap();
        let run1 = StoryRun::new("APX-245".to_string(), "Story 1".to_string());
        let run2 = StoryRun::new("APX-270".to_string(), "Story 2".to_string());
        save_run(dir.path(), &run1).unwrap();
        save_run(dir.path(), &run2).unwrap();
        let all = load_all_runs(dir.path()).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_load_all_from_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let all = load_all_runs(dir.path()).unwrap();
        assert!(all.is_empty());
    }
}
