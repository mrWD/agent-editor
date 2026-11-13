//! The Map view of a task: pictures of the code the task changed.
//!
//! Three of them are worked out here by reading the files. No agent is asked
//! and no tokens are spent:
//!
//! - the tree: the files the task changed and their near neighbours (the files
//!   they import and the files that import them), each with what it defines;
//! - dependencies: what the file at the centre needs, two steps out, and who needs it;
//! - calls: who uses the function at the centre and what it uses, two steps out.
//!
//! The fourth picture, the flow, takes an understanding of the code, so the
//! task's agent draws it; the interface asks for it and reads the answer.
//!
//! The command is `map.get` with `{ taskId, center?: { path, symbol? } }`. Its
//! answer is a [`CodeMap`]. Reading is kept cheap: only the changed files and
//! their neighbours are read in full, every other file is at most searched for
//! the names of the changed files, and what was learnt about a file is kept
//! until the file changes.
//!
//! Like `changes` and `review`, everything here blocks. Call it from `spawn_blocking`.

mod go;
mod js;
mod lex;
mod python;
mod resolve;
mod rust;
mod scan;

use crate::gitops;
use crate::tasks::Manager;
use anyhow::{Context, Result, bail};
use ignore::{WalkBuilder, WalkState};
use resolve::{Resolver, Target};
use scan::{Binding, Def, Facts, Lang};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

/// Files larger than this are listed but not read.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// How many changed files one map covers.
const MAX_CHANGED: usize = 200;
/// How many files in all the tree lists.
const MAX_TREE: usize = 400;
/// How many files that mention a changed file are read to see whether they import it.
const MAX_MENTIONS: usize = 3000;
/// How many files that import one changed file are kept.
const MAX_IMPORTERS: usize = 24;
/// How many boxes stand on the left of the centre in a picture.
const MAX_BEFORE: usize = 8;
/// How many boxes stand right after the centre.
const MAX_AFTER: usize = 12;
/// How many boxes one box of the first step may lead to, and how many the second step has in all.
const MAX_PER_BOX: usize = 4;
const MAX_SECOND: usize = 16;
/// How many definitions stand at the centre when a whole file is picked.
const MAX_MIDDLE: usize = 6;
/// How many files the map remembers between calls before it starts over.
const MAX_REMEMBERED: usize = 200_000;

// ----- what the interface gets -----

/// What a map can be centred on: a changed file, or one changed definition in it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Center {
    pub path: String,
    /// `None`: the whole file.
    pub symbol: Option<String>,
    /// What the head line shows after "Centered on".
    pub label: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TreeDef {
    pub name: String,
    /// A plain word: "function", "const", "class", "route" and so on.
    pub kind: &'static str,
    pub line: u32,
    pub changed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TreeFile {
    pub path: String,
    pub changed: bool,
    /// The language as shown, or `None` when the map does not read this kind of
    /// file. Such a file is listed without details.
    pub language: Option<&'static str>,
    pub defs: Vec<TreeDef>,
    pub tests: usize,
}

/// One box of a picture.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub label: String,
    /// "file", "function" (anything a project file defines), "package" (from
    /// outside the project) or "other" (a call the map cannot place).
    pub kind: &'static str,
    /// Where a click leads: the file and line of the thing itself, or, for a
    /// package or a call the map cannot place, the line that names it.
    pub path: Option<String>,
    pub line: Option<u32>,
    pub changed: bool,
    /// How many steps from the centre: negative is before it, 0 is the centre.
    pub layer: i32,
    /// A sentence for the tooltip and for screen readers.
    pub title: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// How many boxes were left out to keep the picture readable.
    pub hidden: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CodeMap {
    pub center: Option<Center>,
    /// Everything the centre can be moved to.
    pub centers: Vec<Center>,
    pub tree: Vec<TreeFile>,
    pub deps: Graph,
    pub calls: Graph,
    /// Plain sentences about what the map could not cover.
    pub notes: Vec<String>,
}

// ----- what the task changed -----

/// Which lines of a file the task changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lines {
    /// The file is new, or not text: all of it counts.
    All,
    /// Stretches of lines, first and last, counted from 1 in the file as it is now.
    Some(Vec<(u32, u32)>),
}

impl Lines {
    fn touch(&self, from: u32, to: u32) -> bool {
        match self {
            Lines::All => true,
            Lines::Some(ranges) => ranges.iter().any(|(first, last)| *first <= to && *last >= from),
        }
    }
}

/// The files a task changed, by path from the task's folder.
pub type Changed = BTreeMap<String, Lines>;

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        // Paths as they are, not escaped.
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .context("Could not run git. Is it installed?")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Reads which lines changed out of what `git diff -U0` printed. Deleted files are left out.
fn parse_patch(text: &str) -> Changed {
    let mut changed = Changed::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // "a/path b/path". The `+++` line below is the better source; this is for files without one.
            current = rest.split(" b/").last().map(str::to_string);
            if let Some(path) = &current {
                changed.insert(path.clone(), Lines::Some(vec![]));
            }
        } else if let Some(path) = line.strip_prefix("+++ b/") {
            let path = path.trim_end_matches('\t').to_string();
            if current.as_deref() != Some(&path) {
                if let Some(old) = current.take() {
                    let lines = changed.remove(&old).unwrap_or(Lines::Some(vec![]));
                    changed.insert(path.clone(), lines);
                }
                current = Some(path);
            }
        } else if line.starts_with("deleted file mode") {
            // A file that is gone has nothing to draw.
            if let Some(path) = current.take() {
                changed.remove(&path);
            }
        } else if line.starts_with("new file mode") || line.starts_with("Binary files ") {
            if let Some(path) = &current {
                changed.insert(path.clone(), Lines::All);
            }
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            // "@@ -3,2 +3,4 @@": four lines from line 3 of the file as it is now.
            let Some(new) = rest.split(' ').find_map(|part| part.strip_prefix('+')) else { continue };
            let mut numbers = new.split(',');
            let start: u32 = numbers.next().and_then(|n| n.parse().ok()).unwrap_or(1);
            let count: u32 = numbers.next().and_then(|n| n.parse().ok()).unwrap_or(1);
            // Lines that were only taken out leave a mark on the line they stood after.
            let range = if count == 0 { (start.max(1), start.max(1)) } else { (start, start + count - 1) };
            if let Some(Lines::Some(ranges)) = current.as_ref().and_then(|path| changed.get_mut(path)) {
                ranges.push(range);
            }
        }
    }
    changed
}

