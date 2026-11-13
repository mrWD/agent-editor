//! What the map knows about one source file, and the parts of reading a file
//! that the languages share.
//!
//! Why a scanner written by hand and not a parsing library: the map needs
//! little from a file (what it defines, what it imports, which names it uses)
//! and needs it fast, for thousands of files, in a core that must build on
//! three systems without a C compiler. A scanner that only understands where
//! strings, comments and brackets end is enough for that. Its limit is plain:
//! it reads names, not types, so a call is matched by its name.

use super::lex::{ALONE, Kind, Token};
use std::collections::{HashMap, HashSet};

/// The languages the map reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    /// JavaScript and TypeScript, with or without JSX.
    Js,
    Rust,
    Python,
    Go,
}

impl Lang {
    /// The language of a file, by its ending, and the name shown for it.
    pub fn of(path: &str) -> Option<(Lang, &'static str)> {
        let name = path.rsplit('/').next()?;
        let (_, ending) = name.rsplit_once('.')?;
        Some(match ending {
            "ts" | "tsx" | "mts" | "cts" => (Lang::Js, "TypeScript"),
            "js" | "jsx" | "mjs" | "cjs" => (Lang::Js, "JavaScript"),
            "rs" => (Lang::Rust, "Rust"),
            "py" | "pyi" => (Lang::Python, "Python"),
            "go" => (Lang::Go, "Go"),
            _ => return None,
        })
    }
}

/// One name a file uses: `rateLimit`, `res.status`, `gitops::state`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use {
    /// The name as written, with its dots or `::`. A leading dot means a method
    /// called on the result of something else, as in `a().b()`.
    pub chain: String,
    pub line: u32,
    /// True when it is followed by `(`: the name is called, not only mentioned.
    pub call: bool,
}

/// Something a file defines: a function, a constant, a class, a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Def {
    /// For a method this is `Type.method`, or `Type::method` in Rust.
    pub name: String,
    /// A plain word: "function", "const", "class", "type" and so on.
    pub kind: &'static str,
    pub line: u32,
    /// The last line of the definition.
    pub end: u32,
    pub uses: Vec<Use>,
}

impl Def {
    /// The part of the name after the type, for a method.
    pub fn method_name(&self) -> Option<&str> {
        self.name.rsplit_once("::").or_else(|| self.name.rsplit_once('.')).map(|(_, method)| method)
    }

    /// The type a method belongs to.
    pub fn owner(&self) -> Option<&str> {
        self.name.rsplit_once("::").or_else(|| self.name.rsplit_once('.')).map(|(owner, _)| owner)
    }

    /// Whether calling it is the usual thing to do with it.
    pub fn is_callable(&self) -> bool {
        self.kind == "function"
    }
}

/// One name an import brings into a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The name inside this file.
    pub local: String,
    /// What it is called where it comes from. `None`: the whole module.
    pub item: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// What is imported, as written: `./rateLimit`, `express`, `crate::gitops`, `os.path`.
    pub spec: String,
    pub line: u32,
    pub bindings: Vec<Binding>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub defs: Vec<Def>,
    pub imports: Vec<Import>,
    /// Names used outside every definition: in tests, in code that runs when the file loads.
    pub loose: Vec<Use>,
    /// How many test cases the file declares.
    pub tests: usize,
    /// Go only: the name after `package`.
    pub package: Option<String>,
}

impl Facts {
    pub fn def(&self, name: &str) -> Option<&Def> {
        self.defs.iter().find(|def| def.name == name)
    }
}

/// Reads one file. `None` for a language the map does not read.
pub fn scan(path: &str, text: &str) -> Option<Facts> {
    let (lang, _) = Lang::of(path)?;
    Some(match lang {
        Lang::Js => super::js::scan(text),
        Lang::Rust => super::rust::scan(text),
        Lang::Python => super::python::scan(text),
        Lang::Go => super::go::scan(path, text),
    })
}

/// How a language spells the things the shared code looks at.
pub struct Spelling {
    /// Words that are never the name of something used.
    pub keywords: &'static [&'static str],
    /// Words that may start a chain although they are keywords: `this`, `self`, `crate`.
    pub roots: &'static [&'static str],
    /// The word before a name that makes it a definition, not a call: `function`, `fn`, `def`, `func`.
    pub defines: &'static [&'static str],
    /// The words that give names to values inside a function: `const`, `let`, `var`.
    /// What follows them, up to the `=`, are names of the function's own.
    pub variables: &'static [&'static str],
    /// How a value is given a name without such a word: `:=` in Go, a plain `=` at the start of a line in Python.
    pub naming_sign: Option<&'static str>,
    /// True when `a::b` is a path, as in Rust.
    pub colons: bool,
    /// True when `name!(...)` is a macro and not a call, as in Rust.
    pub bang_macros: bool,
    /// True when `name(args) { ... }` inside an object or a class defines a method, as in JavaScript.
    pub short_methods: bool,
}

