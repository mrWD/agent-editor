//! Reading the files of a task's folder for the interface.
//!
//! Every path the interface sends is relative to the task's folder and is
//! checked to stay inside it, so a crafted path cannot reach the rest of the disk.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The largest file the interface is given in one piece. A source file of
/// fifty thousand lines is about three megabytes; this leaves room above that.
const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    /// Path relative to the task's folder, with forward slashes on every system.
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileText {
    pub path: String,
    pub text: String,
    pub size: u64,
    /// A fingerprint of the contents. Sent back when saving, so a file that
    /// changed on disk in the meantime is not overwritten blindly.
    pub version: String,
}

/// A short fingerprint of file contents (FNV-1a, 64 bit). Not for security,
/// only to notice that a file is no longer what the editor loaded.
pub fn version_of(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Turns a relative path into a real one and refuses anything outside `root`.
pub fn resolve(root: &Path, relative: &str) -> Result<PathBuf> {
    let root = root.canonicalize().with_context(|| format!("No such folder: {}", root.display()))?;
    let joined = root.join(relative.trim_start_matches(['/', '\\']));
    let real = joined.canonicalize().with_context(|| format!("No such path: {relative}"))?;
    if !real.starts_with(&root) {
        bail!("That path is outside the task's folder");
    }
    Ok(real)
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Lists one folder: folders first, then files, each group by name.
pub fn list(root: &Path, relative: &str) -> Result<Vec<Entry>> {
    let canonical_root = root.canonicalize()?;
    let dir = resolve(root, relative)?;
    if !dir.is_dir() {
        bail!("Not a folder: {relative}");
    }
    let mut entries = Vec::new();
    for item in std::fs::read_dir(&dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        // The repository's own bookkeeping is never useful in the tree.
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        let path = item.path();
        entries.push(Entry { name, path: relative_path(&canonical_root, &path), is_dir: path.is_dir() });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(entries)
}

/// Reads one text file whole.
pub fn read_text(root: &Path, relative: &str) -> Result<FileText> {
    let path = resolve(root, relative)?;
    if !path.is_file() {
        bail!("Not a file: {relative}");
    }
    let size = std::fs::metadata(&path)?.len();
    if size > MAX_TEXT_BYTES {
        bail!("This file is too large to open here ({} MB)", size / (1024 * 1024));
    }
    let bytes = std::fs::read(&path)?;
    let version = version_of(&bytes);
    let text = String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("This is not a text file"))?;
    let canonical_root = root.canonicalize()?;
    Ok(FileText { path: relative_path(&canonical_root, &path), text, size, version })
}

/// Where a file that is about to be written really is. The file may not exist
/// yet, but its folder must, and both must be inside `root`.
fn resolve_for_write(root: &Path, relative: &str) -> Result<PathBuf> {
    let root = root.canonicalize().with_context(|| format!("No such folder: {}", root.display()))?;
    let joined = root.join(relative.trim_start_matches(['/', '\\']));
    let name = joined.file_name().context("Give a file name")?.to_owned();
    let parent = joined.parent().context("Give a file name")?;
    let real_parent = parent.canonicalize().with_context(|| format!("No such folder for {relative}"))?;
    if !real_parent.starts_with(&root) {
        bail!("That path is outside the task's folder");
    }
    let target = real_parent.join(name);
    if target.exists() {
        // An existing name may be a link that points elsewhere.
        let real = target.canonicalize()?;
        if !real.starts_with(&root) {
            bail!("That path is outside the task's folder");
        }
        if real.is_dir() {
            bail!("That is a folder, not a file: {relative}");
        }
    }
    Ok(target)
}

/// Saves a text file. When `expected_version` is given and the file on disk is
/// no longer that version, nothing is written and the caller is told.
pub fn write_text(root: &Path, relative: &str, text: &str, expected_version: Option<&str>) -> Result<FileText> {
    let target = resolve_for_write(root, relative)?;
    if let Some(expected) = expected_version
        && target.exists()
    {
        let on_disk = version_of(&std::fs::read(&target)?);
        if on_disk != expected {
            bail!("This file changed on disk since you opened it. Reload it before saving.");
        }
    }
    std::fs::write(&target, text).with_context(|| format!("Could not save {relative}"))?;
    let canonical_root = root.canonicalize()?;
    Ok(FileText {
        path: relative_path(&canonical_root, &target),
        text: text.to_string(),
        size: text.len() as u64,
        version: version_of(text.as_bytes()),
    })
}

/// Where a new file or folder goes. Unlike a save, the folders on the way may
/// not exist yet; the deepest one that does must be inside `root`.
fn resolve_new(root: &Path, relative: &str) -> Result<PathBuf> {
    let root = root.canonicalize().with_context(|| format!("No such folder: {}", root.display()))?;
    let relative = relative.trim().trim_matches(['/', '\\']);
    if relative.is_empty() {
        bail!("Give a name");
    }
    let mut target = root.clone();
    for part in relative.split(['/', '\\']) {
        if part.is_empty() || part == "." || part == ".." {
            bail!("A name cannot have empty parts, \".\" or \"..\": {relative}");
        }
        target.push(part);
    }
    // Links on the way must not lead out of the task's folder.
    let mut existing = target.as_path();
    while !existing.exists() {
        existing = existing.parent().context("Give a name")?;
    }
    if !existing.canonicalize()?.starts_with(&root) {
        bail!("That path is outside the task's folder");
    }
    Ok(target)
}

/// Every file under `path` (or `path` itself when it is a file), relative to `root`.
/// What git keeps for itself is left out.
fn files_under(root: &Path, path: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut waiting = vec![path.to_path_buf()];
    while let Some(next) = waiting.pop() {
        if next.is_dir() && !next.is_symlink() {
            if next.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(&next) {
                waiting.extend(entries.flatten().map(|entry| entry.path()));
            }
        } else {
            found.push(relative_path(root, &next));
        }
    }
    found.sort();
    found
}

/// Every file of a file or folder of the task, relative to the task's folder.
pub fn files_of(root: &Path, relative: &str) -> Result<Vec<String>> {
    let source = resolve(root, relative)?;
    Ok(files_under(&root.canonicalize()?, &source))
}

/// Makes an empty file, or a folder, with the folders on the way to it.
/// Returns its path as the task sees it.
pub fn create(root: &Path, relative: &str, folder: bool) -> Result<String> {
    let target = resolve_new(root, relative)?;
    if target.exists() {
        bail!("There is already something called {relative}");
    }
    if folder {
        std::fs::create_dir_all(&target)?;
    } else {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::OpenOptions::new().write(true).create_new(true).open(&target)?;
    }
    Ok(relative_path(&root.canonicalize()?, &target))
}

/// Moves a file or folder to a new name. Returns the files that moved, as (from, to).
pub fn rename(root: &Path, from: &str, to: &str) -> Result<Vec<(String, String)>> {
    let canonical_root = root.canonicalize()?;
    let source = resolve(root, from)?;
    if source == canonical_root {
        bail!("The task's folder itself cannot be renamed");
    }
    let target = resolve_new(root, to)?;
    if target.exists() {
        bail!("There is already something called {to}");
    }
    let before = files_under(&canonical_root, &source);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&source, &target).with_context(|| format!("Could not rename {from}"))?;
    let (old, new) = (relative_path(&canonical_root, &source), relative_path(&canonical_root, &target));
    Ok(before
        .into_iter()
        .map(|path| {
            let moved = format!("{new}{}", &path[old.len()..]);
            (path, moved)
        })
        .collect())
}

/// Takes a file or folder out of the task and keeps it in `trash`, so it can
/// be put back. Returns the files that went, and where the thing is kept.
pub fn remove(root: &Path, relative: &str, trash: &Path) -> Result<(Vec<String>, PathBuf)> {
    let canonical_root = root.canonicalize()?;
    let source = resolve(root, relative)?;
    if source == canonical_root {
        bail!("The task's folder itself cannot be deleted");
    }
    if source.strip_prefix(&canonical_root).is_ok_and(|inside| inside.starts_with(".git")) {
        bail!("That belongs to git and is left alone");
    }
    let files = files_under(&canonical_root, &source);
    std::fs::create_dir_all(trash)?;
    let kept = trash.join(source.file_name().context("Give a name")?);
    move_across(&source, &kept).with_context(|| format!("Could not delete {relative}"))?;
    Ok((files, kept))
}

/// Puts back what `remove` took out, if its place is still free.
pub fn restore(root: &Path, relative: &str, kept: &Path) -> Result<Vec<String>> {
    let target = resolve_new(root, relative)?;
    if target.exists() {
        bail!("There is something called {relative} again, so the deleted one cannot be put back in its place");
    }
    if !kept.exists() {
        bail!("What was deleted is no longer kept");
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    move_across(kept, &target)?;
    Ok(files_under(&root.canonicalize()?, &target))
}

/// Renames, or copies and removes when the two places are on different disks.
fn move_across(from: &Path, to: &Path) -> Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_all(from, to)?;
    if from.is_dir() {
        std::fs::remove_dir_all(from)?
    } else {
        std::fs::remove_file(from)?
    }
    Ok(())
}

fn copy_all(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() && !from.is_symlink() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ae-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/nested")).unwrap();
        std::fs::write(dir.join("src/b.txt"), "second").unwrap();
        std::fs::write(dir.join("src/A.txt"), "first").unwrap();
        std::fs::write(dir.join("readme.md"), "hello").unwrap();
        dir
    }

    #[test]
    fn lists_folders_first_then_files_by_name() {
        let root = sandbox("list");
        let names: Vec<_> = list(&root, "src").unwrap().into_iter().map(|e| (e.name, e.path, e.is_dir)).collect();
        assert_eq!(
            names,
            vec![
                ("nested".to_string(), "src/nested".to_string(), true),
                ("A.txt".to_string(), "src/A.txt".to_string(), false),
                ("b.txt".to_string(), "src/b.txt".to_string(), false),
            ]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_a_text_file() {
        let root = sandbox("read");
        let file = read_text(&root, "readme.md").unwrap();
        assert_eq!(file.text, "hello");
        assert_eq!(file.path, "readme.md");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_paths_outside_the_folder() {
        let root = sandbox("escape");
        let outside = root.parent().unwrap().join(format!("ae-outside-{}.txt", std::process::id()));
        std::fs::write(&outside, "secret").unwrap();
        let name = outside.file_name().unwrap().to_string_lossy().into_owned();

        let error = read_text(&root, &format!("../{name}")).unwrap_err().to_string();
        assert!(error.contains("outside"), "{error}");
        assert!(list(&root, "..").is_err());

        std::fs::remove_file(outside).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saves_a_file_and_notices_a_change_on_disk() {
        let root = sandbox("write");
        let opened = read_text(&root, "readme.md").unwrap();

        let saved = write_text(&root, "readme.md", "hello again", Some(&opened.version)).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("readme.md")).unwrap(), "hello again");
        assert_ne!(saved.version, opened.version);

        // Someone else changes the file; saving with the old version is refused.
        std::fs::write(root.join("readme.md"), "changed elsewhere").unwrap();
        let refused = write_text(&root, "readme.md", "mine", Some(&saved.version)).unwrap_err().to_string();
        assert!(refused.contains("changed on disk"), "{refused}");
        assert_eq!(std::fs::read_to_string(root.join("readme.md")).unwrap(), "changed elsewhere");

        // A new file in an existing folder is fine; outside the folder is not.
        write_text(&root, "src/new.txt", "fresh", None).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("src/new.txt")).unwrap(), "fresh");
        assert!(write_text(&root, "../escape.txt", "x", None).unwrap_err().to_string().contains("outside"));
        assert!(write_text(&root, "src", "x", None).is_err(), "a folder is not a file");
        assert!(write_text(&root, "missing/file.txt", "x", None).is_err(), "the folder must exist");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opens_a_long_source_file_and_refuses_a_huge_one() {
        let root = sandbox("large");
        let long: String =
            (0..50_000).map(|n| format!("export const line{n} = {n}; // filler text for line number {n}\n")).collect();
        assert!(long.len() > 2 * 1024 * 1024);
        std::fs::write(root.join("long.ts"), &long).unwrap();
        assert_eq!(read_text(&root, "long.ts").unwrap().text.len(), long.len());

        std::fs::write(root.join("huge.log"), vec![b'x'; 17 * 1024 * 1024]).unwrap();
        let error = read_text(&root, "huge.log").unwrap_err().to_string();
        assert!(error.contains("too large"), "{error}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_files_that_are_not_text() {
        let root = sandbox("binary");
        std::fs::write(root.join("image.bin"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let error = read_text(&root, "image.bin").unwrap_err().to_string();
        assert!(error.contains("not a text file"), "{error}");
        std::fs::remove_dir_all(root).unwrap();
    }
}