/// Everything a task's branch changed since it left its base: its commits and
/// what is not committed, files git does not know yet included.
pub fn changed_in_git(root: &Path, remembered_base: Option<&str>) -> Result<Changed> {
    let base = gitops::resolve_base(root, remembered_base);
    let start = base
        .as_deref()
        .and_then(|base| git(root, &["merge-base", "HEAD", base]).ok())
        .map(|commit| commit.trim().to_string())
        .filter(|commit| !commit.is_empty())
        // Without a base only what is not committed is known to be the task's.
        .unwrap_or_else(|| "HEAD".to_string());
    let patch = git(root, &["diff", &start, "--no-color", "--no-ext-diff", "--no-renames", "-U0", "--"])?;
    let mut changed = parse_patch(&patch);
    let status = git(root, &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--no-renames"])?;
    for entry in status.split('\0') {
        if let Some(path) = entry.strip_prefix("?? ") {
            changed.insert(path.to_string(), Lines::All);
        }
    }
    Ok(changed)
}

impl Manager {
    /// What a task changed in a folder without git: the files and lines the review marks hold.
    fn changed_by_marks(&self, task_id: &str) -> Result<Changed> {
        let mut changed = Changed::new();
        for file in self.changed_files(task_id)? {
            if !file.exists {
                continue;
            }
            let lines = if file.existed {
                let runs = self.review_of(task_id, &file.path)?.runs;
                Lines::Some(runs.iter().map(|run| (run.from as u32, run.to as u32)).collect())
            } else {
                Lines::All
            };
            changed.insert(file.path, lines);
        }
        Ok(changed)
    }

    /// The map of a task. `wanted` is the centre the interface asks for:
    /// `{ "path": ..., "symbol": ... }`. Without it, or when it is no longer
    /// there, the map picks the first changed function.
    pub fn map_of(&self, task_id: &str, wanted: Option<&Value>) -> Result<Value> {
        let task = self.store().task(task_id)?;
        let root = PathBuf::from(&task.cwd);
        let changed = match &task.branch {
            Some(_) => changed_in_git(&root, task.base_branch.as_deref())?,
            None => self.changed_by_marks(task_id)?,
        };
        let wanted = wanted.and_then(|value| {
            let path = value.get("path")?.as_str()?;
            Some((path, value.get("symbol").and_then(Value::as_str)))
        });
        Ok(json!(build(&root, &changed, wanted)))
    }
}

// ----- reading files, and remembering what was read -----

enum Remembered {
    /// The file was read. `None`: it cannot be read as text.
    Read(Option<Arc<Facts>>),
    /// The file was searched and does not mention the names with this fingerprint.
    Silent(u64),
}

struct Entry {
    modified: Option<SystemTime>,
    size: u64,
    what: Remembered,
}

/// What is known about files, by full path, for as long as a file's time and
/// size stay the same. Shared by all tasks: a file reads the same for each.
static MEMORY: LazyLock<Mutex<HashMap<PathBuf, Entry>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn memory() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Entry>> {
    // A panic elsewhere while holding the lock does not make the entries wrong.
    let mut memory = MEMORY.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if memory.len() > MAX_REMEMBERED {
        memory.clear();
    }
    memory
}

fn read_text(full: &Path, size: u64) -> Option<String> {
    if size > MAX_FILE_BYTES {
        return None;
    }
    String::from_utf8(std::fs::read(full).ok()?).ok()
}

/// What a file defines, imports and uses. `None` when the map does not read
/// its language, or the file is missing, too large or not text.
fn facts_of(root: &Path, path: &str) -> Option<Arc<Facts>> {
    Lang::of(path)?;
    let full = root.join(path);
    let meta = std::fs::metadata(&full).ok().filter(|meta| meta.is_file())?;
    let (modified, size) = (meta.modified().ok(), meta.len());
    if let Some(entry) = memory().get(&full)
        && entry.modified == modified
        && entry.size == size
        && let Remembered::Read(facts) = &entry.what
    {
        return facts.clone();
    }
    let facts = read_text(&full, size).and_then(|text| scan::scan(path, &text)).map(Arc::new);
    memory().insert(full, Entry { modified, size, what: Remembered::Read(facts.clone()) });
    facts
}

/// The names of the changed files, as other files would write them in an import.
#[derive(Debug, Default)]
struct Needles {
    by_lang: HashMap<Lang, Vec<String>>,
    fingerprint: u64,
}

impl Needles {
    fn add(&mut self, lang: Lang, needle: String) {
        let list = self.by_lang.entry(lang).or_default();
        if !list.contains(&needle) {
            list.push(needle);
        }
    }

    fn seal(&mut self) {
        let mut all: Vec<(u8, &String)> =
            self.by_lang.iter().flat_map(|(lang, list)| list.iter().map(|needle| (*lang as u8, needle))).collect();
        all.sort();
        let mut hasher = DefaultHasher::new();
        all.hash(&mut hasher);
        self.fingerprint = hasher.finish();
    }
}

/// Whether `text` names one of `needles` the way an import would: right after
/// a slash or a quote in JavaScript, after a dot or a space in Python, and so
/// on. A cheap test that spares most files a full reading.
fn mentions(text: &str, lang: Lang, needles: &[String]) -> bool {
    let bytes = text.as_bytes();
    needles.iter().any(|needle| {
        text.match_indices(needle.as_str()).any(|(at, _)| {
            let before = if at == 0 { b'\n' } else { bytes[at - 1] };
            let after = bytes.get(at + needle.len()).copied().unwrap_or(b'\n');
            let starts = match lang {
                Lang::Js => matches!(before, b'/' | b'\'' | b'"' | b'`'),
                Lang::Python => matches!(before, b' ' | b'.' | b',' | b'(' | b'\t' | b'\n'),
                Lang::Rust => matches!(before, b':' | b' ' | b'{' | b',' | b'(' | b'\t' | b'\n'),
                Lang::Go => before == b'"',
            };
            starts && !(after.is_ascii_alphanumeric() || after == b'_')
        })
    })
}

/// The same question asked of a file that was read before: do its imports name one of `needles`?
fn imports_mention(facts: &Facts, needles: &[String]) -> bool {
    facts.imports.iter().any(|import| {
        needles.iter().any(|needle| {
            import.spec.contains(needle.as_str())
                || import.bindings.iter().any(|b| b.local == *needle || b.item.as_deref() == Some(needle.as_str()))
        })
    })
}

/// Whether the file at `full` is worth a closer look as an importer. Reads it
/// only when its time or size changed since the last look.
fn may_import(full: &Path, path: &str, meta: &std::fs::Metadata, lang: Lang, needles: &Needles) -> bool {
    let Some(list) = needles.by_lang.get(&lang) else { return false };
    let (modified, size) = (meta.modified().ok(), meta.len());
    if let Some(entry) = memory().get(full)
        && entry.modified == modified
        && entry.size == size
    {
        match &entry.what {
            Remembered::Read(Some(facts)) => return imports_mention(facts, list),
            Remembered::Read(None) => return false,
            Remembered::Silent(fingerprint) if *fingerprint == needles.fingerprint => return false,
            Remembered::Silent(_) => {}
        }
    }
    let Some(text) = read_text(full, size) else {
        memory().insert(full.to_path_buf(), Entry { modified, size, what: Remembered::Read(None) });
        return false;
    };
    if !mentions(&text, lang, list) {
        let what = Remembered::Silent(needles.fingerprint);
        memory().insert(full.to_path_buf(), Entry { modified, size, what });
        return false;
    }
    let facts = scan::scan(path, &text).map(Arc::new);
    let found = facts.as_ref().is_some_and(|facts| imports_mention(facts, list));
    memory().insert(full.to_path_buf(), Entry { modified, size, what: Remembered::Read(facts) });
    found
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Walks the project the way git sees it and returns the files that may
/// import one of the changed files. The flag says the walk stopped early.
fn files_that_mention(root: &Path, needles: &Needles) -> (Vec<String>, bool) {
    let found = Mutex::new(Vec::new());
    let count = AtomicUsize::new(0);
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false);
    builder.build_parallel().run(|| {
        Box::new(|entry| {
            if count.load(Ordering::Relaxed) >= MAX_MENTIONS {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                return WalkState::Continue;
            }
            // The ending decides first: most files of a project are not code of the changed languages.
            let name = entry.file_name().to_string_lossy();
            let Some((lang, _)) = Lang::of(&name) else { return WalkState::Continue };
            if !needles.by_lang.contains_key(&lang) {
                return WalkState::Continue;
            }
            let Ok(meta) = entry.metadata() else { return WalkState::Continue };
            let path = relative(root, entry.path());
            if may_import(entry.path(), &path, &meta, lang, needles) {
                found.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(path);
                count.fetch_add(1, Ordering::Relaxed);
            }
            WalkState::Continue
        })
    });
    let stopped = count.load(Ordering::Relaxed) >= MAX_MENTIONS;
    let mut list = found.into_inner().unwrap_or_else(|poisoned| poisoned.into_inner());
    list.sort();
    (list, stopped)
}

// ----- what names mean -----

/// Where a name a file uses comes from.
#[derive(Debug, Clone)]
enum Bound {
    /// A whole file of the project: a module, a namespace.
    Module(String),
    /// One name of a file of the project.
    Item(String, String),
    /// A folder of the project that is one Go package.
    Folder(String),
    Package(String),
    Builtin,
}

/// Something a file needs in order to work.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Need {
    File(String),
    Package(String),
    /// An import the map cannot place, as written.
    Other(String),
}

/// What a file brings in: the names, and the files and packages behind them with the line that asks for each.
#[derive(Debug, Default)]
struct Scope {
    names: HashMap<String, Bound>,
    needs: Vec<(Need, u32)>,
}

impl Scope {
    fn need(&mut self, need: Need, line: u32) {
        if !self.needs.iter().any(|(known, _)| *known == need) {
            self.needs.push((need, line));
        }
    }

    fn needs_file(&self, path: &str) -> bool {
        self.needs.iter().any(|(need, _)| matches!(need, Need::File(file) if file == path))
    }
}

/// What a name used in a file turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Meaning {
    /// Something a file of the project defines.
    Def {
        path: String,
        name: String,
    },
    /// A file of the project, when the name in it was not found.
    File(String),
    Package(String),
    /// A part of the language.
    Builtin,
    Unknown,
}

fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn builtin(lang: Lang, name: &str) -> bool {
    match lang {
        Lang::Js => js::builtin(name),
        Lang::Rust => rust::builtin(name),
        Lang::Python => python::builtin(name),
        Lang::Go => go::builtin(name),
    }
}

/// The name of a changed file as a file that imports it would write it.
fn needle_of(resolver: &mut Resolver, lang: Lang, path: &str) -> Option<String> {
    let name = base_name(path);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let folder = base_name(folder_of(path));
    let needle = match lang {
        // `index.ts` is imported by the name of its folder; `x.d.ts` describes `x`.
        Lang::Js if stem == "index" => folder,
        Lang::Js => stem.strip_suffix(".d").unwrap_or(stem),
        Lang::Python if stem == "__init__" => folder,
        Lang::Rust if stem == "mod" => folder,
        // The first file of a crate is named by every path in it; a search for that would find everything.
        Lang::Rust if matches!(stem, "lib" | "main") => "",
        Lang::Python | Lang::Rust => stem,
        Lang::Go => return resolver.go_import_path(folder_of(path)),
    };
    (!needle.is_empty()).then(|| needle.to_string())
}

/// The files that import each changed file.
type Importers = BTreeMap<String, Vec<String>>;

