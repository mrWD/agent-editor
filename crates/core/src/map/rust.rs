//! Rust.
//!
//! Read: `use` and `mod` declarations, paths written in full inside code
//! (`crate::files::list`), functions, the functions of `impl` blocks (named
//! `Type::function`), structs, enums, traits, constants, statics and type
//! names. Tests (`#[test]` and friends, and everything in a `#[cfg(test)]`
//! module) are counted, not listed.

use super::lex::{Kind, Syntax, Token, lex, pairs};
use super::scan::{Binding, Def, Facts, Import, Spelling, Use, close_of, line_at, uses};

const KEYWORDS: &[&str] = &[
    "Self", "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
    "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
];

pub const SPELLING: Spelling = Spelling {
    keywords: KEYWORDS,
    roots: &["self", "Self", "crate", "super"],
    defines: &["fn"],
    variables: &["let"],
    naming_sign: None,
    colons: true,
    bang_macros: true,
    short_methods: false,
};

/// Names every Rust file has without importing them. A call to one of them
/// tells nothing about the project, so the map leaves it out.
pub fn builtin(root: &str) -> bool {
    const NAMES: &[&str] = &[
        "AsMut",
        "AsRef",
        "Box",
        "Clone",
        "Copy",
        "Default",
        "Drop",
        "Eq",
        "Err",
        "Fn",
        "FnMut",
        "FnOnce",
        "From",
        "Into",
        "Iterator",
        "None",
        "Ok",
        "Option",
        "Ord",
        "PartialEq",
        "PartialOrd",
        "Result",
        "Send",
        "Sized",
        "Some",
        "String",
        "Sync",
        "ToOwned",
        "ToString",
        "Vec",
        "bool",
        "char",
        "drop",
        "f32",
        "f64",
        "i128",
        "i16",
        "i32",
        "i64",
        "i8",
        "isize",
        "str",
        "u128",
        "u16",
        "u32",
        "u64",
        "u8",
        "usize",
    ];
    NAMES.contains(&root)
}

struct Reader<'a> {
    t: &'a [Token<'a>],
    partner: &'a [usize],
    facts: Facts,
    /// How many `mod name { ... }` blocks the reader is inside of.
    inline: usize,
}