/// The token before `index`, skipping Rust's `?` so `a()?.b()` reads like `a().b()`.
fn before(tokens: &[Token], index: usize) -> Option<usize> {
    let mut at = index.checked_sub(1)?;
    while tokens[at].is("?") {
        at = at.checked_sub(1)?;
    }
    Some(at)
}

/// The names a `const`, `let` or `var` at `at` gives: the words up to its `=`.
/// A pattern in brackets gives every name in it: `const [a, setA] = ...`, `let (x, y) = ...`.
fn named_by<'a>(tokens: &[Token<'a>], at: usize, to: usize, spelling: &Spelling, own: &mut HashSet<&'a str>) {
    let mut depth = 0;
    // Outside brackets names come one at a time with commas between: a second word in a row is a type.
    let mut may_name = true;
    for token in tokens.iter().take(to.min(at + 40)).skip(at + 1) {
        match token.kind {
            Kind::Sign => match token.text {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" if depth == 0 => return,
                ")" | "]" | "}" => depth -= 1,
                "=" | ":" | ";" | "=>" if depth == 0 => return,
                "," => may_name = true,
                _ => {}
            },
            // `for (const x of list)`, `for x in list`: the names end where the list begins.
            Kind::Word if depth == 0 && matches!(token.text, "of" | "in") => return,
            // Other keywords among the names, such as `mut`, name nothing.
            Kind::Word if spelling.keywords.contains(&token.text) => {}
            Kind::Word if depth > 0 || may_name => {
                own.insert(token.text);
                may_name = depth > 0;
            }
            Kind::Word => return,
            _ => {}
        }
    }
}

/// The names before the sign at `at` that gives them a value: `a, b := f()`.
/// `line_start` asks that they be the first thing on their line, which tells
/// Python's `x = 1` from the `x=1` inside a call.
fn named_before<'a>(tokens: &[Token<'a>], at: usize, from: usize, line_start: bool, own: &mut HashSet<&'a str>) {
    let mut names = Vec::new();
    let mut index = at;
    while index > from && tokens[index - 1].is_word() {
        names.push(tokens[index - 1].text);
        index -= 1;
        if index > from && tokens[index - 1].is(",") {
            index -= 1;
        } else {
            break;
        }
    }
    let first_on_line = index == 0 || tokens[index - 1].line < tokens[index].line;
    if !names.is_empty() && (!line_start || first_on_line) {
        own.extend(names);
    }
}

/// Collects the names used between two tokens. A chain such as `a.b.c(` is one
/// use; a name is listed once, at its first line. Names the code gives to its
/// own values and helpers (`const user = ...`, `let total`, a function inside
/// the function) are left out: a call to one of them stays inside.
pub fn uses<'a>(tokens: &[Token<'a>], partner: &[usize], from: usize, to: usize, spelling: &Spelling) -> Vec<Use> {
    let mut own: HashSet<&'a str> = HashSet::new();
    let mut found: Vec<Use> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut note = |chain: String, line: u32, call: bool| match seen.get(&chain) {
        Some(index) => found[*index].call |= call,
        None => {
            seen.insert(chain.clone(), found.len());
            found.push(Use { chain, line, call });
        }
    };
    let joins = |token: &Token| token.is(".") || (spelling.colons && token.is("::"));
    let to = to.min(tokens.len());
    let mut at = from;

    while at < to {
        let token = &tokens[at];
        if token.kind != Kind::Word {
            // `a, b := f()` in Go; `a = f()` at the start of a line in Python.
            match spelling.naming_sign {
                Some(":=") if token.is(":") && tokens.get(at + 1).is_some_and(|next| next.is("=")) => {
                    named_before(tokens, at, from, false, &mut own);
                }
                Some("=") if token.is("=") && !tokens.get(at + 1).is_some_and(|next| next.is("=")) => {
                    named_before(tokens, at, from, true, &mut own);
                }
                _ => {}
            }
            at += 1;
            continue;
        }
        let previous = at.checked_sub(1).map(|index| &tokens[index]);
        if spelling.variables.contains(&token.text) && !previous.is_some_and(joins) {
            named_by(tokens, at, to, spelling, &mut own);
        }

        // A name after a dot belongs to a chain that began earlier. Only a
        // method called on a result, `a().b()`, starts here.
        if previous.is_some_and(joins) {
            let called = tokens.get(at + 1).is_some_and(|next| next.is("("));
            let on_a_result =
                before(tokens, at - 1).is_some_and(|index| tokens[index].is(")") || tokens[index].is("]"));
            if called && on_a_result && previous.is_some_and(|p| p.is(".")) {
                note(format!(".{}", token.text), token.line, true);
            }
            at += 1;
            continue;
        }
        if spelling.keywords.contains(&token.text) {
            // `this.x`, `self.x`, `crate::x` are chains worth keeping; alone they say nothing.
            let starts_chain = spelling.roots.contains(&token.text) && tokens.get(at + 1).is_some_and(joins);
            if !starts_chain {
                at += 1;
                continue;
            }
        }
        // A name that is being defined is not being used. It is one of the code's own from here on.
        if previous.is_some_and(|p| p.is_word() && spelling.defines.contains(&p.text)) {
            own.insert(token.text);
            at += 1;
            continue;
        }
        // `{ key: value }` and `name: Type` name a place, not a thing in scope.
        let next_is_colon = tokens.get(at + 1).is_some_and(|next| next.is(":"));
        if next_is_colon && previous.is_some_and(|p| p.is("{") || p.is(",")) {
            at += 1;
            continue;
        }

        // Follow the chain: `a.b.c`, `a::b::c`.
        let mut chain = String::from(token.text);
        let mut parts = 1;
        let mut end = at + 1;
        let mut cut = false;
        while end + 1 < to && joins(&tokens[end]) && tokens[end + 1].is_word() {
            if parts < 4 {
                chain.push_str(tokens[end].text);
                chain.push_str(tokens[end + 1].text);
                parts += 1;
            } else {
                // Longer chains are cut: the map shows where a call starts, not every step of it.
                cut = true;
            }
            end += 2;
        }
        let next = tokens.get(end);
        let opens = |index: usize| tokens.get(index).is_some_and(|t| t.is("(") || t.is("[") || t.is("{"));
        if spelling.bang_macros && next.is_some_and(|n| n.is("!")) && opens(end + 1) {
            // A macro such as `json!(...)`. What is inside it is read as it comes.
            at = end + 1;
            continue;
        }
        let call = !cut && end < to && next.is_some_and(|n| n.is("("));
        // `name(args) {` is a method written in an object or a class, not a call.
        if call && parts == 1 && spelling.short_methods {
            let close = partner[end];
            let opens_body = close != ALONE && tokens.get(close + 1).is_some_and(|after| after.is("{"));
            let is_new = previous.is_some_and(|p| p.word("new"));
            if opens_body && !is_new {
                at = end;
                continue;
            }
        }
        note(chain, token.line, call);
        at = end;
    }
    // Only the bare name: `user.save()` on a value of the code's own is still a call worth showing.
    found.retain(|used| !own.contains(used.chain.as_str()));
    found
}