/// A box for something a definition uses and, when the project defines it, where: its file and itself.
type Used = (Node, Option<(String, Def)>);

/// One project while one map is made: reads files once and remembers what names mean.
struct Project<'a> {
    root: &'a Path,
    resolver: Resolver<'a>,
    facts: HashMap<String, Option<Arc<Facts>>>,
    scopes: HashMap<String, Arc<Scope>>,
    go_folders: HashMap<String, Arc<Vec<String>>>,
}

impl<'a> Project<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            resolver: Resolver::new(root),
            facts: HashMap::new(),
            scopes: HashMap::new(),
            go_folders: HashMap::new(),
        }
    }

    fn facts(&mut self, path: &str) -> Option<Arc<Facts>> {
        if let Some(known) = self.facts.get(path) {
            return known.clone();
        }
        let found = facts_of(self.root, path);
        self.facts.insert(path.to_string(), found.clone());
        found
    }

    /// The Go files of one folder: together they are one package.
    fn go_files(&mut self, folder: &str) -> Arc<Vec<String>> {
        if let Some(known) = self.go_folders.get(folder) {
            return known.clone();
        }
        let mut files: Vec<String> = std::fs::read_dir(self.root.join(folder))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".go"))
            .map(|name| if folder.is_empty() { name } else { format!("{folder}/{name}") })
            .collect();
        files.sort();
        let files = Arc::new(files);
        self.go_folders.insert(folder.to_string(), files.clone());
        files
    }

    /// The file of a Go package that defines `name`, other than `except`.
    fn go_definer(&mut self, folder: &str, name: &str, except: Option<&str>) -> Option<String> {
        let files = self.go_files(folder);
        files
            .iter()
            .filter(|file| Some(file.as_str()) != except)
            .find(|file| self.facts(file).is_some_and(|facts| facts.def(name).is_some()))
            .cloned()
    }

    /// Notes what one import leads to: the name it brings in, and the file or package it needs.
    fn bind(
        scope: &mut Scope,
        own: &str,
        lang: Lang,
        target: Target,
        binding: Option<&Binding>,
        spec: &str,
        line: u32,
    ) {
        let bound = match target {
            Target::File { path, item } => {
                if path != own {
                    scope.need(Need::File(path.clone()), line);
                }
                // JavaScript says which name it wants in the import itself; the others say it in the path.
                let wanted = if lang == Lang::Js { binding.and_then(|b| b.item.clone()) } else { item };
                match wanted {
                    Some(item) => Bound::Item(path, item),
                    None => Bound::Module(path),
                }
            }
            Target::Folder(folder) => Bound::Folder(folder),
            Target::Package(name) => {
                scope.need(Need::Package(name.clone()), line);
                Bound::Package(name)
            }
            Target::Builtin => Bound::Builtin,
            Target::Unknown => {
                scope.need(Need::Other(spec.to_string()), line);
                return;
            }
        };
        if let Some(binding) = binding {
            scope.names.insert(binding.local.clone(), bound);
        }
    }

    /// What a file brings in. Worked out once per file and map.
    fn scope(&mut self, path: &str) -> Arc<Scope> {
        if let Some(known) = self.scopes.get(path) {
            return known.clone();
        }
        let mut scope = Scope::default();
        if let (Some((lang, _)), Some(facts)) = (Lang::of(path), self.facts(path)) {
            // Go: which line imports each package folder, for the files found in it later.
            let mut folder_lines: HashMap<String, u32> = HashMap::new();
            for import in &facts.imports {
                if lang == Lang::Rust {
                    // `use Kind::*` after `enum Kind` or `use ...::Kind` names something already here.
                    let first = import.spec.split("::").next().unwrap_or_default();
                    if facts.def(first).is_some() || scope.names.contains_key(first) {
                        continue;
                    }
                }
                if lang == Lang::Python && import.bindings.iter().any(|binding| binding.item.is_some()) {
                    // `from app import users`: each name may be a module with a file of its own.
                    for binding in &import.bindings {
                        let target = self.resolver.python(path, &import.spec, binding.item.as_deref());
                        Self::bind(&mut scope, path, lang, target, Some(binding), &import.spec, import.line);
                    }
                    continue;
                }
                let target = self.resolver.resolve(lang, path, &import.spec);
                if let Target::Folder(folder) = &target {
                    folder_lines.entry(folder.clone()).or_insert(import.line);
                }
                if import.bindings.is_empty() {
                    Self::bind(&mut scope, path, lang, target, None, &import.spec, import.line);
                } else {
                    for binding in &import.bindings {
                        Self::bind(&mut scope, path, lang, target.clone(), Some(binding), &import.spec, import.line);
                    }
                }
            }
            if lang == Lang::Go {
                self.go_needs(path, &facts, &folder_lines, &mut scope);
            }
        }
        let scope = Arc::new(scope);
        self.scopes.insert(path.to_string(), scope.clone());
        scope
    }

    /// Go imports packages, not files, and files of one package see each other
    /// without any import. So a Go file needs the files that define the names it uses.
    fn go_needs(&mut self, path: &str, facts: &Facts, folder_lines: &HashMap<String, u32>, scope: &mut Scope) {
        let own_folder = folder_of(path).to_string();
        for used in facts.defs.iter().flat_map(|def| &def.uses).chain(&facts.loose) {
            let mut parts = used.chain.split('.');
            let (Some(first), second) = (parts.next(), parts.next()) else { continue };
            if first.is_empty() {
                continue;
            }
            let found = match scope.names.get(first).cloned() {
                Some(Bound::Folder(folder)) => {
                    let line = folder_lines.get(&folder).copied().unwrap_or(used.line);
                    second.and_then(|name| self.go_definer(&folder, name, None)).map(|file| (file, line))
                }
                Some(_) => None,
                // A name of the package wins over a name of the language: a package may have its own `max`.
                None if facts.def(first).is_none() => {
                    self.go_definer(&own_folder, first, Some(path)).map(|file| (file, used.line))
                }
                None => None,
            };
            if let Some((file, line)) = found {
                scope.need(Need::File(file), line);
            }
        }
    }

    /// What `name` means inside `file`. `more` is what follows it in a chain: `name.more`.
    fn inside(&mut self, file: &str, name: Option<&str>, more: &[&str]) -> Meaning {
        let Some(facts) = self.facts(file) else { return Meaning::File(file.to_string()) };
        let def = |name: String| facts.def(&name).map(|_| Meaning::Def { path: file.to_string(), name });
        let (name, more) = match (name, more.split_first()) {
            (Some(name), _) => (name, more),
            // A whole module: the first word after it is the name looked for.
            (None, Some((first, rest))) => (*first, rest),
            (None, None) => return Meaning::File(file.to_string()),
        };
        // `Type.method` and `Type::method` first, then the thing itself.
        if let Some(next) = more.first()
            && let Some(found) = def(format!("{name}::{next}")).or_else(|| def(format!("{name}.{next}")))
        {
            return found;
        }
        if let Some(found) = def(name.to_string()) {
            return found;
        }
        // `Manager::new` given whole: the type, when the function is not found.
        if let Some((head, _)) = name.split_once("::")
            && let Some(found) = def(head.to_string())
        {
            return found;
        }
        Meaning::File(file.to_string())
    }

    fn through(&mut self, bound: &Bound, more: &[&str]) -> Meaning {
        match bound {
            Bound::Module(file) => self.inside(file, None, more),
            Bound::Item(file, item) => self.inside(file, Some(item), more),
            Bound::Folder(folder) => match more.split_first() {
                Some((name, rest)) => match self.go_definer(folder, name, None) {
                    Some(file) => self.inside(&file, Some(name), rest),
                    None => Meaning::Unknown,
                },
                None => Meaning::Unknown,
            },
            Bound::Package(name) => Meaning::Package(name.clone()),
            Bound::Builtin => Meaning::Builtin,
        }
    }

    /// What a name used in `path` means. `owner` is the type whose method uses it, for `this` and `self`.
    fn meaning(&mut self, path: &str, owner: Option<&str>, chain: &str) -> Meaning {
        let (Some((lang, _)), Some(facts)) = (Lang::of(path), self.facts(path)) else { return Meaning::Unknown };
        // A method called on the result of something else: there is no name to look up.
        if chain.starts_with('.') {
            return Meaning::Unknown;
        }
        let parts: Vec<&str> = chain.split("::").flat_map(|part| part.split('.')).collect();
        let first = parts[0];
        if matches!(first, "this" | "self" | "Self" | "cls") {
            if let (Some(owner), Some(method)) = (owner, parts.get(1)) {
                for name in [format!("{owner}::{method}"), format!("{owner}.{method}")] {
                    if facts.def(&name).is_some() {
                        return Meaning::Def { path: path.to_string(), name };
                    }
                }
            }
            return Meaning::Unknown;
        }
        if lang == Lang::Rust && matches!(first, "crate" | "super") {
            // A path written in full. A method called on what it names is not part of the path.
            let spec = chain.split('.').next().unwrap_or(chain);
            return match self.resolver.resolve(Lang::Rust, path, spec) {
                Target::File { path: file, item } => self.inside(&file, item.as_deref(), &[]),
                Target::Package(name) => Meaning::Package(name),
                Target::Builtin => Meaning::Builtin,
                _ => Meaning::Unknown,
            };
        }
        // Something this file defines itself.
        if facts.def(first).is_some() {
            return self.inside(path, Some(first), &parts[1..]);
        }
        // Something it imports. Python's `import a.b.c` makes the dotted name itself usable.
        let scope = self.scope(path);
        for take in (1..=parts.len()).rev() {
            if let Some(bound) = scope.names.get(&parts[..take].join(".")) {
                return self.through(&bound.clone(), &parts[take..]);
            }
        }
        if lang == Lang::Go
            && let Some(file) = self.go_definer(folder_of(path), first, Some(path))
        {
            return self.inside(&file, Some(first), &parts[1..]);
        }
        if builtin(lang, first) { Meaning::Builtin } else { Meaning::Unknown }
    }

    /// Finds the files that import each of `targets`. Only files of the same
    /// languages are looked at, and only those that mention a target by name are read.
    fn importers(&mut self, targets: &[String], notes: &mut Vec<String>) -> Importers {
        let mut needles = Needles::default();
        let mut go_folders = BTreeSet::new();
        for path in targets {
            let Some((lang, _)) = Lang::of(path) else { continue };
            if let Some(needle) = needle_of(&mut self.resolver, lang, path) {
                needles.add(lang, needle);
            }
            if lang == Lang::Go {
                go_folders.insert(folder_of(path).to_string());
            }
        }
        needles.seal();
        let mut found = Importers::new();
        if needles.by_lang.is_empty() && go_folders.is_empty() {
            return found;
        }
        let (mut candidates, stopped) =
            if needles.by_lang.is_empty() { (vec![], false) } else { files_that_mention(self.root, &needles) };
        if stopped {
            notes.push(format!(
                "Very many files mention the changed files. Only the first {MAX_MENTIONS} were read, so some files that use them may be missing."
            ));
        }
        // Files of the same Go package use each other without an import.
        for folder in go_folders {
            candidates.extend(self.go_files(&folder).iter().cloned());
        }
        candidates.sort();
        candidates.dedup();
        for candidate in candidates {
            let scope = self.scope(&candidate);
            for target in targets {
                if *target != candidate && scope.needs_file(target) {
                    found.entry(target.clone()).or_default().push(candidate.clone());
                }
            }
        }
        found
    }

    fn tree_file(&mut self, path: &str, changed: &Changed) -> TreeFile {
        let lines = changed.get(path);
        let facts = self.facts(path);
        let defs = facts.iter().flat_map(|facts| &facts.defs).map(|def| TreeDef {
            name: def.name.clone(),
            kind: def.kind,
            line: def.line,
            changed: lines.is_some_and(|lines| lines.touch(def.line, def.end)),
        });
        TreeFile {
            path: path.to_string(),
            changed: lines.is_some(),
            // A file that could not be read is listed like one in a language the map does not know.
            language: facts.as_ref().and_then(|_| Lang::of(path)).map(|(_, name)| name),
            defs: defs.collect(),
            tests: facts.as_ref().map_or(0, |facts| facts.tests),
        }
    }

    /// Everything the centre can be: each changed file the map can read, and each definition changed in it.
    /// Also says which of them the map starts on.
    fn centers(&mut self, files: &[String], changed: &Changed) -> (Vec<Center>, Option<usize>) {
        let mut centers = Vec::new();
        // The best start: a changed function outside the tests. Then any changed definition, then a file.
        let (mut function, mut definition) = (None, None);
        for path in files {
            let (Some(facts), Some(lines)) = (self.facts(path), changed.get(path)) else { continue };
            centers.push(Center { path: path.clone(), symbol: None, label: base_name(path).to_string(), line: 1 });
            for def in facts.defs.iter().filter(|def| lines.touch(def.line, def.end)) {
                let holds_others = matches!(def.kind, "class" | "struct" | "enum" | "trait" | "interface" | "type");
                if function.is_none() && !holds_others && facts.tests == 0 {
                    function = Some(centers.len());
                }
                definition.get_or_insert(centers.len());
                centers.push(Center {
                    path: path.clone(),
                    symbol: Some(def.name.clone()),
                    label: def.name.clone(),
                    line: def.line,
                });
            }
        }
        let start = function.or(definition).or(if centers.is_empty() { None } else { Some(0) });
        (centers, start)
    }

    // ----- the two pictures -----

    fn file_node(&self, path: &str, layer: i32, changed: &Changed) -> Node {
        Node {
            id: format!("file:{path}"),
            label: base_name(path).to_string(),
            kind: "file",
            path: Some(path.to_string()),
            line: None,
            changed: changed.contains_key(path),
            layer,
            title: path.to_string(),
        }
    }

    /// The box for something `from` needs. `line` is where `from` asks for it.
    fn need_node(&self, need: &Need, layer: i32, from: &str, line: u32, changed: &Changed) -> Node {
        match need {
            Need::File(path) => self.file_node(path, layer, changed),
            Need::Package(name) => Node {
                id: format!("package:{name}"),
                label: name.clone(),
                kind: "package",
                path: Some(from.to_string()),
                line: Some(line),
                changed: false,
                layer,
                title: format!(
                    "{name}, a package from outside the project. {} imports it on line {line}.",
                    base_name(from)
                ),
            },
            Need::Other(spec) => Node {
                id: format!("other:{spec}"),
                label: spec.clone(),
                kind: "other",
                path: Some(from.to_string()),
                line: Some(line),
                changed: false,
                layer,
                title: format!("{} imports this on line {line}. The map cannot tell where it leads.", base_name(from)),
            },
        }
    }

    /// What the centre's file needs, two steps out, and which files need it.
    fn dependencies(&mut self, center: &Center, changed: &Changed, importers: &Importers) -> Graph {
        let mut graph = Graph::default();
        let file = center.path.as_str();
        let middle = graph.add(self.file_node(file, 0, changed));

        let users = importers.get(file).map(Vec::as_slice).unwrap_or_default();
        for user in users.iter().take(MAX_BEFORE) {
            let id = graph.add(self.file_node(user, -1, changed));
            graph.edge(&id, &middle);
        }
        graph.hidden += users.len().saturating_sub(MAX_BEFORE);

        let scope = self.scope(file);
        let mut first_step = Vec::new();
        for (need, line) in scope.needs.iter().take(MAX_AFTER) {
            let id = graph.add(self.need_node(need, 1, file, *line, changed));
            graph.edge(&middle, &id);
            if let Need::File(path) = need {
                first_step.push((path.clone(), id));
            }
        }
        graph.hidden += scope.needs.len().saturating_sub(MAX_AFTER);

        let mut room = MAX_SECOND;
        for (parent, parent_id) in first_step {
            let scope = self.scope(&parent);
            let mut shown = 0;
            for (need, line) in &scope.needs {
                let node = self.need_node(need, 2, &parent, *line, changed);
                match graph.layer_of(&node.id) {
                    // Two files of the first step may need the same thing.
                    Some(2) => graph.edge(&parent_id, &node.id),
                    // Already nearer the centre: an arrow back would cross the picture and say little.
                    Some(_) => {}
                    None if shown == MAX_PER_BOX || room == 0 => graph.hidden += 1,
                    None => {
                        let id = graph.add(node);
                        graph.edge(&parent_id, &id);
                        shown += 1;
                        room -= 1;
                    }
                }
            }
        }
        graph.tell_files_apart();
        graph
    }

    fn def_node(&self, path: &str, def: &Def, layer: i32, line: Option<u32>, changed: &Changed) -> Node {
        Node {
            id: format!("def:{path}#{}", def.name),
            // A function is shown as a call; a constant, a class or a route by its name alone.
            label: if def.is_callable() { format!("{}()", def.name) } else { def.name.clone() },
            kind: "function",
            path: Some(path.to_string()),
            line: Some(line.unwrap_or(def.line)),
            changed: changed.get(path).is_some_and(|lines| lines.touch(def.line, def.end)),
            layer,
            title: format!("{} {} in {path}, line {}", def.kind, def.name, def.line),
        }
    }

    /// What one definition uses, as boxes one step after it: first what the
    /// project defines, then packages, then calls the map cannot place.
    fn used_by(&mut self, path: &str, def: &Def, layer: i32, changed: &Changed) -> Vec<Used> {
        let Some((lang, _)) = Lang::of(path) else { return vec![] };
        let mut groups: [Vec<Used>; 3] = Default::default();
        for used in &def.uses {
            let root = used.chain.split([':', '.']).next().unwrap_or_default();
            match self.meaning(path, def.owner(), &used.chain) {
                Meaning::Def { path: file, name } => {
                    if file == path && name == def.name {
                        continue;
                    }
                    let Some(target) = self.facts(&file).and_then(|facts| facts.def(&name).cloned()) else { continue };
                    // A type named in a signature is not something the code calls.
                    let is_a_type = matches!(target.kind, "type" | "interface" | "enum" | "struct" | "trait");
                    if is_a_type && !used.call {
                        continue;
                    }
                    groups[0].push((self.def_node(&file, &target, layer, None, changed), Some((file, target))));
                }
                Meaning::File(file) if used.call => {
                    let mut node = self.file_node(&file, layer, changed);
                    node.title = format!("{} is called on line {} of {}. It leads into {file}.", used.chain, used.line, base_name(path));
                    groups[0].push((node, None));
                }
                Meaning::Package(name) if used.call => groups[1].push((
                    Node {
                        id: format!("call:{}", used.chain),
                        label: format!("{}()", used.chain),
                        kind: "package",
                        path: Some(path.to_string()),
                        line: Some(used.line),
                        changed: false,
                        layer,
                        title: format!("{}() comes from {name}, a package from outside the project. It is called on line {} of {}.", used.chain, used.line, base_name(path)),
                    },
                    None,
                )),
                Meaning::Unknown if used.call && !used.chain.starts_with('.') && !builtin(lang, root) => groups[2].push((
                    Node {
                        id: format!("call:{}", used.chain),
                        label: format!("{}()", used.chain),
                        kind: "other",
                        path: Some(path.to_string()),
                        line: Some(used.line),
                        changed: false,
                        layer,
                        title: format!("{}() is called on line {} of {}. The map cannot tell where it is defined.", used.chain, used.line, base_name(path)),
                    },
                    None,
                )),
                _ => {}
            }
        }
        let mut seen = BTreeSet::new();
        groups.into_iter().flatten().filter(|(node, _)| seen.insert(node.id.clone())).collect()
    }

    /// Whether `used`, written in `path`, names the definition `target` of `file`.
    fn names_it(&mut self, path: &str, owner: Option<&str>, used: &scan::Use, file: &str, target: &Def) -> bool {
        match self.meaning(path, owner, &used.chain) {
            Meaning::Def { path: found, name } => {
                // A class is used when one of its methods is.
                let part_of_it = target.owner().is_none()
                    && (name.strip_prefix(target.name.as_str()))
                        .is_some_and(|rest| rest.starts_with("::") || rest.starts_with('.'));
                found == file && (name == target.name || part_of_it)
            }
            // A method is called through a value whose type the scanner cannot see. A call of
            // that name counts when the file could reach the method at all.
            Meaning::Unknown if used.call => {
                let Some(method) = target.method_name() else { return false };
                let last = used.chain.rsplit([':', '.']).next().unwrap_or_default();
                let through_a_value = used.chain.contains('.');
                last == method && through_a_value && (path == file || self.scope(path).needs_file(file))
            }
            _ => false,
        }
    }

    /// The definitions, in `file` and in the files that import it, that use `target`.
    fn users_of(&mut self, file: &str, target: &Def, importers: &Importers, changed: &Changed) -> Vec<Node> {
        let mut places = vec![file.to_string()];
        places.extend(importers.get(file).into_iter().flatten().cloned());
        let mut found = Vec::new();
        for place in places {
            let Some(facts) = self.facts(&place) else { continue };
            for def in &facts.defs {
                if place == file && def.name == target.name {
                    continue;
                }
                let hit = def.uses.iter().find(|used| self.names_it(&place, def.owner(), used, file, target));
                if let Some(used) = hit {
                    found.push(self.def_node(&place, def, -1, Some(used.line), changed));
                }
            }
            // Code that runs when the file loads, and tests: the file itself is the user.
            if let Some(used) = facts.loose.iter().find(|used| self.names_it(&place, None, used, file, target)) {
                let mut node = self.file_node(&place, -1, changed);
                node.line = Some(used.line);
                node.title = format!("{place} uses {} on line {}", target.name, used.line);
                found.push(node);
            }
        }
        found
    }

    /// Who uses the definition at the centre and what it uses, two steps out.
    fn calls(&mut self, center: &Center, changed: &Changed, importers: &Importers) -> Graph {
        let mut graph = Graph::default();
        let file = center.path.as_str();
        let Some(facts) = self.facts(file) else { return graph };
        let lines = changed.get(file);
        // The picked definition, or every changed one when a whole file is picked.
        let middle: Vec<&Def> = match &center.symbol {
            Some(name) => facts.def(name).into_iter().collect(),
            None => {
                let all: Vec<&Def> =
                    facts.defs.iter().filter(|def| lines.is_some_and(|lines| lines.touch(def.line, def.end))).collect();
                graph.hidden += all.len().saturating_sub(MAX_MIDDLE);
                all.into_iter().take(MAX_MIDDLE).collect()
            }
        };
        for def in &middle {
            graph.add(self.def_node(file, def, 0, None, changed));
        }
        let alone = middle.len() == 1;
        let mut room = MAX_SECOND;
        for def in middle {
            let id = format!("def:{file}#{}", def.name);
            let users = self.users_of(file, def, importers, changed);
            graph.hidden += users.len().saturating_sub(MAX_BEFORE);
            for user in users.into_iter().take(MAX_BEFORE) {
                // Two definitions at the centre may use each other; that arrow would run inside one column.
                if graph.layer_of(&user.id).is_none_or(|layer| layer == -1) {
                    let user_id = graph.add(user);
                    graph.edge(&user_id, &id);
                }
            }
            let used = self.used_by(file, def, 1, changed);
            graph.hidden += used.len().saturating_sub(MAX_AFTER);
            for (node, target) in used.into_iter().take(MAX_AFTER) {
                if graph.layer_of(&node.id).is_some_and(|layer| layer != 1) {
                    continue;
                }
                let node_id = graph.add(node);
                graph.edge(&id, &node_id);
                // With one definition at the centre there is room for what its callees call.
                let Some((path, target)) = target.filter(|_| alone) else { continue };
                let mut shown = 0;
                for (next, _) in self.used_by(&path, &target, 2, changed) {
                    match graph.layer_of(&next.id) {
                        Some(2) => graph.edge(&node_id, &next.id),
                        Some(_) => {}
                        None if shown == MAX_PER_BOX || room == 0 => graph.hidden += 1,
                        None => {
                            let next_id = graph.add(next);
                            graph.edge(&node_id, &next_id);
                            shown += 1;
                            room -= 1;
                        }
                    }
                }
            }
        }
        graph.tell_files_apart();
        graph
    }
}