impl<'a> Reader<'a> {
    fn word(&self, index: usize) -> &'a str {
        self.t.get(index).filter(|token| token.is_word()).map_or("", |token| token.text)
    }

    fn is_word(&self, index: usize) -> bool {
        self.t.get(index).is_some_and(Token::is_word)
    }

    fn sign(&self, index: usize, sign: &str) -> bool {
        self.t.get(index).is_some_and(|token| token.is(sign))
    }

    fn opens(&self, index: usize) -> bool {
        self.sign(index, "(") || self.sign(index, "[") || self.sign(index, "{")
    }

    fn close(&self, index: usize) -> usize {
        close_of(self.t, self.partner, index)
    }

    fn uses(&self, from: usize, to: usize) -> Vec<Use> {
        uses(self.t, self.partner, from, to, &SPELLING)
    }

    fn line(&self, index: usize) -> u32 {
        line_at(self.t, index)
    }

    /// Where the `<` at `open` is closed. `->` is one token, so the arrow of
    /// `Fn(u8) -> u8` does not count as a `>`.
    fn angle_close(&self, open: usize, to: usize) -> usize {
        let mut depth = 0;
        let mut at = open;
        while at < to {
            if self.sign(at, "<") {
                depth += 1;
            } else if self.sign(at, ">") {
                depth -= 1;
                if depth == 0 {
                    return at;
                }
            } else if self.opens(at) {
                at = self.close(at);
            }
            at += 1;
        }
        open
    }

    /// The first of `signs` from `from` on that is not inside brackets.
    fn first_of(&self, from: usize, to: usize, signs: &[&str]) -> Option<usize> {
        let mut at = from;
        while at < to {
            if signs.iter().any(|sign| self.sign(at, sign)) {
                return Some(at);
            }
            at = if self.opens(at) { self.close(at) + 1 } else { at + 1 };
        }
        None
    }

    /// Keeps a definition, or only what it uses when it is test code.
    fn keep(&mut self, name: String, kind: &'static str, line: u32, last: usize, used: Vec<Use>, in_tests: bool) {
        if in_tests || name.is_empty() || name == "_" {
            self.facts.loose.extend(used);
        } else {
            self.facts.defs.push(Def { name, kind, line, end: self.line(last).max(line), uses: used });
        }
    }

    fn function(&mut self, at: usize, to: usize, owner: Option<&str>, in_tests: bool, is_test: bool) -> usize {
        let line = self.line(at);
        let name = self.word(at + 1);
        let mut params = at + 2;
        if self.sign(params, "<") {
            params = self.angle_close(params, to) + 1;
        }
        if !self.sign(params, "(") {
            return at + 1;
        }
        // The body is the first `{` after the parameters. A `;` first means there
        // is none: a trait's method without a default, or a foreign function.
        let after = self.close(params) + 1;
        let Some(body) = self.first_of(after, to, &["{", ";"]).filter(|index| self.sign(*index, "{")) else {
            return self.first_of(after, to, &[";"]).map_or(to, |end| end + 1);
        };
        let close = self.close(body);
        let used = self.uses(params, close);
        if is_test {
            self.facts.tests += 1;
        }
        let name = match owner {
            Some(owner) => format!("{owner}::{name}"),
            None => name.to_string(),
        };
        self.keep(name, "function", line, close, used, in_tests || is_test);
        close + 1
    }

    /// `struct`, `enum`, `union` and `trait`: a name, then braces or a `;`.
    fn shaped(&mut self, at: usize, to: usize, in_tests: bool) -> usize {
        let kind = match self.word(at) {
            "enum" => "enum",
            "trait" => "trait",
            _ => "struct",
        };
        let end = match self.first_of(at + 2, to, &["{", ";"]) {
            Some(index) if self.sign(index, "{") => self.close(index),
            Some(index) => index,
            None => to.saturating_sub(1),
        };
        let used = self.uses(at + 2, end);
        self.keep(self.word(at + 1).to_string(), kind, self.line(at), end, used, in_tests);
        end + 1
    }

    /// `const NAME: T = ...;`, `static NAME: T = ...;` and `type Name = ...;`.
    fn one_line(&mut self, at: usize, to: usize, owner: Option<&str>, in_tests: bool) -> usize {
        let kind = match self.word(at) {
            "const" => "const",
            "static" => "static",
            _ => "type",
        };
        let name_at = if self.word(at + 1) == "mut" { at + 2 } else { at + 1 };
        let end = self.first_of(name_at, to, &[";"]).unwrap_or(to.saturating_sub(1));
        let used = self.uses(name_at + 1, end);
        // Inside an `impl` these belong to the type; the map lists the type's functions only.
        self.keep(self.word(name_at).to_string(), kind, self.line(at), end, used, in_tests || owner.is_some());
        end + 1
    }

    /// `impl<T> Trait for Type<T> { ... }`: its functions are named after `Type`.
    fn implementation(&mut self, at: usize, to: usize, in_tests: bool) -> usize {
        let mut index = at + 1;
        if self.sign(index, "<") {
            index = self.angle_close(index, to) + 1;
        }
        let mut owner = "";
        let mut body = None;
        while index < to {
            if self.sign(index, "{") {
                body = Some(index);
                break;
            }
            if self.sign(index, ";") || self.word(index) == "where" {
                body = self.first_of(index, to, &["{"]);
                break;
            }
            if self.sign(index, "<") {
                index = self.angle_close(index, to) + 1;
                continue;
            }
            if self.opens(index) {
                index = self.close(index) + 1;
                continue;
            }
            match self.word(index) {
                // What stands before `for` is the trait; the type comes after it.
                "for" => owner = "",
                "" | "dyn" | "unsafe" | "const" | "mut" | "impl" => {}
                // The last name of a path is the type: `fmt::Display`, `crate::tasks::Manager`.
                word => owner = word,
            }
            index += 1;
        }
        let Some(body) = body else { return to };
        let close = self.close(body);
        self.items(body + 1, close, Some(owner), in_tests);
        close + 1
    }

    fn module(&mut self, at: usize, to: usize, in_tests: bool) -> usize {
        let name = self.word(at + 1);
        if self.sign(at + 2, ";") {
            // `mod name;` says the module lives in a file of its own.
            self.facts.imports.push(Import {
                spec: format!("mod {name}"),
                line: self.line(at),
                bindings: vec![Binding { local: name.to_string(), item: None }],
            });
            return at + 3;
        }
        if self.sign(at + 2, "{") {
            let close = self.close(at + 2).min(to);
            self.inline += 1;
            self.items(at + 3, close, None, in_tests || matches!(name, "tests" | "test"));
            self.inline -= 1;
            return close + 1;
        }
        at + 1
    }

    /// One branch of a `use`: `a::b::{c, d as e, self}`, `a::*`.
    fn use_tree(&mut self, from: usize, to: usize, path: &mut Vec<&'a str>, line: u32) {
        let depth = path.len();
        let mut at = from;
        if self.sign(at, "::") {
            at += 1;
        }
        while at < to {
            if self.sign(at, "{") {
                let close = self.close(at).min(to);
                let mut item = at + 1;
                while item < close {
                    let stop = self.first_of(item, close, &[","]).unwrap_or(close);
                    self.use_tree(item, stop, path, line);
                    item = stop + 1;
                }
                break;
            }
            if self.sign(at, "*") {
                // Everything of a module: the module is needed, but no name is known.
                self.facts.imports.push(Import { spec: path.join("::"), line, bindings: vec![] });
                break;
            }
            if !self.is_word(at) {
                break;
            }
            let word = self.t[at].text;
            let alias = if self.word(at + 1) == "as" { Some(self.word(at + 2)) } else { None };
            if word == "self" && !path.is_empty() && !self.sign(at + 1, "::") {
                // `{self}` brings the module itself in, under its own name.
                let local = alias.unwrap_or(path[path.len() - 1]);
                let bindings = vec![Binding { local: local.to_string(), item: None }];
                self.facts.imports.push(Import { spec: path.join("::"), line, bindings });
                break;
            }
            path.push(word);
            if self.sign(at + 1, "::") {
                at += 2;
                continue;
            }
            let local = alias.unwrap_or(word);
            // `as _` brings a trait's methods in without a name.
            let bindings = if local == "_" { vec![] } else { vec![Binding { local: local.to_string(), item: None }] };
            self.facts.imports.push(Import { spec: path.join("::"), line, bindings });
            break;
        }
        path.truncate(depth);
    }

    /// Inside `mod name { ... }` a path is counted from that module, not from
    /// the file: there `super` is the file itself. Rewrites the imports added
    /// since `before` so they read as if written at the top of the file.
    fn out_of_inline_modules(&mut self, before: usize) {
        if self.inline == 0 {
            return;
        }
        let depth = self.inline;
        let mut kept = self.facts.imports.split_off(before);
        kept.retain_mut(|import| {
            let mut parts: Vec<&str> = import.spec.split("::").collect();
            let supers = parts.iter().take_while(|part| **part == "super").count();
            if parts.first() == Some(&"self") || supers < depth && supers > 0 {
                // It names something inside this file's own inline modules.
                return false;
            }
            if supers >= depth {
                parts.drain(..depth);
                parts.insert(0, "self");
                import.spec = parts.join("::");
            }
            // `use super::*` in a test module is the file looking at itself.
            import.spec != "self"
        });
        self.facts.imports.append(&mut kept);
    }

    /// The items between two tokens: of a file, of a `mod { }` or of an `impl { }`.
    fn items(&mut self, from: usize, to: usize, owner: Option<&str>, in_tests: bool) {
        let mut at = from;
        // What the attributes in front of the next item said.
        let mut is_test = false;
        let mut only_in_tests = false;
        while at < to {
            if self.sign(at, "#") {
                let open = if self.sign(at + 1, "!") { at + 2 } else { at + 1 };
                if self.sign(open, "[") {
                    let close = self.close(open);
                    // `#[test]`, `#[tokio::test]`, `#[rstest]`, `#[test_case(1)]`: the name before any `(`.
                    let name_end =
                        (open + 1..close).find(|i| self.sign(*i, "(") || self.sign(*i, "=")).unwrap_or(close);
                    let name = (open + 1..name_end).rev().map(|i| self.word(i)).find(|word| !word.is_empty());
                    is_test |= matches!(name, Some("test" | "rstest" | "test_case" | "bench"));
                    let mentions_test = (open + 1..close).any(|i| self.word(i) == "test");
                    only_in_tests |= self.word(open + 1) == "cfg" && mentions_test;
                    at = close + 1;
                    continue;
                }
            }
            if self.word(at) == "pub" {
                at += 1;
                if self.sign(at, "(") {
                    at = self.close(at) + 1;
                }
            }
            loop {
                match self.word(at) {
                    "async" | "unsafe" | "default" => at += 1,
                    "const" if matches!(self.word(at + 1), "fn" | "unsafe" | "async" | "extern") => at += 1,
                    "extern" if self.word(at + 1) != "crate" => {
                        at += 1;
                        // `extern "C" fn`: the string names the calling convention.
                        if self.t.get(at).is_some_and(|token| token.kind == Kind::Text) {
                            at += 1;
                        }
                    }
                    _ => break,
                }
            }
            let tests = in_tests || only_in_tests;
            at = match self.word(at) {
                "fn" => self.function(at, to, owner, tests, is_test),
                "struct" | "union" | "enum" | "trait" if self.is_word(at + 1) => self.shaped(at, to, tests),
                "const" | "static" | "type" => self.one_line(at, to, owner, tests),
                "impl" => self.implementation(at, to, tests),
                "mod" => self.module(at, to, tests),
                "use" => {
                    let end = self.first_of(at + 1, to, &[";"]).unwrap_or(to);
                    let line = self.line(at);
                    let before = self.facts.imports.len();
                    self.use_tree(at + 1, end, &mut Vec::new(), line);
                    self.out_of_inline_modules(before);
                    end + 1
                }
                "extern" => {
                    // `extern crate name;`
                    let name = self.word(at + 2);
                    let bindings = vec![Binding { local: name.to_string(), item: None }];
                    self.facts.imports.push(Import { spec: name.to_string(), line: self.line(at), bindings });
                    self.first_of(at, to, &[";"]).map_or(to, |end| end + 1)
                }
                // Not an item this scanner knows, such as `macro_rules!`: step over it whole.
                _ if self.opens(at) => self.close(at) + 1,
                _ => at + 1,
            };
            is_test = false;
            only_in_tests = false;
        }
    }

    /// A path written in full inside code, `crate::files::list(...)`, needs
    /// its file as much as a `use` does.
    fn paths_in_code(&mut self) {
        let mut specs: Vec<(String, u32)> = Vec::new();
        let all = self.facts.defs.iter().flat_map(|def| &def.uses).chain(&self.facts.loose);
        for used in all {
            if used.chain.starts_with("crate::") {
                // Only the path: a method called on what it names is not part of it.
                let path = used.chain.split('.').next().unwrap_or_default();
                specs.push((path.to_string(), used.line));
            }
        }
        for (spec, line) in specs {
            if !self.facts.imports.iter().any(|import| import.spec == spec) {
                self.facts.imports.push(Import { spec, line, bindings: vec![] });
            }
        }
    }
}