/// The partner of the bracket at `index`, or the last token when it has none.
pub fn close_of(tokens: &[Token], partner: &[usize], index: usize) -> usize {
    match partner.get(index) {
        Some(&at) if at != ALONE && at > index => at,
        _ => tokens.len().saturating_sub(1),
    }
}

/// The line a definition ends on: the line of its last token.
pub fn line_at(tokens: &[Token], index: usize) -> u32 {
    tokens.get(index).or(tokens.last()).map_or(1, |token| token.line)
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::path::PathBuf;

    /// A project on disk with the given files, removed when dropped.
    pub struct Tree(pub PathBuf);

    impl Tree {
        pub fn new(name: &str, files: &[(&str, &str)]) -> Self {
            let dir = std::env::temp_dir().join(format!("ae-map-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            for (path, text) in files {
                let full = dir.join(path);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(full, text).unwrap();
            }
            Self(dir.canonicalize().unwrap())
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// "kind name" for every definition, in the order found.
    pub fn defs(facts: &Facts) -> Vec<String> {
        facts.defs.iter().map(|def| format!("{} {}", def.kind, def.name)).collect()
    }

    /// "spec: local<-item, ..." for every import.
    pub fn imports(facts: &Facts) -> Vec<String> {
        facts
            .imports
            .iter()
            .map(|import| {
                let names: Vec<String> = import
                    .bindings
                    .iter()
                    .map(|b| match &b.item {
                        Some(item) if *item == b.local => b.local.clone(),
                        Some(item) => format!("{}<-{item}", b.local),
                        None => format!("{}<-*", b.local),
                    })
                    .collect();
                if names.is_empty() { import.spec.clone() } else { format!("{}: {}", import.spec, names.join(", ")) }
            })
            .collect()
    }

    /// The calls a definition makes, as written.
    pub fn calls(facts: &Facts, name: &str) -> Vec<String> {
        let def = facts.def(name).unwrap_or_else(|| panic!("no definition called {name}"));
        def.uses.iter().filter(|u| u.call).map(|u| u.chain.clone()).collect()
    }

    /// Every name a definition uses, called or not.
    pub fn names(facts: &Facts, name: &str) -> Vec<String> {
        let def = facts.def(name).unwrap_or_else(|| panic!("no definition called {name}"));
        def.uses.iter().map(|u| u.chain.clone()).collect()
    }

    pub fn lines(facts: &Facts, name: &str) -> (u32, u32) {
        let def = facts.def(name).unwrap_or_else(|| panic!("no definition called {name}"));
        (def.line, def.end)
    }
}