impl Graph {
    /// Adds a box unless one with its id is there. Returns the id.
    fn add(&mut self, node: Node) -> String {
        let id = node.id.clone();
        if !self.nodes.iter().any(|known| known.id == id) {
            self.nodes.push(node);
        }
        id
    }

    fn layer_of(&self, id: &str) -> Option<i32> {
        self.nodes.iter().find(|node| node.id == id).map(|node| node.layer)
    }

    fn edge(&mut self, from: &str, to: &str) {
        let edge = Edge { from: from.to_string(), to: to.to_string() };
        if from != to && !self.edges.contains(&edge) {
            self.edges.push(edge);
        }
    }

    /// A file is shown by its name. Names that say little alone (`index.ts`,
    /// `mod.rs`, `client.ts`) and names two boxes share get their folder in front.
    fn tell_files_apart(&mut self) {
        let mut times: HashMap<String, usize> = HashMap::new();
        for node in self.nodes.iter().filter(|node| node.kind == "file") {
            *times.entry(node.label.clone()).or_default() += 1;
        }
        for node in self.nodes.iter_mut().filter(|node| node.kind == "file") {
            let Some(path) = node.path.clone() else { continue };
            let stem = node.label.split('.').next().unwrap_or_default();
            let says_little = matches!(
                stem,
                "index"
                    | "mod"
                    | "__init__"
                    | "main"
                    | "lib"
                    | "client"
                    | "config"
                    | "util"
                    | "utils"
                    | "types"
                    | "common"
            );
            let folder = base_name(folder_of(&path));
            if (says_little || times.get(&node.label).copied().unwrap_or(0) > 1) && !folder.is_empty() {
                node.label = format!("{folder}/{}", base_name(&path));
            }
        }
    }
}