pub fn scan(text: &str) -> Facts {
    let tokens = lex(text, Syntax::Rust);
    let partner = pairs(&tokens);
    let mut reader = Reader { t: &tokens, partner: &partner, facts: Facts::default(), inline: 0 };
    reader.items(0, tokens.len(), None, false);
    reader.paths_in_code();
    reader.facts
}

#[cfg(test)]
mod tests {
    use super::super::scan::testing::{calls, defs, imports, lines, names};
    use super::*;

    const CORE: &str = r####"//! Docs that hold code: fn fake_in_docs() {}
use crate::gitops::{self, BranchState, Pick as Chosen};
use std::collections::HashMap;
use super::marks::*;
use serde::{Deserialize, Serialize};
use anyhow::Context as _;
mod parser;
pub mod util { pub fn inner() {} }

/// fn in_a_doc_comment() {}
#[derive(Debug, Clone)]
pub struct Manager<T: Clone> { store: Store, items: Vec<T> }

pub enum State { Idle, Running }
pub trait Runner { fn run(&self); fn stop(&self) { halt(); } }
pub const LIMIT: usize = 5;
static mut COUNTER: u32 = 0;
type Shared = Arc<Mutex<State>>;
struct Unit;
struct Pair(u8, u8);

impl<T: Clone> Manager<T> {
    const INNER: u8 = 1;
    pub fn new(store: Store) -> Self { Self { store, items: Vec::new() } }
    pub async fn git_state(&self, task_id: &str) -> Result<BranchState> {
        let (root, base) = self.git_place(task_id)?;
        let text = r#"fn inside_a_raw_string() { "#;
        let brace = '{';
        if check(&root) != 0 { log!("state {}", text); }
        gitops::state(&root, base.as_deref())
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.label()) }
}

