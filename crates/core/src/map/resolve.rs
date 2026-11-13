//! Finds what an import leads to: a file of the project, a package from
//! outside, or a part of the language itself.
//!
//! Each language has its own rules for turning the words of an import into a
//! file. They are followed as far as a look at the disk can tell; what cannot
//! be told is reported as unknown, never guessed.

use super::scan::Lang;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// What an import leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A file of the project, as a path from its top folder, and the name
    /// inside it that was meant when the import names one.
    File { path: String, item: Option<String> },
    /// A folder of the project that is one Go package.
    Folder(String),
    /// Something installed from outside: an npm package, a crate, a Go module.
    Package(String),
    /// A part of the language, such as `std` or `node:fs`. The map leaves these out.
    Builtin,
    /// Nothing the scanner can place.
    Unknown,
}

/// Endings tried for an import that names no ending, in the order tools try them.
const JS_ENDINGS: &[&str] = &["ts", "tsx", "d.ts", "js", "jsx", "mjs", "cjs", "mts", "cts"];

/// Modules that come with Node.
const NODE: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

/// Modules that come with Python.
const PYTHON: &[&str] = &[
    "__future__",
    "abc",
    "argparse",
    "array",
    "ast",
    "asyncio",
    "atexit",
    "base64",
    "binascii",
    "bisect",
    "builtins",
    "bz2",
    "calendar",
    "cmath",
    "codecs",
    "collections",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "csv",
    "ctypes",
    "dataclasses",
    "datetime",
    "decimal",
    "difflib",
    "dis",
    "email",
    "enum",
    "errno",
    "faulthandler",
    "fnmatch",
    "fractions",
    "functools",
    "gc",
    "getpass",
    "gettext",
    "glob",
    "graphlib",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "linecache",
    "locale",
    "logging",
    "lzma",
    "math",
    "mimetypes",
    "multiprocessing",
    "numbers",
    "operator",
    "os",
    "pathlib",
    "pickle",
    "pkgutil",
    "platform",
    "plistlib",
    "pprint",
    "queue",
    "random",
    "re",
    "reprlib",
    "sched",
    "secrets",
    "select",
    "selectors",
    "shlex",
    "shutil",
    "signal",
    "site",
    "smtplib",
    "socket",
    "socketserver",
    "sqlite3",
    "ssl",
    "stat",
    "statistics",
    "string",
    "struct",
    "subprocess",
    "sys",
    "sysconfig",
    "tarfile",
    "tempfile",
    "textwrap",
    "threading",
    "time",
    "timeit",
    "tkinter",
    "token",
    "tokenize",
    "tomllib",
    "trace",
    "traceback",
    "types",
    "typing",
    "unicodedata",
    "unittest",
    "urllib",
    "uuid",
    "venv",
    "warnings",
    "weakref",
    "xml",
    "zipfile",
    "zlib",
    "zoneinfo",
];

/// A path made of plain parts, with `.` and `..` worked out. `None` when it leaves the top folder.
fn tidy(path: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn folder_of(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(folder, _)| folder)
}

fn join(folder: &str, rest: &str) -> String {
    if folder.is_empty() { rest.to_string() } else { format!("{folder}/{rest}") }
}

/// What a TypeScript project says about where its imports start: the folder
/// `baseUrl` names and the patterns of `paths`.
#[derive(Debug, Default, Clone)]
struct TsPaths {
    /// The folder the patterns count from, from the top folder of the project.
    base: Option<String>,
    /// `("@/", "", ["src/"])`: what an import starts and ends with, and what replaces the star.
    patterns: Vec<(String, String, Vec<String>)>,
}

/// What `Cargo.toml` says about a crate.
#[derive(Debug, Clone)]
struct Crate {
    /// The folder with `Cargo.toml`, from the top folder of the project.
    folder: String,
    /// The name code uses for the crate: dashes become underscores.
    name: String,
}

/// Answers "what does this import lead to" for one project, and remembers the
/// project files it had to read for that while one map is made.
pub struct Resolver<'a> {
    root: &'a Path,
    ts_paths: HashMap<String, TsPaths>,
    crates: HashMap<String, Option<Crate>>,
    go_modules: HashMap<String, Option<(String, String)>>,
    exists: HashMap<String, bool>,
}