/// Makes the map of the project in `root`, given what the task changed.
/// `wanted` is the centre asked for: a path and, for one definition, its name.
pub fn build(root: &Path, changed: &Changed, wanted: Option<(&str, Option<&str>)>) -> CodeMap {
    let mut project = Project::new(root);
    let mut notes = Vec::new();

    // A changed file that is gone has nothing to show.
    let mut files: Vec<String> = changed.keys().filter(|path| root.join(path).is_file()).cloned().collect();
    if files.len() > MAX_CHANGED {
        notes.push(format!("This task changed {} files. The map covers the first {MAX_CHANGED}.", files.len()));
        files.truncate(MAX_CHANGED);
    }
    let readable: Vec<String> = files.iter().filter(|path| project.facts(path).is_some()).cloned().collect();

    // The near neighbours: what the changed files import, and what imports them.
    let mut tree_files: BTreeSet<String> = files.iter().cloned().collect();
    for path in &readable {
        for (need, _) in &project.scope(path).needs {
            if let Need::File(file) = need {
                tree_files.insert(file.clone());
            }
        }
    }
    let mut importers = project.importers(&readable, &mut notes);
    for (target, list) in importers.iter_mut() {
        if list.len() > MAX_IMPORTERS {
            notes.push(format!(
                "{} files import {}. The map shows the first {MAX_IMPORTERS}.",
                list.len(),
                base_name(target)
            ));
            list.truncate(MAX_IMPORTERS);
        }
        tree_files.extend(list.iter().cloned());
    }
    if tree_files.len() > MAX_TREE {
        notes.push(format!("The tree lists the first {MAX_TREE} of {} files.", tree_files.len()));
    }
    let tree: Vec<TreeFile> = tree_files.iter().take(MAX_TREE).map(|path| project.tree_file(path, changed)).collect();

    let (centers, start) = project.centers(&readable, changed);
    let asked = wanted.and_then(|(path, symbol)| {
        centers.iter().position(|center| center.path == path && center.symbol.as_deref() == symbol)
    });
    let center = asked.or(start).map(|index| centers[index].clone());
    let (deps, calls) = match &center {
        Some(center) => (project.dependencies(center, changed, &importers), project.calls(center, changed, &importers)),
        None => (Graph::default(), Graph::default()),
    };
    CodeMap { center, centers, tree, deps, calls, notes }
}