pub fn free<F: Fn(u8) -> u8>(apply: F) -> u8 where F: Send { crate::files::list(apply(1)) }

#[cfg(test)]
mod tests {
    use super::*;
    fn helper() -> u8 { free(|x| x) }
    #[test]
    fn it_works() { assert_eq!(helper(), 1); }
    #[tokio::test]
    async fn it_waits() { Manager::new(store()).git_state("a").await.unwrap(); }
}
"####;

    #[test]
    fn definitions_with_their_kinds_and_lines() {
        let facts = scan(CORE);
        assert_eq!(
            defs(&facts),
            vec![
                "function inner",
                "struct Manager",
                "enum State",
                "trait Runner",
                "const LIMIT",
                "static COUNTER",
                "type Shared",
                "struct Unit",
                "struct Pair",
                "function Manager::new",
                "function Manager::git_state",
                "function State::fmt",
                "function free",
            ]
        );
        assert_eq!(lines(&facts, "Manager"), (12, 12));
        assert_eq!(lines(&facts, "Manager::git_state"), (25, 31));
        assert_eq!(lines(&facts, "free"), (38, 38));
    }

    #[test]
    fn use_trees_modules_and_paths_written_in_code() {
        let facts = scan(CORE);
        assert_eq!(
            imports(&facts),
            vec![
                "crate::gitops: gitops<-*",
                "crate::gitops::BranchState: BranchState<-*",
                "crate::gitops::Pick: Chosen<-*",
                "std::collections::HashMap: HashMap<-*",
                "super::marks",
                "serde::Deserialize: Deserialize<-*",
                "serde::Serialize: Serialize<-*",
                "anyhow::Context",
                "mod parser: parser<-*",
                "crate::files::list",
            ]
        );
    }

    #[test]
    fn calls_inside_functions_and_not_inside_strings_or_macros() {
        let facts = scan(CORE);
        assert_eq!(
            calls(&facts, "Manager::git_state"),
            vec!["self.git_place", "check", "gitops::state", "base.as_deref"]
        );
        assert_eq!(calls(&facts, "Manager::new"), vec!["Vec::new"]);
        // A macro is not a call; what is inside it is still read.
        assert_eq!(calls(&facts, "State::fmt"), vec!["self.label"]);
        assert_eq!(calls(&facts, "free"), vec!["apply", "crate::files::list"]);
        assert!(names(&facts, "Manager").contains(&"Store".to_string()));
        assert_eq!(calls(&facts, "Runner"), vec!["halt"]);
    }

    #[test]
    fn tests_are_counted_and_not_listed() {
        let facts = scan(CORE);
        assert_eq!(facts.tests, 2);
        assert!(facts.def("helper").is_none() && facts.def("it_works").is_none());
        let loose: Vec<_> = facts.loose.iter().map(|u| u.chain.as_str()).collect();
        assert!(
            loose.contains(&"free") && loose.contains(&"Manager::new") && loose.contains(&".git_state"),
            "{loose:?}"
        );

        // A file of integration tests: its helpers are definitions, its tests are not.
        let facts =
            scan("mod common;\nfn helper() {}\n#[tokio::test]\nasync fn works() { helper(); }\n#[test]\nfn too() {}\n");
        assert_eq!(defs(&facts), vec!["function helper"]);
        assert_eq!(facts.tests, 2);
        assert_eq!(imports(&facts), vec!["mod common: common<-*"]);
    }

    #[test]
    fn odd_spellings() {
        let facts = scan(
            r#"pub(crate) const fn fast() -> u8 { 1 }
pub unsafe extern "C" fn raw(p: *const u8) {}
impl<'a, T> From<&'a T> for Wrapper<T> where T: Clone { fn from(v: &'a T) -> Self { wrap(v.clone()) } }
macro_rules! shout { ($x:expr) => { fn not_a_function() {} }; }
extern crate alloc;
/* fn commented() {} /* nested */ fn still_commented() {} */
fn generic_bound<T>() where T: Fn() -> u8 { if a < b { c() } }
"#,
        );
        assert_eq!(
            defs(&facts),
            vec!["function fast", "function raw", "function Wrapper::from", "function generic_bound"]
        );
        assert_eq!(calls(&facts, "Wrapper::from"), vec!["wrap", "v.clone"]);
        // `Fn()` in a bound is spelled like a call; the map drops it later as a name of the language.
        assert_eq!(calls(&facts, "generic_bound"), vec!["Fn", "c"]);
        assert_eq!(imports(&facts), vec!["alloc: alloc<-*"]);
    }

    #[test]
    fn names_a_function_gives_to_its_own_values_are_not_calls_it_makes() {
        let facts = scan(
            r#"fn total(orders: &[Order]) -> u32 {
    let mut sum = 0;
    let add = |n: u32| sum += n;
    let (first, rest) = split(orders);
    if let Some(order) = first { add(order.total()); }
    let Config { limit, .. } = load();
    for order in rest { add(price(order)); }
    limit(sum)
}
"#,
        );
        assert_eq!(calls(&facts, "total"), vec!["split", "order.total", "load", "price"]);
    }
}