impl<'a> Resolver<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self {
            root,
            ts_paths: HashMap::new(),
            crates: HashMap::new(),
            go_modules: HashMap::new(),
            exists: HashMap::new(),
        }
    }

    fn is_file(&mut self, path: &str) -> bool {
        if let Some(known) = self.exists.get(path) {
            return *known;
        }
        let found = self.root.join(path).is_file();
        self.exists.insert(path.to_string(), found);
        found
    }

    fn is_folder(&self, path: &str) -> bool {
        self.root.join(path).is_dir()
    }

    /// What `spec`, imported by `from`, leads to.
    pub fn resolve(&mut self, lang: Lang, from: &str, spec: &str) -> Target {
        match lang {
            Lang::Js => self.js(from, spec),
            Lang::Python => self.python(from, spec, None),
            Lang::Rust => self.rust(from, spec),
            Lang::Go => self.go(from, spec),
        }
    }

    // ----- JavaScript and TypeScript -----

    /// `base` as it is, with each ending, and as a folder with an `index` file.
    fn js_file(&mut self, base: &str) -> Option<String> {
        if base.rsplit('/').next().is_some_and(|name| name.contains('.')) && self.is_file(base) {
            return Some(base.to_string());
        }
        for ending in JS_ENDINGS {
            let candidate = format!("{base}.{ending}");
            if self.is_file(&candidate) {
                return Some(candidate);
            }
        }
        // TypeScript code imports `./x.js` and means `./x.ts`.
        for (written, real) in
            [(".js", ["ts", "tsx"]), (".jsx", ["tsx", "ts"]), (".mjs", ["mts", "ts"]), (".cjs", ["cts", "ts"])]
        {
            if let Some(stem) = base.strip_suffix(written) {
                for ending in real {
                    let candidate = format!("{stem}.{ending}");
                    if self.is_file(&candidate) {
                        return Some(candidate);
                    }
                }
            }
        }
        for ending in JS_ENDINGS {
            let candidate = format!("{base}/index.{ending}");
            if self.is_file(&candidate) {
                return Some(candidate);
            }
        }
        // A file written without any ending, such as `./LICENSE`.
        if self.is_file(base) { Some(base.to_string()) } else { None }
    }

    /// The nearest `tsconfig.json` or `jsconfig.json` above `folder`, read for its paths.
    fn ts_paths_for(&mut self, folder: &str) -> TsPaths {
        if let Some(known) = self.ts_paths.get(folder) {
            return known.clone();
        }
        let mut found = None;
        for name in ["tsconfig.json", "jsconfig.json"] {
            if let Ok(text) = std::fs::read_to_string(self.root.join(join(folder, name))) {
                found = Some(read_ts_paths(folder, &text));
                break;
            }
        }
        let paths = match found {
            Some(paths) => paths,
            None if folder.is_empty() => TsPaths::default(),
            None => self.ts_paths_for(folder_of(folder)),
        };
        self.ts_paths.insert(folder.to_string(), paths.clone());
        paths
    }

    fn js(&mut self, from: &str, spec: &str) -> Target {
        let clean = spec.split(['?', '#']).next().unwrap_or(spec);
        if clean == "." || clean == ".." || clean.starts_with("./") || clean.starts_with("../") {
            let base = tidy(&PathBuf::from(folder_of(from)).join(clean));
            return match base.and_then(|base| self.js_file(&base)) {
                Some(path) => Target::File { path, item: None },
                None => Target::Unknown,
            };
        }
        if clean.starts_with('/') || clean.is_empty() {
            return Target::Unknown;
        }
        // A project may name its own folders: `@/components/Button`.
        let paths = self.ts_paths_for(folder_of(from));
        for (prefix, suffix, replacements) in &paths.patterns {
            let Some(star) = clean.strip_prefix(prefix.as_str()).and_then(|rest| rest.strip_suffix(suffix.as_str()))
            else {
                continue;
            };
            for replacement in replacements {
                let base = replacement.replace('*', star);
                if let Some(path) = tidy(Path::new(&base)).and_then(|base| self.js_file(&base)) {
                    return Target::File { path, item: None };
                }
            }
        }
        if let Some(base) = &paths.base
            && let Some(path) = tidy(Path::new(&join(base, clean))).and_then(|base| self.js_file(&base))
        {
            return Target::File { path, item: None };
        }
        if clean.starts_with("node:") || NODE.contains(&clean.split('/').next().unwrap_or(clean)) {
            return Target::Builtin;
        }
        // An npm name: `name`, `name/part`, `@scope/name`, `@scope/name/part`.
        let mut parts = clean.split('/');
        let first = parts.next().unwrap_or_default();
        let name = match first.strip_prefix('@') {
            Some(scope) if !scope.is_empty() => match parts.next() {
                Some(second) if !second.is_empty() => format!("{first}/{second}"),
                _ => return Target::Unknown,
            },
            Some(_) => return Target::Unknown,
            None => first.to_string(),
        };
        let looks_like_a_package =
            name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '/'))
                && !name.starts_with('.');
        if looks_like_a_package { Target::Package(name) } else { Target::Unknown }
    }

    // ----- Python -----

    /// The file of the module `parts` under `base`: `a/b.py` or `a/b/__init__.py`.
    fn python_module(&mut self, base: &str, parts: &[&str]) -> Option<String> {
        if parts.is_empty() {
            let package = join(base, "__init__.py");
            return self.is_file(&package).then_some(package);
        }
        let path = join(base, &parts.join("/"));
        [format!("{path}.py"), format!("{path}/__init__.py"), format!("{path}.pyi")]
            .into_iter()
            .find(|candidate| self.is_file(candidate))
    }

    /// What `from spec import name` leads to. A name may be a module of its own
    /// (`from app import users` for `app/users.py`), which wins over a name inside `app`.
    pub fn python(&mut self, from: &str, spec: &str, name: Option<&str>) -> Target {
        let dots = spec.bytes().take_while(|b| *b == b'.').count();
        let rest: Vec<&str> = spec[dots..].split('.').filter(|part| !part.is_empty()).collect();
        let mut bases: Vec<String> = Vec::new();
        if dots > 0 {
            // One dot is the folder of the importing file; each further dot is one folder up.
            let mut folder = folder_of(from).to_string();
            for _ in 1..dots {
                if folder.is_empty() {
                    return Target::Unknown;
                }
                folder = folder_of(&folder).to_string();
            }
            bases.push(folder);
        } else {
            // A name from the top is looked for where Python programs usually start:
            // next to the importing file and in every folder above it, then in `src`.
            let mut folder = folder_of(from).to_string();
            loop {
                bases.push(folder.clone());
                if folder.is_empty() {
                    break;
                }
                folder = folder_of(&folder).to_string();
            }
            bases.push("src".to_string());
        }
        for base in &bases {
            if let Some(name) = name {
                let mut longer = rest.clone();
                longer.push(name);
                if let Some(path) = self.python_module(base, &longer) {
                    return Target::File { path, item: None };
                }
            }
            if let Some(path) = self.python_module(base, &rest) {
                return Target::File { path, item: name.map(str::to_string) };
            }
            // A folder without `__init__.py` is still a package to newer Pythons.
            if !rest.is_empty() && name.is_none() && self.is_folder(&join(base, &rest.join("/"))) {
                return Target::Unknown;
            }
        }
        if dots > 0 {
            return Target::Unknown;
        }
        match rest.first() {
            Some(top) if PYTHON.contains(top) => Target::Builtin,
            Some(top) => Target::Package((*top).to_string()),
            None => Target::Unknown,
        }
    }

    // ----- Rust -----

    /// The crate a file belongs to: the nearest folder above it with a `Cargo.toml`.
    fn crate_of(&mut self, folder: &str) -> Option<Crate> {
        if let Some(known) = self.crates.get(folder) {
            return known.clone();
        }
        let found = match std::fs::read_to_string(self.root.join(join(folder, "Cargo.toml"))) {
            Ok(text) => Some(Crate { folder: folder.to_string(), name: crate_name(&text, folder) }),
            Err(_) if folder.is_empty() => None,
            Err(_) => self.crate_of(folder_of(folder)),
        };
        self.crates.insert(folder.to_string(), found.clone());
        found
    }

    /// The file of the module `name` whose files live in `folder`.
    fn rust_module(&mut self, folder: &str, name: &str) -> Option<String> {
        [join(folder, &format!("{name}.rs")), join(folder, &format!("{name}/mod.rs"))]
            .into_iter()
            .find(|candidate| self.is_file(candidate))
    }

    fn rust(&mut self, from: &str, spec: &str) -> Target {
        let file_name = from.rsplit('/').next().unwrap_or(from);
        let here = folder_of(from);
        let krate = self.crate_of(here);
        let src = krate.as_ref().map(|krate| join(&krate.folder, "src"));
        let in_src = src.as_ref().is_some_and(|src| from.starts_with(&format!("{src}/")));
        // Where the modules declared by this file live. `a/mod.rs`, `lib.rs` and
        // `main.rs` keep theirs next to them; `a/b.rs` keeps them in `a/b/`.
        let is_root = !in_src
            || src.as_ref().is_some_and(|src| folder_of(from) == src && matches!(file_name, "lib.rs" | "main.rs"));
        let children = if is_root || file_name == "mod.rs" {
            here.to_string()
        } else {
            join(here, file_name.trim_end_matches(".rs"))
        };
        // The file `crate` means: the library's first file, or this file when it is a crate of its own (a test, an example).
        let crate_folder = if in_src { src.clone().unwrap_or_default() } else { here.to_string() };
        let crate_file = if in_src {
            ["lib.rs", "main.rs"].iter().map(|name| join(&crate_folder, name)).find(|path| self.is_file(path))
        } else {
            Some(from.to_string())
        };

        if let Some(name) = spec.strip_prefix("mod ") {
            return match self.rust_module(&children, name) {
                Some(path) => Target::File { path, item: None },
                None => Target::Unknown,
            };
        }
        let parts: Vec<&str> = spec.split("::").filter(|part| !part.is_empty()).collect();
        let Some(first) = parts.first().copied() else { return Target::Unknown };
        // `folder` is where the next module of the path is looked for; `file` is the module reached so far.
        let (mut folder, mut file, mut rest): (String, Option<String>, &[&str]) = match first {
            "crate" => (crate_folder, crate_file, &parts[1..]),
            "self" => (children, Some(from.to_string()), &parts[1..]),
            "super" => {
                let mut folder = children;
                let mut rest = &parts[..];
                while rest.first() == Some(&"super") {
                    if is_root && folder == here {
                        return Target::Unknown;
                    }
                    folder = folder_of(&folder).to_string();
                    rest = &rest[1..];
                }
                // The module above: the file that owns `folder`.
                let owner = folder.rsplit('/').next().unwrap_or_default().to_string();
                let above = folder_of(&folder).to_string();
                let file = if Some(&folder) == src.as_ref() || folder == crate_folder {
                    crate_file
                } else {
                    self.rust_module(&above, &owner)
                };
                (folder, file, rest)
            }
            "std" | "core" | "alloc" | "proc_macro" | "test" => return Target::Builtin,
            name => {
                if let Some(path) = self.rust_module(&children, name) {
                    // A module declared by this file, used without `self::`.
                    (join(&children, name), Some(path), &parts[1..])
                } else if let Some(krate) = krate.as_ref().filter(|krate| krate.name == name) {
                    // The crate's own library, named from a test or a binary of the same package.
                    let folder = join(&krate.folder, "src");
                    let file =
                        ["lib.rs", "main.rs"].iter().map(|name| join(&folder, name)).find(|path| self.is_file(path));
                    (folder, file, &parts[1..])
                } else {
                    return Target::Package(name.to_string());
                }
            }
        };
        // Walk down while the next part of the path is a module with a file of its own.
        while let Some(part) = rest.first() {
            match self.rust_module(&folder, part) {
                Some(path) => {
                    folder = join(&folder, part);
                    file = Some(path);
                    rest = &rest[1..];
                }
                None => break,
            }
        }
        match file {
            // What is left of the path names something inside that file: `Manager`, `Manager::new`.
            Some(path) => Target::File { path, item: (!rest.is_empty()).then(|| rest.join("::")) },
            None => Target::Unknown,
        }
    }

    // ----- Go -----

    /// The module a file belongs to: the folder of the nearest `go.mod` above it and the module's name.
    fn go_module(&mut self, folder: &str) -> Option<(String, String)> {
        if let Some(known) = self.go_modules.get(folder) {
            return known.clone();
        }
        let found = match std::fs::read_to_string(self.root.join(join(folder, "go.mod"))) {
            Ok(text) => text
                .lines()
                .find_map(|line| line.trim().strip_prefix("module "))
                .map(|name| (folder.to_string(), name.trim().trim_matches('"').to_string())),
            Err(_) if folder.is_empty() => None,
            Err(_) => self.go_module(folder_of(folder)),
        };
        self.go_modules.insert(folder.to_string(), found.clone());
        found
    }

    /// The path other Go files import the package in `folder` by.
    pub fn go_import_path(&mut self, folder: &str) -> Option<String> {
        let (module_folder, module) = self.go_module(folder)?;
        let inside = folder.strip_prefix(module_folder.as_str())?.trim_start_matches('/');
        Some(if inside.is_empty() { module } else { format!("{module}/{inside}") })
    }

    fn go(&mut self, from: &str, spec: &str) -> Target {
        if let Some((folder, module)) = self.go_module(folder_of(from)) {
            let inside = if spec == module { Some("") } else { spec.strip_prefix(&format!("{module}/")) };
            if let Some(inside) = inside {
                let target = join(&folder, inside);
                return if self.is_folder(&target) { Target::Folder(target) } else { Target::Unknown };
            }
        }
        // By Go's own rule a path whose first part has no dot belongs to the language.
        let first = spec.split('/').next().unwrap_or(spec);
        if !first.contains('.') {
            return Target::Builtin;
        }
        // A module is named by its first three parts on the usual hosts.
        let parts: Vec<&str> = spec.split('/').collect();
        let keep = if first == "gopkg.in" { 2 } else { 3 };
        let name = parts.iter().take(keep).copied().collect::<Vec<_>>().join("/");
        Target::Package(name)
    }
}

