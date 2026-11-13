use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub dir: PathBuf,
    pub branch: String,
    pub base_ref: String,
    pub base_head: String,
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn detect_base_ref(repo: &Path) -> Result<String> {
    let mut candidates = Vec::new();
    if let Ok(symbolic) = git(
        repo,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) && !symbolic.is_empty()
    {
        candidates.push(symbolic);
    }
    candidates.extend(
        ["origin/main", "origin/master", "main", "master", "HEAD"]
            .into_iter()
            .map(str::to_string),
    );
    for candidate in candidates {
        if git(
            repo,
            &["rev-parse", "--verify", &format!("{candidate}^{{commit}}")],
        )
        .is_ok()
        {
            return Ok(candidate);
        }
    }
    anyhow::bail!("cannot detect a base ref in {}", repo.display())
}

pub fn safe_segment(value: &str) -> String {
    let segment: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if segment.is_empty() || segment == "." || segment == ".." {
        "_".to_string()
    } else {
        segment
    }
}

pub fn create_worktree(
    repo: &Path,
    run_dir: &Path,
    run_id: &str,
    ticket_id: &str,
    base_ref: Option<&str>,
) -> Result<WorktreeInfo> {
    let base_ref = match base_ref {
        Some(value) => value.to_string(),
        None => detect_base_ref(repo)?,
    };
    let dir = run_dir.join("worktrees").join(safe_segment(ticket_id));
    let branch = format!("lyra/{}/{}", safe_segment(run_id), safe_segment(ticket_id));
    if dir.exists() {
        anyhow::bail!("worktree already exists: {}", dir.display());
    }
    fs::create_dir_all(run_dir.join("worktrees"))?;
    git(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            dir.to_str().unwrap_or_default(),
            &base_ref,
        ],
    )?;
    let real_dir = fs::canonicalize(&dir).unwrap_or(dir);
    let base_head = git(&real_dir, &["rev-parse", "HEAD"])?;
    Ok(WorktreeInfo {
        dir: real_dir,
        branch,
        base_ref,
        base_head,
    })
}

pub fn resume_worktree(
    repo: &Path,
    dir: &Path,
    branch: &str,
    base_head: &str,
) -> Result<WorktreeInfo> {
    if !dir.exists() {
        anyhow::bail!("checkpoint worktree is gone: {}", dir.display());
    }
    let real_dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let list = git(repo, &["worktree", "list", "--porcelain"])?;
    let expected_worktree = format!("worktree {}", real_dir.display());
    let expected_branch = format!("branch refs/heads/{branch}");
    let registered = list
        .split("\n\n")
        .any(|block| block.contains(&expected_worktree) && block.contains(&expected_branch));
    if !registered {
        anyhow::bail!(
            "checkpoint is not a registered worktree on {}: {}",
            branch,
            dir.display()
        );
    }
    let current = git(&real_dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if current != branch {
        anyhow::bail!("checkpoint is on {current}, expected {branch}");
    }
    git(
        &real_dir,
        &["merge-base", "--is-ancestor", base_head, "HEAD"],
    )
    .with_context(|| format!("checkpoint HEAD no longer descends from {base_head}"))?;
    let head = git(&real_dir, &["rev-parse", "HEAD"])?;
    Ok(WorktreeInfo {
        dir: real_dir,
        branch: branch.to_string(),
        base_ref: base_head.to_string(),
        base_head: head,
    })
}

pub fn is_clean(dir: &Path) -> bool {
    if !dir.exists() {
        return false;
    }
    git(dir, &["status", "--porcelain"])
        .map(|status| status.is_empty())
        .unwrap_or(false)
}

pub fn remove_worktree(repo: &Path, dir: &Path) -> Result<bool> {
    if !dir.exists() {
        return Ok(false);
    }
    if !is_clean(dir) {
        return Ok(false);
    }
    git(
        repo,
        &["worktree", "remove", dir.to_str().unwrap_or_default()],
    )?;
    Ok(true)
}

/// Delete a run branch whose tip still equals `base_head`: a launch that failed
/// before the worker could commit anything. Branches with worker commits are
/// kept for resume and merge. Returns whether the branch was deleted.
pub fn delete_unchanged_branch(repo: &Path, branch: &str, base_head: &str) -> Result<bool> {
    let reference = format!("refs/heads/{branch}");
    let Ok(head) = git(repo, &["rev-parse", "--verify", &reference]) else {
        return Ok(false);
    };
    if head.trim() != base_head {
        return Ok(false);
    }
    git(repo, &["branch", "-D", branch])?;
    Ok(true)
}

pub fn close_run_worktrees(
    repo: &Path,
    run_dir: &Path,
    keep: &HashSet<PathBuf>,
) -> Result<Vec<(PathBuf, bool, Option<String>)>> {
    let root = run_dir.join("worktrees");
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut results = Vec::new();
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let dir = entry.path();
        let real_dir = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if keep.contains(&dir) || keep.contains(&real_dir) {
            results.push((
                real_dir,
                false,
                Some("kept for a suspended ticket".to_string()),
            ));
            continue;
        }
        match remove_worktree(repo, &dir) {
            Ok(removed) => results.push((
                dir,
                removed,
                if removed {
                    None
                } else {
                    Some("worktree is dirty".to_string())
                },
            )),
            Err(error) => results.push((dir, false, Some(format!("{error:#}")))),
        }
    }
    if results.iter().all(|(_, removed, _)| *removed) {
        let _ = fs::remove_dir(&root);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{delete_unchanged_branch, git, safe_segment};

    #[test]
    fn path_segments_cannot_escape_the_run_directory() {
        assert_eq!(safe_segment(".."), "_");
        assert_eq!(safe_segment("."), "_");
        assert_eq!(safe_segment("../T-001"), "..-T-001");
        assert_eq!(safe_segment("run/../../x"), "run-..-..-x");
    }

    #[test]
    fn unchanged_run_branch_is_deleted_but_committed_work_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q"]).unwrap();
        git(repo, &["config", "user.email", "test@example.com"]).unwrap();
        git(repo, &["config", "user.name", "Test"]).unwrap();
        fs::write(repo.join("README.md"), "# test\n").unwrap();
        git(repo, &["add", "README.md"]).unwrap();
        git(repo, &["commit", "-qm", "init"]).unwrap();
        let head = git(repo, &["rev-parse", "HEAD"]).unwrap();
        let base_head = head.trim().to_string();
        git(repo, &["branch", "lyra/run/Ticket"]).unwrap();
        assert!(delete_unchanged_branch(repo, "lyra/run/Ticket", &base_head).unwrap());
        assert!(
            git(
                repo,
                &["rev-parse", "--verify", "refs/heads/lyra/run/Ticket"]
            )
            .is_err()
        );

        git(repo, &["branch", "lyra/run/Committed"]).unwrap();
        git(repo, &["checkout", "-q", "lyra/run/Committed"]).unwrap();
        fs::write(repo.join("work.txt"), "done\n").unwrap();
        git(repo, &["add", "work.txt"]).unwrap();
        git(repo, &["commit", "-qm", "work"]).unwrap();
        assert!(!delete_unchanged_branch(repo, "lyra/run/Committed", &base_head).unwrap());
        assert!(
            git(
                repo,
                &["rev-parse", "--verify", "refs/heads/lyra/run/Committed"]
            )
            .is_ok()
        );
        assert!(!delete_unchanged_branch(repo, "lyra/run/Missing", &base_head).unwrap());
    }
}
