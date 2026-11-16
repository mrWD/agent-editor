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

/// The remote a repository is pushed to: `origin`, or else its first one. `None` for one without remotes.
pub async fn remote_name(repo: &Path) -> Option<String> {
    let output = git(repo, &["remote"]).await.ok()?;
    let listed = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut names = listed.lines().map(str::trim).filter(|name| !name.is_empty());
    let first = names.next()?.to_string();
    Some(if first == "origin" || names.any(|name| name == "origin") { "origin".into() } else { first })
}

async fn has_ref(repo: &Path, name: &str) -> bool {
    git(repo, &["show-ref", "--verify", "--quiet", name]).await.is_ok_and(|output| output.status.success())
}

/// Creates a working copy of `repo` in `dir` on a branch that is there already: the branch of a
/// request a task goes on with. What the remote has of it is fetched first. A branch this
/// repository has itself is used as it is and brought up to the remote where that only adds to
/// it; one it has not is made from the remote's. The branch is never made anew and never reset:
/// what is on it, pushed or not, is the work to go on with.
pub async fn add_worktree_on(repo: &Path, dir: &Path, branch: &str) -> Result<()> {
    if let Some(parent) = dir.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let remote = remote_name(repo).await;
    let mut fetch_said = String::new();
    if let Some(remote) = &remote {
        let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
        // Not fatal by itself: without the network the branch may still be here.
        if let Ok(output) = git(repo, &["fetch", remote, &spec]).await
            && !output.status.success()
        {
            fetch_said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        }
    }
    let theirs = remote.map(|remote| format!("{remote}/{branch}"));
    let here = has_ref(repo, &format!("refs/heads/{branch}")).await;
    let there = match &theirs {
        Some(theirs) => has_ref(repo, &format!("refs/remotes/{theirs}")).await,
        None => false,
    };
    let dir_text = dir.to_string_lossy();
    let output = match (here, &theirs) {
        (true, _) => git(repo, &["worktree", "add", &dir_text, branch]).await?,
        (false, Some(theirs)) if there => {
            git(repo, &["worktree", "add", "--track", "-b", branch, &dir_text, theirs]).await?
        }
        _ => {
            let why = if fetch_said.is_empty() { String::new() } else { format!(" git said: {fetch_said}") };
            bail!("The branch {branch} is neither in this project's folder nor on its remote.{why}");
        }
    };
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // One branch cannot be open in two folders at once.
        if said.contains("already checked out") || said.contains("already used by worktree") {
            bail!(
                "The branch {branch} is open in another folder of this project. Switch that folder to another branch, \
                 or start the task on a new branch. git said: {said}"
            );
        }
        bail!("git could not make a working copy: {said}");
    }
    if here && there {
        // What was pushed since this folder last saw the branch. A branch that went its own way is left as it is.
        let _ = git(dir, &["merge", "--ff-only", theirs.as_deref().unwrap_or_default()]).await;
    }
    Ok(())
}

/// What the remote has of a branch, fetched: `true` when it has the branch. Without the network
/// it says what was known before.
async fn fetch_branch(repo: &Path, branch: &str) -> (Option<String>, bool, String) {
    let remote = remote_name(repo).await;
    let mut said = String::new();
    if let Some(remote) = &remote {
        let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
        if let Ok(output) = git(repo, &["fetch", remote, &spec]).await
            && !output.status.success()
        {
            said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        }
    }
    let theirs = remote.map(|remote| format!("{remote}/{branch}"));
    let there = match &theirs {
        Some(theirs) => has_ref(repo, &format!("refs/remotes/{theirs}")).await,
        None => false,
    };
    (theirs, there, said)
}

/// How many files of a working copy are not as the last commit has them.
pub async fn dirty_count(dir: &Path) -> Result<usize> {
    let output = git(dir, &["status", "--porcelain"]).await?;
    if !output.status.success() {
        bail!("git could not say how the folder stands: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).lines().filter(|line| !line.trim().is_empty()).count())
}

/// How many commits the working copy has that neither `branch` (as the remote has it) nor
/// `base` holds: the work of its own that going over to `branch` would leave behind.
pub async fn commits_of_its_own(dir: &Path, branch: &str, base: Option<&str>) -> usize {
    let (theirs, there, _) = fetch_branch(dir, branch).await;
    let mut args: Vec<String> = vec!["rev-list".into(), "--count".into(), "HEAD".into(), "--not".into()];
    if let (Some(theirs), true) = (theirs, there) {
        args.push(theirs);
    }
    if has_ref(dir, &format!("refs/heads/{branch}")).await {
        args.push(branch.to_string());
    }
    if let Some(base) = base
        && git(dir, &["rev-parse", "--verify", "--quiet", base]).await.is_ok_and(|output| output.status.success())
    {
        args.push(base.to_string());
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match git(dir, &args).await {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Puts a working copy that is there already onto another branch that is there already: the
/// branch of a request its task goes over to. The same care as `add_worktree_on`: the branch is
/// fetched, used as it is where the repository has it, made from the remote's where it has not,
/// and never reset. The working copy has to be clean; the caller sees to that.
pub async fn switch_worktree_to(dir: &Path, branch: &str) -> Result<()> {
    let (theirs, there, fetch_said) = fetch_branch(dir, branch).await;
    let here = has_ref(dir, &format!("refs/heads/{branch}")).await;
    let output = match (here, &theirs) {
        (true, _) => git(dir, &["checkout", branch]).await?,
        (false, Some(theirs)) if there => git(dir, &["checkout", "--track", "-b", branch, theirs]).await?,
        _ => {
            let why = if fetch_said.is_empty() { String::new() } else { format!(" git said: {fetch_said}") };
            bail!("The branch {branch} is neither in this project's folder nor on its remote.{why}");
        }
    };
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if said.contains("already checked out") || said.contains("already used by worktree") {
            bail!(
                "The branch {branch} is open in another folder of this project. Switch that folder to another branch \
                 first. git said: {said}"
            );
        }
        bail!("git could not go over to {branch}: {said}");
    }
    if here && there {
        let _ = git(dir, &["merge", "--ff-only", theirs.as_deref().unwrap_or_default()]).await;
    }
    Ok(())
}

/// Removes a branch that holds nothing of its own any more. One that does is left: git refuses.
pub async fn drop_merged_branch(repo: &Path, branch: &str) {
    let _ = git(repo, &["branch", "-d", branch]).await;
}

/// Removes a working copy and leaves its branch: for a task that could not start on a branch that
/// was there before it.
pub async fn remove_worktree_only(repo: &Path, dir: &Path) {
    let dir_text = dir.to_string_lossy();
    let _ = git(repo, &["worktree", "remove", "--force", &dir_text]).await;
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