/// Reads `baseUrl` and `paths` out of a tsconfig. Such files may hold comments
/// and commas at the end of a list, which strict JSON does not allow.
fn read_ts_paths(folder: &str, text: &str) -> TsPaths {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&loosen_json(text)) else {
        return TsPaths::default();
    };
    let options = &value["compilerOptions"];
    let base_url = options["baseUrl"].as_str();
    let base = join(folder, base_url.unwrap_or("."));
    let mut paths = TsPaths { base: base_url.and_then(|_| tidy(Path::new(&base))), patterns: vec![] };
    if let Some(map) = options["paths"].as_object() {
        for (pattern, targets) in map {
            let (prefix, suffix) = pattern.split_once('*').unwrap_or((pattern, ""));
            // A pattern without a star must match the whole import.
            if !pattern.contains('*') {
                continue;
            }
            let replacements = targets
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|target| target.as_str())
                .map(|target| join(&base, target))
                .collect();
            paths.patterns.push((prefix.to_string(), suffix.to_string(), replacements));
        }
    }
    paths
}

/// Takes comments and trailing commas out of JSON that allows them.
fn loosen_json(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    let mut in_string = false;
    while at < bytes.len() {
        let byte = bytes[at];
        if in_string {
            let width = if byte == b'\\' { 2 } else { 1 };
            in_string = byte != b'"';
            let end = (at + width).min(bytes.len());
            // Strings are copied whole, so a character of several bytes is never cut.
            let end = (end..=bytes.len()).find(|index| text.is_char_boundary(*index)).unwrap_or(bytes.len());
            out.push_str(&text[at..end]);
            at = end;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                out.push('"');
                at += 1;
            }
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at += 2;
                while at < bytes.len() && !(bytes[at] == b'*' && bytes.get(at + 1) == Some(&b'/')) {
                    at += 1;
                }
                at = (at + 2).min(bytes.len());
            }
            b',' => {
                // A comma is dropped when only blanks stand between it and a closing bracket.
                let next = bytes[at + 1..].iter().find(|b| !b.is_ascii_whitespace());
                if !matches!(next, Some(b'}' | b']')) {
                    out.push(',');
                }
                at += 1;
            }
            _ => {
                let end = (at + 1..=bytes.len()).find(|index| text.is_char_boundary(*index)).unwrap_or(bytes.len());
                out.push_str(&text[at..end]);
                at = end;
            }
        }
    }
    out
}

