//! The few git commands the core needs so far, run through the `git` program.
//!
//! Each task gets its own working copy (a git worktree) on its own branch, so
//! ten tasks in one project never touch each other's files.

use anyhow::{Context, Result, bail};
use std::path::Path;
use tokio::process::Command;

async fn git(repo: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        // A command that only reads must not take the index lock: others run at the same time.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .await
        .context("Could not run git. Is it installed?")
}

/// True when `path` is inside a git repository that has at least one commit.
/// A repository without commits cannot have working copies.
pub async fn is_repo_with_commits(path: &Path) -> bool {
    match git(path, &["rev-parse", "--verify", "--quiet", "HEAD"]).await {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

/// The branch `repo` has checked out, or `None` when it is on no branch.
pub async fn current_branch(repo: &Path) -> Option<String> {
    let output = git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await.ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !name.is_empty()).then_some(name)
}

/// Creates a new working copy of `repo` in `dir`, on a new branch.
pub async fn add_worktree(repo: &Path, dir: &Path, branch: &str) -> Result<()> {
    if let Some(parent) = dir.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let dir_text = dir.to_string_lossy();
    let output = git(repo, &["worktree", "add", "-b", branch, &dir_text]).await?;
    if !output.status.success() {
        bail!("git could not make a working copy: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

/// Removes a working copy and its branch. Used when a task could not start.
pub async fn remove_worktree(repo: &Path, dir: &Path, branch: &str) {
    let dir_text = dir.to_string_lossy();
    let _ = git(repo, &["worktree", "remove", "--force", &dir_text]).await;
    let _ = git(repo, &["branch", "-D", branch]).await;
}

/// Turns a task title into something safe for a branch or folder name.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "task".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_short_lowercase_and_safe() {
        assert_eq!(slug("Add rate limiting to /login"), "add-rate-limiting-to-login");
        assert_eq!(slug("  Fix: crash!!  "), "fix-crash");
        assert_eq!(slug("Починить вход"), "task");
        assert!(slug(&"word ".repeat(30)).len() <= 40);
    }
}
