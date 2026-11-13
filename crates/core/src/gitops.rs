//! Git for a task's working copy: what changed, committing, the branch's
//! history, pushing and taking things back.
//!
//! Everything goes through the `git` program, so the result is exactly what
//! the user would get in a terminal. The functions block; call them from
//! `spawn_blocking`. Git is never allowed to stop and ask for a password:
//! a command that would ask fails with git's own message instead.

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Files larger than this are listed but not shown line by line.
const MAX_DIFF_BYTES: u64 = 2 * 1024 * 1024;

fn command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        // Paths as they are, not escaped, and messages in one language so they can be read by code.
        .args(["-c", "core.quotepath=off"])
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        // Commands run side by side. One that only reads (status, diff) must not
        // take the index lock to tidy up: a commit that starts at that moment would fail.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(Stdio::null());
    command
}

fn run(root: &Path, args: &[&str]) -> Result<Output> {
    command(root).args(args).output().context("Could not run git. Is it installed?")
}

/// Runs git and returns what it printed, or its complaint as the error.
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = run(root, args)?;
    if !output.status.success() {
        bail!("{}", complaint(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn complaint(output: &Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr);
    let text = text.trim();
    let text =
        if text.is_empty() { String::from_utf8_lossy(&output.stdout).trim().to_string() } else { text.to_string() };
    // Git prefixes its messages; the words after the prefix are what a person needs.
    text.lines()
        .map(|line| line.trim_start_matches("fatal: ").trim_start_matches("error: "))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Runs git with `input` on its standard input.
fn git_with_input(root: &Path, args: &[&str], input: &str) -> Result<String> {
    let mut child = command(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not run git. Is it installed?")?;
    child.stdin.take().context("git has no input")?.write_all(input.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("{}", complaint(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn succeeds(root: &Path, args: &[&str]) -> bool {
    run(root, args).map(|output| output.status.success()).unwrap_or(false)
}

// ----- what changed -----

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    /// "modified", "added" or "deleted".
    pub kind: &'static str,
    pub added: usize,
    pub removed: usize,
    /// Not text, or too large to show line by line.
    pub binary: bool,
    /// How many separate changes the file has. Each can be committed or reverted on its own.
    pub changes: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    /// "context", "add" or "remove".
    pub kind: &'static str,
    pub old_no: Option<usize>,
    pub new_no: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub index: usize,
    /// The first and last changed line, as numbered in the file now
    /// (or as it was, for a change that only removes lines).
    pub from_line: usize,
    pub to_line: usize,
    pub lines: Vec<DiffLine>,
    #[serde(skip)]
    raw: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub kind: &'static str,
    pub binary: bool,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<Hunk>,
    /// The lines git puts before the first change; needed to apply a part of the file's changes.
    #[serde(skip)]
    header: String,
}

fn parse_range(text: &str) -> (usize, usize) {
    // "12,3" is three lines from line 12; "12" alone is one line.
    let mut parts = text.split(',');
    let start = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let count = parts.next().and_then(|n| n.parse().ok()).unwrap_or(1);
    (start, count)
}

/// Splits what `git diff` printed into files and their changes.
fn parse_patch(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut in_hunk = false;
    let (mut old_no, mut new_no) = (0usize, 0usize);

    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if let Some(rest) = bare.strip_prefix("diff --git ") {
            // "a/path b/path". The real path is taken from the +++ or --- line below; this is the fallback.
            let path = rest.split(" b/").last().unwrap_or(rest).to_string();
            files.push(FileDiff {
                path,
                kind: "modified",
                binary: false,
                added: 0,
                removed: 0,
                hunks: vec![],
                header: line.to_string(),
            });
            in_hunk = false;
            continue;
        }
        let Some(file) = files.last_mut() else { continue };

        if let Some(rest) = bare.strip_prefix("@@ ") {
            // "@@ -3,7 +3,8 @@ optional context"
            let mut parts = rest.split(' ');
            let (old_start, _) = parse_range(parts.next().unwrap_or("-0").trim_start_matches('-'));
            let (new_start, _) = parse_range(parts.next().unwrap_or("+0").trim_start_matches('+'));
            old_no = old_start;
            new_no = new_start;
            file.hunks.push(Hunk {
                index: file.hunks.len(),
                from_line: 0,
                to_line: 0,
                lines: vec![],
                raw: line.to_string(),
            });
            in_hunk = true;
            continue;
        }

        if !in_hunk {
            file.header.push_str(line);
            if bare.starts_with("new file mode") {
                file.kind = "added";
            } else if bare.starts_with("deleted file mode") {
                file.kind = "deleted";
            } else if bare.starts_with("Binary files ") || bare.starts_with("GIT binary patch") {
                file.binary = true;
            } else if let Some(path) = bare.strip_prefix("+++ b/") {
                file.path = path.trim_end_matches('\t').to_string();
            } else if let Some(path) = bare.strip_prefix("--- a/")
                && file.kind == "deleted"
            {
                file.path = path.trim_end_matches('\t').to_string();
            }
            continue;
        }

        let hunk = file.hunks.last_mut().expect("a hunk is open");
        hunk.raw.push_str(line);
        let (kind, text) = match bare.chars().next() {
            Some('+') => ("add", &bare[1..]),
            Some('-') => ("remove", &bare[1..]),
            Some(' ') => ("context", &bare[1..]),
            // "\ No newline at end of file" belongs to the patch but is not a line of the file.
            _ => continue,
        };
        let (line_old, line_new) = match kind {
            "add" => (None, Some(new_no)),
            "remove" => (Some(old_no), None),
            _ => (Some(old_no), Some(new_no)),
        };
        if kind != "add" {
            old_no += 1;
        }
        if kind != "remove" {
            new_no += 1;
        }
        match kind {
            "add" => file.added += 1,
            "remove" => file.removed += 1,
            _ => {}
        }
        hunk.lines.push(DiffLine { kind, old_no: line_old, new_no: line_new, text: text.to_string() });
    }

    for file in &mut files {
        for hunk in &mut file.hunks {
            let changed: Vec<usize> = hunk
                .lines
                .iter()
                .filter(|line| line.kind != "context")
                .filter_map(|line| line.new_no.or(line.old_no))
                .collect();
            let added: Vec<usize> = hunk.lines.iter().filter(|l| l.kind == "add").filter_map(|l| l.new_no).collect();
            // Say where the change is in the file as it is now, when it has lines there.
            let range = if added.is_empty() { &changed } else { &added };
            hunk.from_line = range.first().copied().unwrap_or(0);
            hunk.to_line = range.last().copied().unwrap_or(0);
        }
    }
    files
}

/// A file git does not know yet, shown as one change that adds every line.
fn untracked_diff(root: &Path, path: &str) -> FileDiff {
    let full = root.join(path);
    let size = std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
    let text =
        (size <= MAX_DIFF_BYTES).then(|| std::fs::read(&full).ok().and_then(|b| String::from_utf8(b).ok())).flatten();
    let mut diff = FileDiff {
        path: path.to_string(),
        kind: "added",
        binary: text.is_none(),
        added: 0,
        removed: 0,
        hunks: vec![],
        header: String::new(),
    };
    if let Some(text) = text {
        let lines: Vec<DiffLine> = text
            .lines()
            .enumerate()
            .map(|(i, line)| DiffLine { kind: "add", old_no: None, new_no: Some(i + 1), text: line.to_string() })
            .collect();
        diff.added = lines.len();
        if !lines.is_empty() {
            diff.hunks.push(Hunk { index: 0, from_line: 1, to_line: lines.len(), lines, raw: String::new() });
        }
    }
    diff
}

/// The paths git reports as changed, with whether git knows the file at all.
fn status_paths(root: &Path) -> Result<Vec<(String, bool)>> {
    let text = git(root, &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--no-renames"])?;
    let mut paths = Vec::new();
    for entry in text.split('\0') {
        if entry.len() < 4 {
            continue;
        }
        let (status, path) = entry.split_at(3);
        paths.push((path.to_string(), status.starts_with("??")));
    }
    Ok(paths)
}

fn tracked_diffs(root: &Path, path: Option<&str>) -> Result<Vec<FileDiff>> {
    let mut args = vec!["diff", "HEAD", "--no-color", "--no-ext-diff", "--no-renames", "-U3", "--"];
    if let Some(path) = path {
        args.push(path);
    }
    Ok(parse_patch(&git(root, &args)?))
}

/// Every file that differs from the last commit.
pub fn changes(root: &Path) -> Result<Vec<ChangedFile>> {
    let tracked: HashMap<String, FileDiff> =
        tracked_diffs(root, None)?.into_iter().map(|d| (d.path.clone(), d)).collect();
    let mut files = Vec::new();
    for (path, untracked) in status_paths(root)? {
        let diff = if untracked {
            untracked_diff(root, &path)
        } else {
            match tracked.get(&path) {
                Some(diff) => diff.clone(),
                None => continue,
            }
        };
        files.push(ChangedFile {
            path,
            kind: diff.kind,
            added: diff.added,
            removed: diff.removed,
            binary: diff.binary,
            changes: diff.hunks.len().max(1),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// The changes of one file, line by line.
pub fn file_diff(root: &Path, path: &str) -> Result<FileDiff> {
    if let Some(diff) = tracked_diffs(root, Some(path))?.into_iter().next() {
        return Ok(diff);
    }
    if status_paths(root)?.iter().any(|(p, untracked)| p == path && *untracked) {
        return Ok(untracked_diff(root, path));
    }
    bail!("{path} is the same as in the last commit");
}

/// The patch that carries only some of a file's changes.
fn partial_patch(diff: &FileDiff, picked: &[usize]) -> Result<String> {
    let mut patch = diff.header.clone();
    for index in picked {
        let hunk = diff.hunks.get(*index).ok_or_else(|| anyhow!("{} has no change number {}", diff.path, index + 1))?;
        patch.push_str(&hunk.raw);
    }
    Ok(patch)
}

// ----- the branch -----

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    /// When it was made, in milliseconds.
    pub at: u64,
    pub pushed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BranchState {
    pub branch: Option<String>,
    /// The branch this one was started from.
    pub base: Option<String>,
    /// How many commits the base has that this branch does not.
    pub base_ahead: usize,
    /// The name of the remote, when the repository has one.
    pub remote: Option<String>,
    pub not_pushed: usize,
    pub files: Vec<ChangedFile>,
    /// The commits of this branch, newest first.
    pub commits: Vec<Commit>,
}

fn rev(root: &Path, name: &str) -> Option<String> {
    let output = run(root, &["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")]).ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn current_branch(root: &Path) -> Option<String> {
    let name = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The remote to push to: `origin` when there is one, else the first.
fn remote(root: &Path) -> Option<String> {
    let text = git(root, &["remote"]).ok()?;
    let names: Vec<&str> = text.lines().map(str::trim).filter(|n| !n.is_empty()).collect();
    names.iter().find(|n| **n == "origin").or(names.first()).map(|n| n.to_string())
}

/// True when the branch exists on the remote as far as this copy knows (no network is used).
pub fn is_pushed(root: &Path, branch: &str) -> bool {
    remote(root).is_some_and(|remote| rev(root, &format!("refs/remotes/{remote}/{branch}")).is_some())
}

/// The base to compare with: the one the task remembers, else the usual names.
pub fn resolve_base(root: &Path, remembered: Option<&str>) -> Option<String> {
    let branch = current_branch(root);
    remembered
        .into_iter()
        .chain(["main", "master", "develop"])
        .find(|name| Some(*name) != branch.as_deref() && rev(root, name).is_some())
        .map(str::to_string)
}

fn count(root: &Path, range: &str) -> usize {
    git(root, &["rev-list", "--count", range]).ok().and_then(|n| n.trim().parse().ok()).unwrap_or(0)
}

fn commits(root: &Path, base: Option<&str>) -> Result<Vec<Commit>> {
    let range = match base {
        Some(base) => format!("{base}..HEAD"),
        None => "HEAD".to_string(),
    };
    let text = git(root, &["log", "--max-count=50", "--format=%H%x1f%h%x1f%s%x1f%an%x1f%at", &range])?;

    // A commit is pushed when the branch on the remote already has it.
    let branch = current_branch(root);
    let upstream = match (remote(root), &branch) {
        (Some(remote), Some(branch)) => rev(root, &format!("refs/remotes/{remote}/{branch}")),
        _ => None,
    };
    let not_pushed: Option<HashSet<String>> = upstream.map(|upstream| {
        git(root, &["rev-list", &format!("{upstream}..HEAD")]).unwrap_or_default().lines().map(str::to_string).collect()
    });

    Ok(text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            let hash = parts.next()?.to_string();
            Some(Commit {
                pushed: not_pushed.as_ref().is_some_and(|set| !set.contains(&hash)),
                hash,
                short: parts.next()?.to_string(),
                subject: parts.next()?.to_string(),
                author: parts.next()?.to_string(),
                at: parts.next()?.parse::<u64>().ok()? * 1000,
            })
        })
        .collect())
}

/// Everything the Changes view shows on its left side.
pub fn state(root: &Path, remembered_base: Option<&str>) -> Result<BranchState> {
    let base = resolve_base(root, remembered_base);
    let commits = commits(root, base.as_deref())?;
    Ok(BranchState {
        branch: current_branch(root),
        base_ahead: base.as_deref().map(|base| count(root, &format!("HEAD..{base}"))).unwrap_or(0),
        base,
        remote: remote(root),
        not_pushed: commits.iter().filter(|c| !c.pushed).count(),
        files: changes(root)?,
        commits,
    })
}

// ----- committing -----

/// What of one file goes into a commit: all of it, or only some of its changes.
#[derive(Debug, Clone)]
pub struct Pick {
    pub path: String,
    /// `None` takes the whole file.
    pub changes: Option<Vec<usize>>,
}

/// Commits the picked changes and nothing else. Returns the new commit.
pub fn commit(root: &Path, message: &str, picks: &[Pick], amend: bool) -> Result<Commit> {
    let message = message.trim();
    if message.is_empty() {
        bail!("Write a commit message first");
    }
    if picks.is_empty() {
        bail!("Tick at least one file");
    }

    // Start from a clean slate: only what is picked here goes in.
    git(root, &["reset", "-q"])?;
    let staged = (|| -> Result<()> {
        for pick in picks {
            match &pick.changes {
                None => {
                    git(root, &["add", "-A", "--", &pick.path])?;
                }
                Some(indices) => {
                    let diff = file_diff(root, &pick.path)?;
                    if indices.len() >= diff.hunks.len() || diff.header.is_empty() {
                        git(root, &["add", "-A", "--", &pick.path])?;
                    } else {
                        let patch = partial_patch(&diff, indices)?;
                        git_with_input(root, &["apply", "--cached", "--whitespace=nowarn", "-"], &patch)
                            .with_context(|| format!("Could not take a part of {}", pick.path))?;
                    }
                }
            }
        }
        if succeeds(root, &["diff", "--cached", "--quiet"]) && !amend {
            bail!("Nothing that is ticked differs from the last commit");
        }
        let mut args = vec!["commit", "-q", "-m", message];
        if amend {
            args.push("--amend");
        }
        git(root, &args).map_err(no_identity)?;
        Ok(())
    })();
    if staged.is_err() {
        // Leave the index as it was found: nothing half-added.
        let _ = git(root, &["reset", "-q"]);
    }
    staged?;
    commits(root, None)?.into_iter().next().context("The commit was made but cannot be read back")
}

/// Sends the branch to the remote.
pub fn push(root: &Path) -> Result<()> {
    let branch = current_branch(root).context("This working copy is on no branch, so there is nothing to push")?;
    let remote = remote(root)
        .context("This project has no remote to push to. Add one with `git remote add origin <address>`.")?;
    git(root, &["push", "-q", "-u", &remote, &branch])?;
    Ok(())
}

/// Takes the newest commit off the branch and keeps its changes in the files.
pub fn undo_commit(root: &Path, hash: &str) -> Result<()> {
    let head = rev(root, "HEAD").context("There are no commits")?;
    if head != hash {
        bail!("Only the newest commit can be undone. Undo the ones above it first.");
    }
    if rev(root, "HEAD~1").is_none() {
        bail!("This is the first commit of the repository; it cannot be undone");
    }
    git(root, &["reset", "-q", "--soft", "HEAD~1"])?;
    git(root, &["reset", "-q"])?;
    Ok(())
}

/// Adds a commit that takes back what `hash` did.
pub fn revert_commit(root: &Path, hash: &str) -> Result<Commit> {
    if let Err(error) = git(root, &["revert", "--no-edit", hash]) {
        let _ = git(root, &["revert", "--abort"]);
        bail!("That commit cannot be taken back cleanly: {error}. Nothing was changed.");
    }
    commits(root, None)?.into_iter().next().context("The revert was made but cannot be read back")
}

/// Puts a file back as it is in the last commit. A file the commit does not have is removed.
pub fn revert_file(root: &Path, path: &str) -> Result<()> {
    if succeeds(root, &["cat-file", "-e", &format!("HEAD:{path}")]) {
        git(root, &["checkout", "-q", "HEAD", "--", path])?;
    } else {
        let _ = git(root, &["rm", "-q", "--cached", "--ignore-unmatch", "--", path]);
        let full = root.join(path);
        if full.exists() {
            std::fs::remove_file(&full).with_context(|| format!("Could not remove {path}"))?;
        }
    }
    Ok(())
}

/// Takes back one change of a file and leaves its other changes.
pub fn revert_change(root: &Path, path: &str, index: usize) -> Result<()> {
    let diff = file_diff(root, path)?;
    if diff.header.is_empty() || diff.hunks.len() <= 1 {
        return revert_file(root, path);
    }
    let patch = partial_patch(&diff, &[index])?;
    // The index may hold an older state of the file; the working copy is what is being changed.
    git(root, &["reset", "-q", "--", path])?;
    git_with_input(root, &["apply", "-R", "--whitespace=nowarn", "-"], &patch)
        .with_context(|| format!("Could not take back that change of {path}"))?;
    Ok(())
}

/// Brings the base's new commits into this branch. On a conflict nothing is changed.
pub fn update_from_base(root: &Path, base: &str) -> Result<()> {
    if let Err(error) = git(root, &["merge", "--no-edit", "-q", base]) {
        let conflicted = git(root, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
        let files: Vec<&str> = conflicted.lines().filter(|l| !l.is_empty()).collect();
        let _ = git(root, &["merge", "--abort"]);
        if files.is_empty() {
            bail!("Could not update from {base}: {error}");
        }
        bail!("Updating from {base} would conflict in {}. Nothing was changed.", files.join(", "));
    }
    Ok(())
}

// ---------- snapshots: the files of a working copy at one moment ----------

/// Who the snapshot commits are written by. They are never pushed and never on a branch.
const SNAPSHOT_AUTHOR: [&str; 4] =
    ["-c", "user.name=Agent Editor", "-c", "user.email=checkpoints@agent-editor.invalid"];

/// Writes every file of the working copy (all that git does not ignore) as one
/// tree object and returns its id. The index, the branch and the files are not
/// touched: a copy of the index is used and thrown away.
pub fn snapshot_tree(root: &Path) -> Result<String> {
    let git_dir = PathBuf::from(git(root, &["rev-parse", "--absolute-git-dir"])?.trim());
    let index = {
        let path = PathBuf::from(git(root, &["rev-parse", "--git-path", "index"])?.trim());
        if path.is_absolute() { path } else { root.join(path) }
    };
    let temp = git_dir.join(format!("ae-index-{}", uuid::Uuid::new_v4().simple()));
    let with_temp = |args: &[&str]| -> Result<String> {
        let output = command(root).env("GIT_INDEX_FILE", &temp).args(args).output().context("Could not run git")?;
        if !output.status.success() {
            bail!("{}", complaint(&output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let made = (|| {
        // Starting from the real index keeps what git already knows about each
        // file, so only the files that changed are read again.
        if index.is_file() {
            std::fs::copy(&index, &temp).context("Could not copy the index")?;
        } else if rev(root, "HEAD").is_some() {
            with_temp(&["read-tree", "HEAD"])?;
        }
        with_temp(&["add", "-A"])?;
        with_temp(&["write-tree"])
    })();
    let _ = std::fs::remove_file(&temp);
    let _ = std::fs::remove_file(temp.with_extension("lock"));
    made
}

/// Keeps a tree under `name` (a ref outside the branches), so git does not
/// throw it away, and returns the id of the commit that holds it.
pub fn keep_tree(root: &Path, tree: &str, name: &str, message: &str) -> Result<String> {
    let mut args: Vec<&str> = SNAPSHOT_AUTHOR.to_vec();
    args.extend(["commit-tree", tree, "-m", message]);
    let head = rev(root, "HEAD");
    if let Some(head) = &head {
        args.extend(["-p", head]);
    }
    let commit = git(root, &args)?.trim().to_string();
    git(root, &["update-ref", name, &commit])?;
    Ok(commit)
}

/// Forgets every snapshot kept under names that start with `prefix`.
pub fn drop_snapshots(root: &Path, prefix: &str) {
    let Ok(listed) = git(root, &["for-each-ref", "--format=%(refname)", prefix]) else { return };
    for name in listed.lines().filter(|name| !name.is_empty()) {
        let _ = git(root, &["update-ref", "-d", name]);
    }
}

fn diffs_between(root: &Path, from: &str, to: &str, path: Option<&str>) -> Result<Vec<FileDiff>> {
    let mut args = vec!["diff", "--no-color", "--no-ext-diff", "--no-renames", "-U3", from, to, "--"];
    if let Some(path) = path {
        args.push(path);
    }
    Ok(parse_patch(&git(root, &args)?))
}

/// The files that differ between two snapshots, commits or trees.
pub fn changes_between(root: &Path, from: &str, to: &str) -> Result<Vec<ChangedFile>> {
    let mut files: Vec<ChangedFile> = diffs_between(root, from, to, None)?
        .into_iter()
        .map(|diff| ChangedFile {
            path: diff.path.clone(),
            kind: diff.kind,
            added: diff.added,
            removed: diff.removed,
            binary: diff.binary,
            changes: diff.hunks.len().max(1),
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// One file's changes between two snapshots, commits or trees, line by line.
pub fn diff_between(root: &Path, from: &str, to: &str, path: &str) -> Result<FileDiff> {
    match diffs_between(root, from, to, Some(path))?.into_iter().next() {
        Some(diff) => Ok(diff),
        None => bail!("{path} is the same at both points"),
    }
}

/// One change to a file, as line numbers on both sides and the lines themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stretch {
    /// The first line taken out, as numbered before. For a change that only
    /// adds lines, the line after which they were put in, plus one.
    pub old_start: usize,
    pub old_len: usize,
    /// The first line put in, as numbered after.
    pub new_start: usize,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// Every change between two snapshots, commits or trees, file by file, with
/// no lines around them: exactly which lines each side touched.
pub fn stretches_between(root: &Path, from: &str, to: &str) -> Result<HashMap<String, Vec<Stretch>>> {
    let text = git(root, &["diff", "--no-color", "--no-ext-diff", "--no-renames", "-U0", from, to, "--"])?;
    Ok(parse_stretches(&text))
}

fn parse_stretches(text: &str) -> HashMap<String, Vec<Stretch>> {
    let mut files: HashMap<String, Vec<Stretch>> = HashMap::new();
    let mut path = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            path = rest.split(" b/").last().unwrap_or(rest).to_string();
        } else if let Some(rest) = line.strip_prefix("+++ b/") {
            path = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            let mut parts = rest.split(' ');
            let (old_start, old_len) = parse_range(parts.next().unwrap_or("-0").trim_start_matches('-'));
            let (new_start, _) = parse_range(parts.next().unwrap_or("+0").trim_start_matches('+'));
            // For an addition git names the line before it; the change itself begins one further.
            let old_start = if old_len == 0 { old_start + 1 } else { old_start };
            files.entry(path.clone()).or_default().push(Stretch {
                old_start,
                old_len,
                new_start,
                added: Vec::new(),
                removed: Vec::new(),
            });
        } else if let Some(stretch) = files.get_mut(&path).and_then(|list| list.last_mut()) {
            if let Some(added) = line.strip_prefix('+') {
                stretch.added.push(added.to_string());
            } else if let Some(removed) = line.strip_prefix('-') {
                stretch.removed.push(removed.to_string());
            }
        }
    }
    files
}

/// The commit a ref or a working copy's HEAD points at.
pub fn commit_of(root: &Path, name: &str) -> Option<String> {
    rev(root, name)
}

/// Git refuses to commit until it knows who is committing. Said plainly, with what to do about it.
fn no_identity(error: anyhow::Error) -> anyhow::Error {
    let said = error.to_string();
    // Git says this three ways: no identity at all, no name, or no email.
    let lost = ["Please tell me who you are", "Author identity unknown", "no name was given", "no email was given"];
    if lost.iter().any(|words| said.contains(words)) {
        return anyhow!(
            "Git does not know who you are, so it will not make a commit. Set it once:\n\n  \
             git config --global user.name \"Your Name\"\n  git config --global user.email \"you@example.com\""
        );
    }
    error
}

/// The newest commit two commits both come from.
pub fn common_start(root: &Path, a: &str, b: &str) -> Option<String> {
    let text = git(root, &["merge-base", a, b]).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A file's bytes as they were in a snapshot or commit. `None` when it was not there.
pub fn bytes_at(root: &Path, rev_name: &str, path: &str) -> Result<Option<Vec<u8>>> {
    let output = run(root, &["cat-file", "blob", &format!("{rev_name}:{path}")])?;
    Ok(output.status.success().then_some(output.stdout))
}

/// The commit where the task's branch left `base`, if both are known.
pub fn fork_point(root: &Path, base: &str) -> Option<String> {
    let text = git(root, &["merge-base", base, "HEAD"]).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The commits made after `from` on the current branch, oldest first.
pub fn commits_since(root: &Path, from: &str) -> Vec<(String, String)> {
    let Ok(text) = git(root, &["log", "--reverse", "--format=%h%x1f%s", &format!("{from}..HEAD")]) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| line.split_once('\x1f'))
        .map(|(short, subject)| (short.into(), subject.into()))
        .collect()
}

// ---------- commits: finding one and looking at it ----------

/// A commit as the search over everything lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FoundCommit {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    /// When it was made, in milliseconds.
    pub at: u64,
    /// True for a commit of the task's own branch, one the branch it started from does not have.
    pub on_branch: bool,
}

/// One commit in full: what it says and which files it changed.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShownCommit {
    #[serde(flatten)]
    pub commit: FoundCommit,
    /// The message below its first line.
    pub body: String,
    pub files: Vec<ChangedFile>,
}

const FOUND_FORMAT: &str = "--format=%H%x1f%h%x1f%s%x1f%an%x1f%at";
/// How many of the newest commits a search reads the first lines of. It is asked
/// for on every key the user types, so its cost must not grow with the age of a
/// project. An older commit is still found by its id.
const COMMITS_LOOKED_AT: usize = 5000;

/// One line of `git log` in `FOUND_FORMAT`.
fn parse_found(line: &str, on_branch: &HashSet<String>) -> Option<FoundCommit> {
    let mut parts = line.split('\x1f');
    let hash = parts.next()?.to_string();
    Some(FoundCommit {
        on_branch: on_branch.contains(&hash),
        hash,
        short: parts.next()?.to_string(),
        subject: parts.next()?.to_string(),
        author: parts.next()?.to_string(),
        at: parts.next()?.parse::<u64>().ok()? * 1000,
    })
}

/// The commits the working copy's branch has and its base does not.
fn branch_commits(root: &Path, base: Option<&str>) -> HashSet<String> {
    let Some(base) = base else { return HashSet::new() };
    git(root, &["rev-list", "--max-count=2000", &format!("{base}..HEAD")])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// True for text that can be the id of a commit, or the start of one.
fn looks_like_an_id(text: &str) -> bool {
    (4..=64).contains(&text.len()) && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// Commits whose first line holds `query`, in any letter case, or whose id
/// starts with it. Looked for on the task's branch and on the branch it was
/// started from, which is the project's history. Newest first; a commit found
/// by its id comes before the rest.
///
/// A working copy without a commit, or a folder without git, has none: that
/// is an empty answer, not an error, because a search asks everywhere at once.
pub fn find_commits(root: &Path, remembered_base: Option<&str>, query: &str, limit: usize) -> Vec<FoundCommit> {
    let query = query.trim();
    if query.is_empty() || limit == 0 || rev(root, "HEAD").is_none() {
        return Vec::new();
    }
    let base = resolve_base(root, remembered_base);
    let on_branch = branch_commits(root, base.as_deref());
    let mut tips = vec!["HEAD"];
    tips.extend(base.as_deref());

    // The first lines are matched here, not by git: git would look through whole messages, of the
    // whole history, and what it found in a message's later lines could not be shown in a row.
    let count = format!("--max-count={COMMITS_LOOKED_AT}");
    let mut args = vec!["log", FOUND_FORMAT, &count];
    args.extend(&tips);
    args.push("--");
    let needle = query.to_lowercase();
    let mut found: Vec<FoundCommit> = git(root, &args)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| parse_found(line, &on_branch))
        .filter(|commit| commit.subject.to_lowercase().contains(&needle))
        .take(limit)
        .collect();

    // An id, or its first letters, names one commit. It counts when it is part of the same history.
    if looks_like_an_id(query)
        && let Some(hash) = rev(root, query)
        && !found.iter().any(|commit| commit.hash == hash)
        && tips.iter().any(|tip| succeeds(root, &["merge-base", "--is-ancestor", &hash, tip]))
    {
        let text = git(root, &["log", "-1", FOUND_FORMAT, &hash, "--"]).unwrap_or_default();
        found.splice(0..0, text.lines().next().and_then(|line| parse_found(line, &on_branch)));
    }
    found.truncate(limit);
    found
}

/// The patch of one commit, compared with the commit before it. A merge is
/// compared with the branch it was merged into; the first commit of a
/// repository adds everything it holds.
fn commit_patch(root: &Path, hash: &str, path: Option<&str>) -> Result<Vec<FileDiff>> {
    let mut args = vec![
        "show",
        "--format=",
        "--diff-merges=first-parent",
        "--no-color",
        "--no-ext-diff",
        "--no-renames",
        "-U3",
        hash,
        "--",
    ];
    args.extend(path);
    Ok(parse_patch(&git(root, &args)?))
}

/// The full id of the commit `id` names, when it names one.
fn commit_named(root: &Path, id: &str) -> Result<String> {
    // Only an id is taken: anything else could be read by git as a path or an option.
    if !looks_like_an_id(id) {
        bail!("That is not the id of a commit");
    }
    rev(root, id).ok_or_else(|| anyhow!("This project has no commit {id}"))
}

/// One commit with the files it changed.
pub fn show_commit(root: &Path, remembered_base: Option<&str>, id: &str) -> Result<ShownCommit> {
    let hash = commit_named(root, id)?;
    let base = resolve_base(root, remembered_base);
    let on_branch = branch_commits(root, base.as_deref());
    let commit = git(root, &["log", "-1", FOUND_FORMAT, &hash, "--"])?
        .lines()
        .next()
        .and_then(|line| parse_found(line, &on_branch))
        .context("The commit cannot be read")?;
    let body = git(root, &["log", "-1", "--format=%b", &hash, "--"])?.trim().to_string();
    let mut files: Vec<ChangedFile> = commit_patch(root, &hash, None)?
        .into_iter()
        .map(|diff| ChangedFile {
            path: diff.path.clone(),
            kind: diff.kind,
            added: diff.added,
            removed: diff.removed,
            binary: diff.binary,
            changes: diff.hunks.len().max(1),
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ShownCommit { commit, body, files })
}

/// What one commit changed in one file, line by line.
pub fn commit_file_diff(root: &Path, id: &str, path: &str) -> Result<FileDiff> {
    let hash = commit_named(root, id)?;
    match commit_patch(root, &hash, Some(path))?.into_iter().next() {
        Some(diff) => Ok(diff),
        None => bail!("That commit did not change {path}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn changes_without_context_say_exactly_which_lines_each_side_touched() {
        let patch = "diff --git a/src/a.ts b/src/a.ts\nindex 1..2 100644\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -3,0 +4,2 @@ import x\n+one\n+two\n@@ -12 +14 @@\n-old\n+new\n@@ -20,2 +21,0 @@\n-gone\n-too\n";
        let files = super::parse_stretches(patch);
        let list = &files["src/a.ts"];
        assert_eq!(list.len(), 3);
        // Two lines put in after line 3: the change sits at line 4 of the old text and takes nothing out.
        assert_eq!((list[0].old_start, list[0].old_len, list[0].new_start), (4, 0, 4));
        assert_eq!(list[0].added, ["one", "two"]);
        assert_eq!((list[1].old_start, list[1].old_len, list[1].new_start), (12, 1, 14));
        assert_eq!(
            (list[1].removed.as_slice(), list[1].added.as_slice()),
            (&["old".to_string()][..], &["new".to_string()][..])
        );
        assert_eq!((list[2].old_start, list[2].old_len), (20, 2));
        assert!(list[2].added.is_empty());
    }

    use super::*;
    use std::path::PathBuf;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ae-gitops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let dir = crate::paths::real_path(&dir).unwrap();
        let lines: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        std::fs::write(dir.join("src/long.txt"), lines).unwrap();
        std::fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        git(&dir, &["init", "-q", "-b", "main"]).unwrap();
        git(&dir, &["config", "user.name", "Test"]).unwrap();
        git(&dir, &["config", "user.email", "test@example.com"]).unwrap();
        git(&dir, &["add", "."]).unwrap();
        git(&dir, &["commit", "-q", "-m", "Start"]).unwrap();
        git(&dir, &["checkout", "-q", "-b", "task/one"]).unwrap();
        dir
    }

    /// Changes line 2 and line 28 (two separate changes), deletes a file and adds one.
    fn edit(dir: &Path) {
        let text = std::fs::read_to_string(dir.join("src/long.txt")).unwrap();
        let text = text.replace("line 2\n", "line two\n").replace("line 28\n", "line 28\nline 28 and a half\n");
        std::fs::write(dir.join("src/long.txt"), text).unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::write(dir.join("new file.txt"), "hello\nworld\n").unwrap();
    }

    #[test]
    fn lists_what_changed_with_counts() {
        let dir = repo("changes");
        edit(&dir);
        let files = changes(&dir).unwrap();
        let summary: Vec<_> = files.iter().map(|f| (f.path.as_str(), f.kind, f.added, f.removed, f.changes)).collect();
        assert_eq!(
            summary,
            vec![
                ("gone.txt", "deleted", 0, 1, 1),
                ("new file.txt", "added", 2, 0, 1),
                ("src/long.txt", "modified", 2, 1, 2),
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shows_a_file_change_by_change_with_line_numbers() {
        let dir = repo("diff");
        edit(&dir);
        let diff = file_diff(&dir, "src/long.txt").unwrap();
        assert_eq!(diff.hunks.len(), 2);
        assert_eq!((diff.hunks[0].from_line, diff.hunks[0].to_line), (2, 2));
        assert_eq!((diff.hunks[1].from_line, diff.hunks[1].to_line), (29, 29));
        let changed: Vec<_> = diff.hunks[0].lines.iter().filter(|l| l.kind != "context").collect();
        assert_eq!(changed[0], &DiffLine { kind: "remove", old_no: Some(2), new_no: None, text: "line 2".into() });
        assert_eq!(changed[1], &DiffLine { kind: "add", old_no: None, new_no: Some(2), text: "line two".into() });

        let added = file_diff(&dir, "new file.txt").unwrap();
        assert_eq!((added.kind, added.added, added.hunks.len()), ("added", 2, 1));
        assert!(file_diff(&dir, "nothing-here.txt").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn commits_only_what_is_picked_down_to_one_change_of_a_file() {
        let dir = repo("commit");
        edit(&dir);
        let made = commit(
            &dir,
            "Rename line two",
            &[
                Pick { path: "src/long.txt".into(), changes: Some(vec![0]) },
                Pick { path: "gone.txt".into(), changes: None },
            ],
            false,
        )
        .unwrap();
        assert_eq!(made.subject, "Rename line two");
        assert!(!made.pushed);

        // The second change of the file and the new file are still waiting.
        let left = changes(&dir).unwrap();
        let summary: Vec<_> = left.iter().map(|f| (f.path.as_str(), f.added, f.removed, f.changes)).collect();
        assert_eq!(summary, vec![("new file.txt", 2, 0, 1), ("src/long.txt", 1, 0, 1)]);
        let committed = git(&dir, &["show", "--stat", "--format=", "HEAD"]).unwrap();
        assert!(
            committed.contains("gone.txt") && committed.contains("src/long.txt") && !committed.contains("new file")
        );

        // Adding to the last commit keeps one commit on the branch.
        commit(&dir, "Rename line two and add a file", &[Pick { path: "new file.txt".into(), changes: None }], true)
            .unwrap();
        let state = state(&dir, Some("main")).unwrap();
        assert_eq!(state.commits.len(), 1);
        assert_eq!(state.commits[0].subject, "Rename line two and add a file");
        assert_eq!(
            (state.branch.as_deref(), state.base.as_deref(), state.remote),
            (Some("task/one"), Some("main"), None)
        );

        assert!(commit(&dir, "  ", &[Pick { path: "src/long.txt".into(), changes: None }], false).is_err());
        assert!(commit(&dir, "Nothing", &[], false).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn git_with_nobody_to_commit_as_says_what_to_do() {
        let plain = |raw: &str| no_identity(anyhow!("{raw}")).to_string();
        for raw in [
            "*** Please tell me who you are.",
            "fatal: Author identity unknown",
            "fatal: no name was given and auto-detection is disabled",
            "fatal: no email was given and auto-detection is disabled",
        ] {
            let said = plain(raw);
            assert!(said.starts_with("Git does not know who you are"), "{said}");
            assert!(said.contains("git config --global user.name"), "{said}");
        }
        // Anything else is passed on as git said it.
        assert_eq!(plain("fatal: pathspec 'x' did not match any files"), "fatal: pathspec 'x' did not match any files");
    }

    #[test]
    fn takes_back_one_change_a_file_and_a_commit() {
        let dir = repo("revert");
        edit(&dir);
        revert_change(&dir, "src/long.txt", 0).unwrap();
        let text = std::fs::read_to_string(dir.join("src/long.txt")).unwrap();
        assert!(text.contains("line 2\n") && text.contains("line 28 and a half\n"));

        revert_file(&dir, "gone.txt").unwrap();
        assert!(dir.join("gone.txt").is_file());
        revert_file(&dir, "new file.txt").unwrap();
        assert!(!dir.join("new file.txt").exists());

        let made = commit(&dir, "Add a line", &[Pick { path: "src/long.txt".into(), changes: None }], false).unwrap();
        assert!(changes(&dir).unwrap().is_empty());
        undo_commit(&dir, &made.hash).unwrap();
        assert_eq!(changes(&dir).unwrap().len(), 1, "the commit is gone and its change is back in the file");
        assert!(undo_commit(&dir, "0000").is_err());

        let again = commit(&dir, "Add a line", &[Pick { path: "src/long.txt".into(), changes: None }], false).unwrap();
        let reverted = revert_commit(&dir, &again.hash).unwrap();
        assert!(reverted.subject.starts_with("Revert"));
        assert!(!std::fs::read_to_string(dir.join("src/long.txt")).unwrap().contains("and a half"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pushes_to_a_remote_and_knows_what_is_pushed() {
        let dir = repo("push");
        let bare = dir.parent().unwrap().join(format!("ae-gitops-remote-{}.git", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        git(&dir, &["init", "-q", "--bare", &bare.to_string_lossy()]).unwrap();
        assert!(push(&dir).unwrap_err().to_string().contains("no remote"));
        git(&dir, &["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();

        edit(&dir);
        commit(&dir, "First", &[Pick { path: "gone.txt".into(), changes: None }], false).unwrap();
        assert_eq!(state(&dir, Some("main")).unwrap().not_pushed, 1);
        push(&dir).unwrap();
        commit(&dir, "Second", &[Pick { path: "new file.txt".into(), changes: None }], false).unwrap();

        let state = state(&dir, Some("main")).unwrap();
        let pushed: Vec<_> = state.commits.iter().map(|c| (c.subject.as_str(), c.pushed)).collect();
        assert_eq!(pushed, vec![("Second", false), ("First", true)]);
        assert_eq!((state.not_pushed, state.remote.as_deref()), (1, Some("origin")));
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(bare).unwrap();
    }

    #[test]
    fn finds_commits_by_their_first_line_and_by_id_and_shows_one() {
        let dir = repo("find");
        // One more commit on the base, after the task's branch left it.
        git(&dir, &["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("gone.txt"), "changed on main\n").unwrap();
        git(&dir, &["commit", "-q", "-am", "Say goodbye on main", "-m", "The upload LIMIT is in the body only."])
            .unwrap();
        git(&dir, &["checkout", "-q", "task/one"]).unwrap();
        edit(&dir);
        let made = commit(
            &dir,
            "Limit login attempts to five a minute\n\nAsked for in the task.",
            &[Pick { path: "src/long.txt".into(), changes: None }, Pick { path: "new file.txt".into(), changes: None }],
            false,
        )
        .unwrap();

        // Any letter case; the task's own commit is told from the project's.
        let found = find_commits(&dir, Some("main"), "LIMIT", 10);
        let lines: Vec<_> = found.iter().map(|c| (c.subject.as_str(), c.on_branch)).collect();
        assert_eq!(lines, vec![("Limit login attempts to five a minute", true)], "a match in the body alone is no hit");
        assert_eq!((found[0].hash.as_str(), found[0].author.as_str()), (made.hash.as_str(), "Test"));
        let project: Vec<_> =
            find_commits(&dir, Some("main"), "s", 10).into_iter().map(|c| (c.subject, c.on_branch)).collect();
        assert!(project.contains(&("Say goodbye on main".into(), false)) && project.contains(&("Start".into(), false)));
        assert_eq!(find_commits(&dir, Some("main"), "s", 1).len(), 1, "the limit is respected");
        assert!(find_commits(&dir, Some("main"), "  ", 10).is_empty());
        assert!(find_commits(&dir, Some("main"), "--all", 10).is_empty(), "a query is never read as an option");

        // The first letters of an id find its commit.
        let by_id = find_commits(&dir, Some("main"), &made.short, 10);
        assert_eq!(by_id.iter().map(|c| c.hash.as_str()).collect::<Vec<_>>(), vec![made.hash.as_str()]);

        // The commit in full: its message, its files, and one file line by line.
        let shown = show_commit(&dir, Some("main"), &made.short).unwrap();
        assert_eq!(
            (shown.commit.subject.as_str(), shown.body.as_str()),
            (made.subject.as_str(), "Asked for in the task.")
        );
        assert!(shown.commit.on_branch);
        let files: Vec<_> = shown.files.iter().map(|f| (f.path.as_str(), f.kind, f.added, f.removed)).collect();
        assert_eq!(files, vec![("new file.txt", "added", 2, 0), ("src/long.txt", "modified", 2, 1)]);
        let diff = commit_file_diff(&dir, &made.hash, "src/long.txt").unwrap();
        assert_eq!((diff.hunks.len(), diff.hunks[0].from_line), (2, 2));
        assert!(commit_file_diff(&dir, &made.hash, "gone.txt").is_err());

        // The first commit of the repository has nothing before it: everything in it is new.
        let start = find_commits(&dir, Some("main"), "Start", 10);
        let first = show_commit(&dir, Some("main"), &start[0].hash).unwrap();
        assert!(!first.commit.on_branch);
        assert_eq!(first.files.iter().map(|f| f.kind).collect::<Vec<_>>(), vec!["added", "added"]);

        // Only an id is taken.
        assert!(show_commit(&dir, Some("main"), "HEAD").is_err());
        assert!(show_commit(&dir, Some("main"), "--all").is_err());
        assert!(show_commit(&dir, Some("main"), "0000000").is_err());

        // Looking for commits takes no lock and changes nothing.
        assert!(!dir.join(".git/index.lock").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_folder_without_commits_has_no_commits_to_find() {
        let dir = std::env::temp_dir().join(format!("ae-gitops-nocommits-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(find_commits(&dir, None, "anything", 10).is_empty());
        git(&dir, &["init", "-q", "-b", "main"]).unwrap();
        assert!(find_commits(&dir, None, "anything", 10).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn updates_from_the_base_and_backs_out_of_a_conflict() {
        let dir = repo("update");
        // The base gets a commit the task does not have.
        git(&dir, &["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("gone.txt"), "changed on main\n").unwrap();
        git(&dir, &["commit", "-q", "-am", "On main"]).unwrap();
        git(&dir, &["checkout", "-q", "task/one"]).unwrap();
        assert_eq!(state(&dir, Some("main")).unwrap().base_ahead, 1);

        update_from_base(&dir, "main").unwrap();
        assert_eq!(state(&dir, Some("main")).unwrap().base_ahead, 0);
        assert_eq!(std::fs::read_to_string(dir.join("gone.txt")).unwrap(), "changed on main\n");

        // Both sides change the same line: the update is refused and nothing moves.
        git(&dir, &["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("gone.txt"), "main again\n").unwrap();
        git(&dir, &["commit", "-q", "-am", "On main again"]).unwrap();
        git(&dir, &["checkout", "-q", "task/one"]).unwrap();
        std::fs::write(dir.join("gone.txt"), "the task's version\n").unwrap();
        git(&dir, &["commit", "-q", "-am", "On the task"]).unwrap();
        let refused = update_from_base(&dir, "main").unwrap_err().to_string();
        assert!(refused.contains("gone.txt") && refused.contains("Nothing was changed"), "{refused}");
        assert_eq!(std::fs::read_to_string(dir.join("gone.txt")).unwrap(), "the task's version\n");
        assert!(changes(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