/// The name code uses for a crate: `[lib] name` when given, else the package's name with dashes as underscores.
fn crate_name(cargo_toml: &str, folder: &str) -> String {
    let mut section = "";
    let mut package = None;
    let mut lib = None;
    for line in cargo_toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let Some(value) = line.strip_prefix("name").map(str::trim_start).and_then(|rest| rest.strip_prefix('=')) else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match section {
            "[package]" => package = Some(value),
            "[lib]" => lib = Some(value),
            _ => {}
        }
    }
    let name = lib.or(package).unwrap_or_else(|| folder.rsplit('/').next().unwrap_or(folder).to_string());
    name.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::super::scan::testing::Tree;
    use super::*;

    fn file(path: &str) -> Target {
        Target::File { path: path.into(), item: None }
    }

    fn item(path: &str, item: &str) -> Target {
        Target::File { path: path.into(), item: Some(item.into()) }
    }

    #[test]
    fn javascript_imports_lead_to_files_packages_or_the_language() {
        let tree = Tree::new(
            "resolve-js",
            &[
                ("src/routes/auth.ts", ""),
                ("src/middleware/rateLimit.ts", ""),
                ("src/services/index.ts", ""),
                ("src/view.tsx", ""),
                ("src/data.json", "{}"),
                ("src/old.js", ""),
                (
                    "tsconfig.json",
                    "{\n  // where imports start\n  \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@/*\": [\"src/*\"], } },\n}\n",
                ),
            ],
        );
        let mut resolver = Resolver::new(&tree.0);
        let mut to = |spec: &str| resolver.resolve(Lang::Js, "src/routes/auth.ts", spec);
        assert_eq!(to("../middleware/rateLimit"), file("src/middleware/rateLimit.ts"));
        assert_eq!(
            to("../middleware/rateLimit.js"),
            file("src/middleware/rateLimit.ts"),
            "a .js ending means the .ts file"
        );
        assert_eq!(to("../services"), file("src/services/index.ts"));
        assert_eq!(to("../view"), file("src/view.tsx"));
        assert_eq!(to("../data.json"), file("src/data.json"));
        assert_eq!(to("../old.js"), file("src/old.js"));
        assert_eq!(to("@/middleware/rateLimit"), file("src/middleware/rateLimit.ts"), "a path from tsconfig");
        assert_eq!(to("src/view"), file("src/view.tsx"), "counted from baseUrl");
        assert_eq!(to("express-rate-limit"), Target::Package("express-rate-limit".into()));
        assert_eq!(to("lodash/fp"), Target::Package("lodash".into()));
        assert_eq!(to("@scope/pkg/deep"), Target::Package("@scope/pkg".into()));
        assert_eq!(to("node:fs"), Target::Builtin);
        assert_eq!(to("path"), Target::Builtin);
        assert_eq!(to("./missing"), Target::Unknown);
        assert_eq!(to("../../../outside"), Target::Unknown, "nothing above the project is looked at");
        assert_eq!(to("@/nothing/here"), Target::Unknown, "an alias that leads nowhere is not called a package");
    }

    #[test]
    fn python_imports_follow_dots_and_folders() {
        let tree = Tree::new(
            "resolve-py",
            &[
                ("app/__init__.py", ""),
                ("app/routes/auth.py", ""),
                ("app/routes/__init__.py", ""),
                ("app/services/users.py", ""),
                ("app/config.py", ""),
                ("src/tools/clean.py", ""),
            ],
        );
        let mut resolver = Resolver::new(&tree.0);
        let from = "app/routes/auth.py";
        assert_eq!(resolver.python(from, "..services.users", None), file("app/services/users.py"));
        assert_eq!(resolver.python(from, "..services", Some("users")), file("app/services/users.py"));
        assert_eq!(resolver.python(from, "..config", Some("limits")), item("app/config.py", "limits"));
        assert_eq!(resolver.python(from, ".", Some("helpers")), item("app/routes/__init__.py", "helpers"));
        assert_eq!(resolver.python(from, "app.services.users", None), file("app/services/users.py"));
        assert_eq!(resolver.python(from, "app", Some("config")), file("app/config.py"));
        assert_eq!(resolver.python(from, "tools.clean", None), file("src/tools/clean.py"));
        assert_eq!(resolver.python(from, "os.path", None), Target::Builtin);
        assert_eq!(resolver.python(from, "requests", Some("get")), Target::Package("requests".into()));
        assert_eq!(resolver.python(from, "....too.far", None), Target::Unknown);
    }

    #[test]
    fn rust_paths_follow_the_module_tree() {
        let tree = Tree::new(
            "resolve-rs",
            &[
                ("Cargo.toml", "[workspace]\nmembers = [\"crates/core\"]\n"),
                ("crates/core/Cargo.toml", "[package]\nname = \"ae-core\"\n\n[lib]\nname = \"ae_core\"\n"),
                ("crates/core/src/lib.rs", ""),
                ("crates/core/src/main.rs", ""),
                ("crates/core/src/gitops.rs", ""),
                ("crates/core/src/map.rs", ""),
                ("crates/core/src/map/lex.rs", ""),
                ("crates/core/src/map/scan.rs", ""),
                ("crates/core/src/hosts/mod.rs", ""),
                ("crates/core/src/hosts/github.rs", ""),
                ("crates/core/tests/code.rs", ""),
                ("crates/core/tests/common/mod.rs", ""),
            ],
        );
        let mut resolver = Resolver::new(&tree.0);
        let src = "crates/core/src";
        let mut to = |from: &str, spec: &str| resolver.resolve(Lang::Rust, from, spec);
        let changes = format!("{src}/changes.rs");
        assert_eq!(to(&changes, "crate::gitops"), file(&format!("{src}/gitops.rs")));
        assert_eq!(to(&changes, "crate::gitops::BranchState"), item(&format!("{src}/gitops.rs"), "BranchState"));
        assert_eq!(to(&changes, "crate::map::lex::Token"), item(&format!("{src}/map/lex.rs"), "Token"));
        assert_eq!(to(&changes, "crate::Thing"), item(&format!("{src}/lib.rs"), "Thing"));
        assert_eq!(to(&changes, "super::gitops::state"), item(&format!("{src}/gitops.rs"), "state"));
        assert_eq!(to(&format!("{src}/lib.rs"), "mod gitops"), file(&format!("{src}/gitops.rs")));
        assert_eq!(to(&format!("{src}/lib.rs"), "mod hosts"), file(&format!("{src}/hosts/mod.rs")));
        assert_eq!(to(&format!("{src}/map.rs"), "mod lex"), file(&format!("{src}/map/lex.rs")));
        assert_eq!(to(&format!("{src}/map.rs"), "lex::Token"), item(&format!("{src}/map/lex.rs"), "Token"));
        assert_eq!(to(&format!("{src}/map/scan.rs"), "super::lex::Kind"), item(&format!("{src}/map/lex.rs"), "Kind"));
        assert_eq!(to(&format!("{src}/map/scan.rs"), "super::super::gitops"), file(&format!("{src}/gitops.rs")));
        assert_eq!(
            to(&format!("{src}/hosts/mod.rs"), "self::github::Api"),
            item(&format!("{src}/hosts/github.rs"), "Api")
        );
        assert_eq!(
            to(&format!("{src}/hosts/github.rs"), "super::Hosts"),
            item(&format!("{src}/hosts/mod.rs"), "Hosts")
        );
        assert_eq!(to(&changes, "std::collections::HashMap"), Target::Builtin);
        assert_eq!(to(&changes, "serde_json::Value"), Target::Package("serde_json".into()));
        // A test is a crate of its own: `crate` is the test file, the library goes by its name.
        let test = "crates/core/tests/code.rs";
        assert_eq!(to(test, "ae_core::gitops::state"), item(&format!("{src}/gitops.rs"), "state"));
        assert_eq!(to(test, "mod common"), file("crates/core/tests/common/mod.rs"));
        assert_eq!(to(test, "common::Sandbox"), item("crates/core/tests/common/mod.rs", "Sandbox"));
        assert_eq!(to(test, "mod missing"), Target::Unknown);
    }

    #[test]
    fn go_imports_lead_to_folders_of_the_module_packages_or_the_language() {
        let tree = Tree::new(
            "resolve-go",
            &[("go.mod", "module example.com/shop\n\ngo 1.22\n"), ("main.go", ""), ("internal/limits/limits.go", "")],
        );
        let mut resolver = Resolver::new(&tree.0);
        let mut to = |spec: &str| resolver.resolve(Lang::Go, "main.go", spec);
        assert_eq!(to("example.com/shop/internal/limits"), Target::Folder("internal/limits".into()));
        assert_eq!(to("example.com/shop/internal/gone"), Target::Unknown);
        assert_eq!(to("net/http"), Target::Builtin);
        assert_eq!(to("github.com/gorilla/mux"), Target::Package("github.com/gorilla/mux".into()));
        assert_eq!(to("github.com/aws/aws-sdk-go/service/s3"), Target::Package("github.com/aws/aws-sdk-go".into()));
        assert_eq!(to("gopkg.in/yaml.v3"), Target::Package("gopkg.in/yaml.v3".into()));
    }

    #[test]
    fn json_with_comments_and_trailing_commas_is_read() {
        let loose = "{ /* a */ \"a\": [1, 2,], // b\n \"b\": \"x // not a comment, }\", }";
        let value: serde_json::Value = serde_json::from_str(&loosen_json(loose)).unwrap();
        assert_eq!(value["a"][1], 2);
        assert_eq!(value["b"], "x // not a comment, }");
    }
}