#[cfg(test)]
mod tests {
    use super::scan::testing::Tree;
    use super::*;

    fn all(paths: &[&str]) -> Changed {
        paths.iter().map(|path| (path.to_string(), Lines::All)).collect()
    }

    fn lines(path: &str, ranges: &[(u32, u32)]) -> Changed {
        [(path.to_string(), Lines::Some(ranges.to_vec()))].into()
    }

    /// "label" of every box of one layer, in order.
    fn layer(graph: &Graph, layer: i32) -> Vec<String> {
        graph.nodes.iter().filter(|node| node.layer == layer).map(|node| node.label.clone()).collect()
    }

    fn arrows(graph: &Graph) -> Vec<String> {
        let label = |id: &str| graph.nodes.iter().find(|node| node.id == id).map_or("?", |node| node.label.as_str());
        graph.edges.iter().map(|edge| format!("{} -> {}", label(&edge.from), label(&edge.to))).collect()
    }

    fn node<'a>(graph: &'a Graph, label: &str) -> &'a Node {
        graph.nodes.iter().find(|node| node.label == label).unwrap_or_else(|| panic!("no box called {label}"))
    }

    /// The shop of the design: a route, the limiter the task adds, a service and its database.
    fn shop(name: &str) -> Tree {
        Tree::new(
            name,
            &[
                (
                    "src/routes/auth.ts",
                    "import { Router } from 'express';\nimport { loginLimiter } from '../middleware/rateLimit';\nimport { findUser, checkPassword } from '../services/users';\n\nexport const router = Router();\n\nexport async function login(req, res) {\n  const user = await findUser(req.body.name);\n  return checkPassword(user, req.body.password) ? res.json({ ok: true }) : res.status(401).end();\n}\n\nexport function logout(req, res) {\n  res.json({ ok: true });\n}\n\nrouter.post('/login', loginLimiter, login);\n",
                ),
                (
                    "src/middleware/rateLimit.ts",
                    "import rateLimit from 'express-rate-limit';\n\nexport const loginLimiter = rateLimit({\n  windowMs: 60 * 1000,\n  limit: 5,\n  handler: (req, res) => {\n    res.status(429);\n    res.json({ error: 'Too many attempts' });\n  },\n});\n",
                ),
                (
                    "src/services/users.ts",
                    "import { db } from '../db/client';\nimport { compare } from 'node:crypto';\n\nexport function findUser(name: string) {\n  return db.query('select 1', [name]);\n}\n\nexport function checkPassword(user, password: string) {\n  return compare(user.hash, password);\n}\n",
                ),
                ("src/db/client.ts", "import pg from 'pg';\n\nexport const db = new pg.Pool();\n"),
                (
                    "tests/rateLimit.test.ts",
                    "import { test, expect } from 'vitest';\nimport { loginLimiter } from '../src/middleware/rateLimit';\n\ntest('allows five', () => { expect(loginLimiter).toBeDefined(); });\ntest('blocks the sixth', () => {});\ntest('opens again after a minute', () => {});\n",
                ),
                ("src/unrelated.ts", "export const nothing = 1;\n"),
                ("docs/limits.md", "# Limits\n\nSee rateLimit.\n"),
                ("styles/app.css", ".rateLimit { color: red }\n"),
            ],
        )
    }

    #[test]
    fn reads_which_lines_a_patch_changed() {
        let patch = "diff --git a/src/a.ts b/src/a.ts\nindex 1..2 100644\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -3 +3,2 @@ context\n-old\n+new\n+newer\n@@ -10,2 +10,0 @@\n-gone\n-gone too\ndiff --git a/new file.txt b/new file.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new file.txt\t\n@@ -0,0 +1,2 @@\n+a\n+b\ndiff --git a/gone.ts b/gone.ts\ndeleted file mode 100644\n--- a/gone.ts\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\ndiff --git a/logo.png b/logo.png\nindex 1..2 100644\nBinary files a/logo.png and b/logo.png differ\n";
        let changed = parse_patch(patch);
        assert_eq!(changed.get("src/a.ts"), Some(&Lines::Some(vec![(3, 4), (10, 10)])));
        assert_eq!(changed.get("new file.txt"), Some(&Lines::All));
        assert_eq!(changed.get("logo.png"), Some(&Lines::All));
        assert!(!changed.contains_key("gone.ts"));
        assert!(Lines::Some(vec![(3, 4)]).touch(4, 9) && !Lines::Some(vec![(3, 4)]).touch(5, 9));
    }

    #[test]
    fn the_tree_lists_changed_files_and_their_neighbours_with_what_they_define() {
        let tree = shop("tree");
        let changed = all(&["src/middleware/rateLimit.ts", "tests/rateLimit.test.ts", "docs/limits.md"]);
        let mut changed = changed;
        changed.insert("src/routes/auth.ts".into(), Lines::Some(vec![(16, 16)]));
        let map = build(&tree.0, &changed, None);

        let listed: Vec<(&str, bool, Option<&str>)> =
            map.tree.iter().map(|file| (file.path.as_str(), file.changed, file.language)).collect();
        assert_eq!(
            listed,
            vec![
                // Markdown is not read: the file is listed, with nothing made up about it.
                ("docs/limits.md", true, None),
                ("src/middleware/rateLimit.ts", true, Some("TypeScript")),
                ("src/routes/auth.ts", true, Some("TypeScript")),
                // Imported by a changed file, not changed itself.
                ("src/services/users.ts", false, Some("TypeScript")),
                ("tests/rateLimit.test.ts", true, Some("TypeScript")),
            ]
        );
        let file = |path: &str| map.tree.iter().find(|file| file.path == path).unwrap();
        let defs = |path: &str| -> Vec<String> {
            file(path)
                .defs
                .iter()
                .map(|def| format!("{} {}{}", def.kind, def.name, if def.changed { " *" } else { "" }))
                .collect()
        };
        assert_eq!(defs("src/middleware/rateLimit.ts"), vec!["const loginLimiter *"]);
        assert_eq!(
            defs("src/routes/auth.ts"),
            vec!["const router", "function login", "function logout", "route POST /login *"]
        );
        assert_eq!(defs("src/services/users.ts"), vec!["function findUser", "function checkPassword"]);
        assert_eq!((file("tests/rateLimit.test.ts").tests, defs("tests/rateLimit.test.ts").len()), (3, 0));
        assert!(file("docs/limits.md").defs.is_empty());
        assert!(map.notes.is_empty(), "{:?}", map.notes);
    }

    #[test]
    fn dependencies_reach_two_steps_out_and_mark_packages_and_changes() {
        let tree = shop("deps");
        let mut changed = all(&["src/middleware/rateLimit.ts", "tests/rateLimit.test.ts"]);
        changed.insert("src/routes/auth.ts".into(), Lines::Some(vec![(16, 16)]));
        let map = build(&tree.0, &changed, Some(("src/routes/auth.ts", Some("POST /login"))));
        assert_eq!(map.center.as_ref().map(|c| c.label.as_str()), Some("POST /login"));

        let deps = &map.deps;
        assert_eq!(layer(deps, 0), vec!["auth.ts"]);
        assert_eq!(layer(deps, 1), vec!["express", "rateLimit.ts", "users.ts"]);
        assert_eq!(layer(deps, 2), vec!["express-rate-limit", "db/client.ts"]);
        assert_eq!(
            arrows(deps),
            vec![
                "auth.ts -> express",
                "auth.ts -> rateLimit.ts",
                "auth.ts -> users.ts",
                "rateLimit.ts -> express-rate-limit",
                "users.ts -> db/client.ts",
            ]
        );
        assert!(node(deps, "auth.ts").changed && node(deps, "rateLimit.ts").changed && !node(deps, "users.ts").changed);
        let package = node(deps, "express-rate-limit");
        // A click on a package leads to the line that imports it.
        assert_eq!(
            (package.kind, package.path.as_deref(), package.line),
            ("package", Some("src/middleware/rateLimit.ts"), Some(1))
        );
        assert_eq!((node(deps, "users.ts").kind, node(deps, "users.ts").line), ("file", None));
        // `node:crypto` is part of the language's home: it is not a package and is left out.
        assert!(!deps.nodes.iter().any(|node| node.label.contains("crypto")));

        // Centred on the limiter's file: the files that import it stand before it.
        let map = build(&tree.0, &changed, Some(("src/middleware/rateLimit.ts", None)));
        assert_eq!(layer(&map.deps, -1), vec!["auth.ts", "rateLimit.test.ts"]);
        assert_eq!(layer(&map.deps, 1), vec!["express-rate-limit"]);
        assert!(arrows(&map.deps).contains(&"rateLimit.test.ts -> rateLimit.ts".to_string()));
    }

    #[test]
    fn calls_show_who_uses_the_centre_and_what_it_calls() {
        let tree = shop("calls");
        let mut changed = all(&["src/middleware/rateLimit.ts"]);
        changed.insert("src/routes/auth.ts".into(), Lines::Some(vec![(16, 16)]));

        // The map starts on the first changed function outside the tests.
        let map = build(&tree.0, &changed, None);
        let center = map.center.clone().unwrap();
        assert_eq!(
            (center.path.as_str(), center.symbol.as_deref(), center.line),
            ("src/middleware/rateLimit.ts", Some("loginLimiter"), 3)
        );
        let labels: Vec<&str> = map.centers.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["rateLimit.ts", "loginLimiter", "auth.ts", "POST /login"]);

        let calls = &map.calls;
        // The route uses it, and so does the code of the test file, which belongs to no function.
        assert_eq!(layer(calls, -1), vec!["POST /login", "rateLimit.test.ts"]);
        assert_eq!(layer(calls, 0), vec!["loginLimiter"]);
        assert_eq!(layer(calls, 1), vec!["rateLimit()", "res.status()", "res.json()"]);
        assert_eq!(node(calls, "rateLimit()").kind, "package");
        assert_eq!(node(calls, "res.status()").kind, "other");
        // A call the map cannot place leads to the line that makes it.
        assert_eq!(
            (node(calls, "res.status()").path.as_deref(), node(calls, "res.status()").line),
            (Some("src/middleware/rateLimit.ts"), Some(7))
        );
        // The route uses the limiter on its line 16; a click on it leads there.
        let route = node(calls, "POST /login");
        assert_eq!((route.path.as_deref(), route.line, route.changed), (Some("src/routes/auth.ts"), Some(16), true));
        assert!(node(calls, "loginLimiter").changed);

        // Centred on the route, as in the design: the route, what it uses, and what those call.
        let map = build(&tree.0, &changed, Some(("src/routes/auth.ts", Some("POST /login"))));
        let calls = &map.calls;
        assert_eq!(layer(calls, 0), vec!["POST /login"]);
        assert_eq!(layer(calls, 1), vec!["loginLimiter", "login()"]);
        assert_eq!(layer(calls, 2), vec!["rateLimit()", "res.status()", "res.json()", "findUser()", "checkPassword()"]);
        assert!(arrows(calls).contains(&"login() -> findUser()".to_string()));
        assert!(arrows(calls).contains(&"loginLimiter -> rateLimit()".to_string()));
        // `res.json()` is called by both; it is one box with two arrows.
        assert!(arrows(calls).contains(&"login() -> res.json()".to_string()));
        assert_eq!(node(calls, "findUser()").path.as_deref(), Some("src/services/users.ts"));
        assert_eq!(node(calls, "findUser()").line, Some(4));

        // A whole file at the centre: each changed definition of it, one step each way.
        changed.insert("src/routes/auth.ts".into(), Lines::All);
        let map = build(&tree.0, &changed, Some(("src/routes/auth.ts", None)));
        assert_eq!(map.center.as_ref().map(|c| (c.label.as_str(), c.symbol.is_none())), Some(("auth.ts", true)));
        assert_eq!(layer(&map.calls, 0), vec!["router", "login()", "logout()", "POST /login"]);
        assert_eq!(
            layer(&map.calls, 1),
            vec!["Router()", "findUser()", "checkPassword()", "res.json()", "res.status()", "loginLimiter"]
        );
        assert!(layer(&map.calls, 2).is_empty());
        // The route uses `login`, which stands at the centre too: no arrow runs inside the column.
        assert!(!arrows(&map.calls).iter().any(|arrow| arrow == "POST /login -> login()"));
    }

    #[test]
    fn a_centre_that_is_gone_falls_back_and_nothing_changed_is_an_empty_map() {
        let tree = shop("fallback");
        let map = build(&tree.0, &lines("src/routes/auth.ts", &[(7, 8)]), Some(("src/gone.ts", Some("nothing"))));
        assert_eq!(map.center.as_ref().and_then(|c| c.symbol.as_deref()), Some("login"));
        // The tests file imports the limiter, which this task did not change: it is not a neighbour.
        assert!(!map.tree.iter().any(|file| file.path.starts_with("tests/")));

        let map = build(&tree.0, &Changed::new(), None);
        assert_eq!((map.center, map.tree.len(), map.deps.nodes.len(), map.calls.nodes.len()), (None, 0, 0, 0));

        // Only a file the map cannot read changed: it is listed, and there is no centre.
        let map = build(&tree.0, &all(&["docs/limits.md", "gone.md"]), None);
        assert_eq!((map.center, map.tree.len(), map.tree[0].language), (None, 1, None));
    }

    #[test]
    fn what_was_read_is_read_again_only_when_the_file_changes() {
        let tree = shop("memory");
        let changed = all(&["src/middleware/rateLimit.ts"]);
        let before = build(&tree.0, &changed, None);
        assert_eq!(before, build(&tree.0, &changed, None));

        // A new function in a neighbour shows up at once.
        let users = tree.0.join("src/routes/auth.ts");
        let text = std::fs::read_to_string(&users).unwrap();
        std::fs::write(&users, format!("{text}\nexport function extra() {{ return loginLimiter; }}\n")).unwrap();
        let after = build(&tree.0, &changed, None);
        assert!(layer(&after.calls, -1).contains(&"extra()".to_string()), "{:?}", layer(&after.calls, -1));

        // And a file that starts to import the changed one becomes a neighbour.
        std::fs::write(
            tree.0.join("src/unrelated.ts"),
            "import { loginLimiter } from './middleware/rateLimit';\nexport const nothing = loginLimiter;\n",
        )
        .unwrap();
        let after = build(&tree.0, &changed, None);
        assert!(after.tree.iter().any(|file| file.path == "src/unrelated.ts"));
    }

    #[test]
    fn rust_python_and_go_projects() {
        let tree = Tree::new(
            "languages",
            &[
                ("Cargo.toml", "[package]\nname = \"shop\"\n"),
                ("src/lib.rs", "pub mod limits;\npub mod routes;\n"),
                (
                    "src/limits.rs",
                    "use serde::Serialize;\nuse std::time::Duration;\n\n#[derive(Serialize)]\npub struct Limiter { per_minute: u32 }\n\nimpl Limiter {\n    pub fn new(per_minute: u32) -> Self { Self { per_minute } }\n    pub fn over(&self, hits: u32) -> bool { hits > self.per_minute && log(hits) }\n}\n\nfn log(hits: u32) -> bool { tracing::warn!(\"over\"); hits > 0 }\n",
                ),
                (
                    "src/routes.rs",
                    "use crate::limits::Limiter;\n\npub fn login(hits: u32) -> u16 {\n    let limiter = Limiter::new(5);\n    if limiter.over(hits) { 429 } else { 200 }\n}\n",
                ),
                ("app/__init__.py", ""),
                (
                    "app/limits.py",
                    "import redis\nfrom .config import LIMIT\n\ndef over(hits):\n    return hits > LIMIT and redis.incr('over')\n",
                ),
                ("app/config.py", "LIMIT = 5\n"),
                (
                    "app/routes.py",
                    "from .limits import over\n\ndef login(hits):\n    return 429 if over(hits) else 200\n",
                ),
                ("go.mod", "module example.com/shop\n"),
                (
                    "limits/limits.go",
                    "package limits\n\nimport \"github.com/gorilla/mux\"\n\nfunc Over(hits int) bool { return hits > max() }\n\nvar _ = mux.NewRouter\n",
                ),
                ("limits/config.go", "package limits\n\nfunc max() int { return 5 }\n"),
                (
                    "server/main.go",
                    "package main\n\nimport (\n\t\"fmt\"\n\t\"example.com/shop/limits\"\n)\n\nfunc main() { fmt.Println(limits.Over(6)) }\n",
                ),
            ],
        );

        // Rust: the method is called through a value, so it is matched by its name.
        let map = build(&tree.0, &lines("src/limits.rs", &[(9, 9)]), None);
        assert_eq!(map.center.as_ref().and_then(|c| c.symbol.as_deref()), Some("Limiter::over"));
        assert_eq!(layer(&map.deps, -1), vec!["src/lib.rs", "routes.rs"]);
        assert_eq!(layer(&map.deps, 1), vec!["serde"], "std is part of the language and left out");
        assert_eq!(layer(&map.calls, -1), vec!["login()"]);
        assert_eq!(layer(&map.calls, 1), vec!["log()"]);
        assert_eq!(node(&map.calls, "log()").kind, "function");

        // Python.
        let map = build(&tree.0, &all(&["app/limits.py"]), None);
        assert_eq!(map.center.as_ref().and_then(|c| c.symbol.as_deref()), Some("over"));
        assert_eq!(layer(&map.deps, -1), vec!["routes.py"]);
        assert_eq!(layer(&map.deps, 1), vec!["redis", "app/config.py"]);
        assert_eq!(layer(&map.calls, -1), vec!["login()"]);
        assert_eq!(layer(&map.calls, 1), vec!["LIMIT", "redis.incr()"]);
        assert_eq!(node(&map.calls, "redis.incr()").kind, "package");

        // Go: a package is a folder; its files need each other without importing.
        let map = build(&tree.0, &all(&["limits/limits.go"]), None);
        assert_eq!(map.center.as_ref().and_then(|c| c.symbol.as_deref()), Some("Over"));
        assert_eq!(layer(&map.deps, -1), vec!["server/main.go"]);
        assert_eq!(layer(&map.deps, 1), vec!["github.com/gorilla/mux", "limits/config.go"]);
        assert_eq!(layer(&map.calls, -1), vec!["main()"]);
        assert_eq!(layer(&map.calls, 1), vec!["max()"]);
        let paths: Vec<&str> = map.tree.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, vec!["limits/config.go", "limits/limits.go", "server/main.go"]);
    }

    /// The map reads the code it is made of: a check against real files, not ones written for a test.
    #[test]
    fn reads_its_own_code() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let lexer = facts_of(root, "crates/core/src/map/lex.rs").expect("lex.rs is read");
        assert_eq!(lexer.def("lex").map(|def| def.kind), Some("function"));
        assert_eq!(lexer.def("Token").map(|def| def.kind), Some("struct"));
        assert_eq!(lexer.def("Lexer::template").map(|def| def.kind), Some("function"));
        assert!(lexer.tests >= 10, "the tests of lex.rs are counted, not listed: {}", lexer.tests);
        assert!(lexer.def("go_raw_strings_and_runes").is_none());

        // Centred on the function that cuts text into tokens: every language's reader calls it.
        let at = lexer.def("lex").unwrap().line;
        let map = build(root, &lines("crates/core/src/map/lex.rs", &[(at, at)]), None);
        assert_eq!(map.center.as_ref().and_then(|c| c.symbol.as_deref()), Some("lex"));
        let users = layer(&map.deps, -1);
        for file in ["map.rs", "js.rs", "rust.rs", "python.rs", "go.rs", "scan.rs"] {
            assert!(users.contains(&file.to_string()), "{file} uses lex.rs: {users:?}");
        }
        // Its own tests use it too; they belong to no function, so the file stands for them.
        assert_eq!(layer(&map.calls, -1), vec!["lex.rs", "scan()", "scan()", "scan()", "scan()"]);
        let from: Vec<&str> =
            map.calls.nodes.iter().filter(|n| n.layer == -1).filter_map(|n| n.path.as_deref()).map(base_name).collect();
        assert_eq!(from, vec!["lex.rs", "go.rs", "js.rs", "python.rs", "rust.rs"]);

        // And the TypeScript side of the same feature.
        let flow = facts_of(root, "ui/src/core/flow.ts").expect("flow.ts is read");
        for name in ["parseFlow", "layoutFlow", "flowRequest"] {
            assert_eq!(flow.def(name).map(|def| def.kind), Some("function"), "{name}");
        }
        assert_eq!(flow.def("FlowNode").map(|def| def.kind), Some("type"));
        let at = flow.def("parseFlow").unwrap().line;
        let map = build(root, &lines("ui/src/core/flow.ts", &[(at, at)]), None);
        // The Plan view lays its chart out with the same code, so its files need flow.ts too.
        let users = layer(&map.deps, -1);
        for file in ["flow.test.ts", "Map.tsx", "plan.ts"] {
            assert!(users.contains(&file.to_string()), "{file} uses flow.ts: {users:?}");
        }
        let callers = layer(&map.calls, -1);
        assert!(
            callers.contains(&"MapView()".to_string()) && callers.contains(&"flow.test.ts".to_string()),
            "{callers:?}"
        );
    }

    /// Reads every file under `AE_MAP_SCAN` that the map knows how to read, to see that no
    /// real code makes a scanner fall over or stand still. Run it with
    /// `AE_MAP_SCAN=/some/folder cargo test -p ae-core --lib map::tests::reads_any -- --ignored --nocapture`.
    /// With `AE_MAP_PRINT=1` it also prints what it read, which is the quick way to look at one file.
    #[test]
    #[ignore = "reads a folder named in AE_MAP_SCAN; for trying the scanners on real code"]
    fn reads_any_code_without_falling_over() {
        let Ok(folder) = std::env::var("AE_MAP_SCAN") else { return };
        let (mut files, mut defs, mut imports, mut slowest) = (0, 0, 0, (std::time::Duration::ZERO, String::new()));
        for entry in WalkBuilder::new(&folder).hidden(false).git_ignore(false).build().flatten() {
            let path = entry.path().to_string_lossy().replace('\\', "/");
            if !entry.file_type().is_some_and(|kind| kind.is_file()) || Lang::of(&path).is_none() {
                continue;
            }
            let Some(text) = read_text(entry.path(), entry.metadata().map_or(0, |meta| meta.len())) else { continue };
            let started = std::time::Instant::now();
            let facts = scan::scan(&path, &text).unwrap();
            let took = started.elapsed();
            if took > slowest.0 {
                slowest = (took, path.clone());
            }
            if std::env::var("AE_MAP_PRINT").is_ok() {
                // Asked to show what was read: one line for each definition and import.
                for def in &facts.defs {
                    let calls: Vec<&str> = def.uses.iter().filter(|u| u.call).map(|u| u.chain.as_str()).collect();
                    println!("{} {} {}-{} calls {}", def.kind, def.name, def.line, def.end, calls.join(" "));
                }
                for import in &facts.imports {
                    let names: Vec<&str> = import.bindings.iter().map(|b| b.local.as_str()).collect();
                    println!("import {} line {} as {}", import.spec, import.line, names.join(" "));
                }
                println!("{} tests", facts.tests);
            }
            files += 1;
            defs += facts.defs.len();
            imports += facts.imports.len();
        }
        println!("{files} files, {defs} definitions, {imports} imports; slowest {:?} for {}", slowest.0, slowest.1);
    }

    /// Makes a project of 20 000 files and measures the map. Run it with
    /// `cargo test -p ae-core --lib map::tests::stays_fast -- --ignored --nocapture`.
    #[test]
    #[ignore = "writes 20 000 files; run it by hand when the scanning changes"]
    fn stays_fast_on_twenty_thousand_files() {
        let dir = std::env::temp_dir().join(format!("ae-map-large-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for folder in 0..200 {
            let path = dir.join(format!("src/area{folder}"));
            std::fs::create_dir_all(&path).unwrap();
            for file in 0..100 {
                // Every file imports two others and defines a few functions; one in fifty imports the changed file.
                let other = (file + 1) % 100;
                let uses_it =
                    if (folder * 100 + file) % 50 == 0 { "import { changed } from '../area0/file0';\n" } else { "" };
                let text = format!(
                    "import {{ f{other} }} from './file{other}';\nimport express from 'express';\n{uses_it}\nexport function f{file}(a: number) {{\n  return f{other}(a) + {file};\n}}\n\nexport const c{file} = {{ name: 'file{file}', run() {{ return express(); }} }};\n{}",
                    "// padding to make the file a realistic size\n".repeat(40)
                );
                std::fs::write(path.join(format!("file{file}.ts")), text).unwrap();
            }
        }
        std::fs::write(
            dir.join("src/area0/file0.ts"),
            "import { f1 } from './file1';\nexport function changed() { return f1(1); }\n",
        )
        .unwrap();
        let changed = all(&["src/area0/file0.ts"]);

        let started = std::time::Instant::now();
        let map = build(&dir, &changed, None);
        let cold = started.elapsed();
        let started = std::time::Instant::now();
        let again = build(&dir, &changed, None);
        let warm = started.elapsed();
        println!("20 000 files: first map {cold:?}, next map {warm:?}, {} files in the tree", map.tree.len());
        assert_eq!(map, again);
        assert!(map.tree.len() > 20, "the files that import the changed one are found");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(cold.as_millis() < 1000, "the first map took {cold:?}");
        assert!(warm.as_millis() < 500, "the next map took {warm:?}");
    }
}
