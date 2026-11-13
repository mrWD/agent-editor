//! Python.
//!
//! Python has no braces: a block is the lines indented under its first line.
//! Read: `import` and `from ... import`, functions, classes with their
//! methods, and names given a value at the top of the file. Functions named
//! `test_...` are counted as tests, not listed.

use super::lex::{ALONE, Syntax, Token, lex, pairs};
use super::scan::{Binding, Def, Facts, Import, Spelling, Use, line_at, uses};

const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "cls", "continue", "def",
    "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
    "nonlocal", "not", "or", "pass", "raise", "return", "self", "try", "while", "with", "yield",
];

pub const SPELLING: Spelling = Spelling {
    keywords: KEYWORDS,
    roots: &["self", "cls"],
    defines: &["def", "class"],
    variables: &["for", "as"],
    naming_sign: Some("="),
    colons: false,
    bang_macros: false,
    short_methods: false,
};

/// Names Python has without an import. A call to one of them tells nothing
/// about the project, so the map leaves it out.
pub fn builtin(root: &str) -> bool {
    const NAMES: &[&str] = &[
        "abs",
        "all",
        "any",
        "bool",
        "bytearray",
        "bytes",
        "callable",
        "chr",
        "classmethod",
        "dict",
        "dir",
        "divmod",
        "enumerate",
        "filter",
        "float",
        "format",
        "frozenset",
        "getattr",
        "hasattr",
        "hash",
        "id",
        "input",
        "int",
        "isinstance",
        "issubclass",
        "iter",
        "len",
        "list",
        "map",
        "max",
        "min",
        "next",
        "object",
        "open",
        "ord",
        "pow",
        "print",
        "property",
        "range",
        "repr",
        "reversed",
        "round",
        "set",
        "setattr",
        "sorted",
        "staticmethod",
        "str",
        "sum",
        "super",
        "tuple",
        "type",
        "vars",
        "zip",
    ];
    NAMES.contains(&root) || root.ends_with("Error") || root.ends_with("Exception")
}

/// A statement, which may run over several lines while a bracket is open.
struct Line {
    first: usize,
    /// One past its last token.
    end: usize,
    indent: u32,
}

struct Reader<'a> {
    t: &'a [Token<'a>],
    partner: &'a [usize],
    lines: Vec<Line>,
    facts: Facts,
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

    fn uses(&self, from: usize, to: usize) -> Vec<Use> {
        uses(self.t, self.partner, from, to, &SPELLING)
    }

    /// The first line after `at` that is not indented deeper than it: the end of its block.
    fn block_end(&self, at: usize, to: usize) -> usize {
        let indent = self.lines[at].indent;
        (at + 1..to).find(|index| self.lines[*index].indent <= indent).unwrap_or(to)
    }

    /// `a.b.c` starting at `at`. Returns the path and where it ended.
    fn dotted(&self, at: usize, end: usize) -> (String, usize) {
        let mut path = String::new();
        let mut at = at;
        while at < end && self.is_word(at) && !matches!(self.word(at), "import" | "as") {
            path.push_str(self.t[at].text);
            at += 1;
            if self.sign(at, ".") && self.is_word(at + 1) {
                path.push('.');
                at += 1;
            } else {
                break;
            }
        }
        (path, at)
    }

    fn import(&mut self, first: usize, end: usize) {
        let line = self.t[first].line;
        if self.word(first) == "import" {
            // `import a.b.c as d, e`
            let mut at = first + 1;
            while at < end {
                let (path, after) = self.dotted(at, end);
                let local = if self.word(after) == "as" { self.word(after + 1).to_string() } else { path.clone() };
                if !path.is_empty() {
                    self.facts.imports.push(Import { spec: path, line, bindings: vec![Binding { local, item: None }] });
                }
                at = (after..end).find(|index| self.sign(*index, ",")).map_or(end, |comma| comma + 1);
            }
            return;
        }
        // `from ..pkg.mod import a as b, (c, d)`. The dots count folders upwards.
        let mut at = first + 1;
        let mut spec = String::new();
        while self.sign(at, ".") || self.sign(at, "...") {
            spec.push_str(self.t[at].text);
            at += 1;
        }
        let (path, after) = self.dotted(at, end);
        spec.push_str(&path);
        if self.word(after) != "import" || spec.is_empty() {
            return;
        }
        let mut at = after + 1;
        let mut bindings = Vec::new();
        while at < end {
            if self.is_word(at) {
                let item = self.t[at].text;
                let local = if self.word(at + 1) == "as" { self.word(at + 2) } else { item };
                bindings.push(Binding { local: local.to_string(), item: Some(item.to_string()) });
            }
            // On to the name after the next comma; a `(` around the names is skipped by this too.
            at = (at + usize::from(self.is_word(at))..end)
                .find(|index| self.sign(*index, ",") || self.sign(*index, "("))
                .map_or(end, |stop| stop + 1);
        }
        // `from x import *` brings everything in: the file is needed, no name is known.
        self.facts.imports.push(Import { spec, line, bindings });
    }

    /// A `def` at line `at`. `class` is the class it is a method of.
    fn function(&mut self, at: usize, body_end: usize, class: Option<&str>) {
        let first = self.lines[at].first;
        let def_at = if self.word(first) == "async" { first + 1 } else { first };
        let name = self.word(def_at + 1);
        let last = self.lines[body_end - 1].end;
        let used = self.uses(def_at + 2, last);
        // pytest and unittest both find tests by these names.
        let named_as_test = name.starts_with("test_") || name == "test";
        let in_test_class = class.is_some_and(|class| class.starts_with("Test"));
        if named_as_test && (class.is_none() || in_test_class) {
            self.facts.tests += 1;
            self.facts.loose.extend(used);
            return;
        }
        let name = match class {
            Some(class) => format!("{class}.{name}"),
            None => name.to_string(),
        };
        let line = self.t[first].line;
        self.facts.defs.push(Def {
            name,
            kind: "function",
            line,
            end: line_at(self.t, last.saturating_sub(1)).max(line),
            uses: used,
        });
    }

    fn class(&mut self, at: usize, body_end: usize) {
        let first = self.lines[at].first;
        let name = self.word(first + 1).to_string();
        let line = self.t[first].line;
        let last = self.lines[body_end - 1].end;
        // What the class itself uses: what it extends, and what its own lines name.
        let mut own = self.uses(first + 2, self.lines[at].end);
        let index = self.facts.defs.len();
        self.facts.defs.push(Def {
            name: name.clone(),
            kind: "class",
            line,
            end: line_at(self.t, last.saturating_sub(1)).max(line),
            uses: vec![],
        });
        let member_indent = self.lines.get(at + 1).map_or(0, |line| line.indent);
        let mut member = at + 1;
        while member < body_end {
            let end = self.block_end(member, body_end);
            let start = self.lines[member].first;
            let def_at = if self.word(start) == "async" { start + 1 } else { start };
            if self.lines[member].indent == member_indent && self.word(def_at) == "def" {
                self.function(member, end, Some(&name));
            } else {
                own.extend(self.uses(start, self.lines[end - 1].end));
            }
            member = end;
        }
        self.facts.defs[index].uses = own;
    }

    /// `NAME = value` or `name: Type = value` at the left edge of the file.
    fn assignment(&mut self, at: usize) -> bool {
        let Line { first, end, indent } = self.lines[at];
        let name = self.word(first);
        if indent != 0 || name.is_empty() || KEYWORDS.contains(&name) {
            return false;
        }
        let plain = self.sign(first + 1, "=") && !self.sign(first + 2, "=");
        let typed = self.sign(first + 1, ":") && (first + 2..end).any(|index| self.sign(index, "="));
        if !(plain || typed) || self.facts.def(name).is_some() {
            return false;
        }
        // By convention a name in capitals is never given another value.
        let shouts = name.chars().any(|c| c.is_ascii_uppercase()) && !name.chars().any(|c| c.is_ascii_lowercase());
        let line = self.t[first].line;
        let used = self.uses(first + 1, end);
        self.facts.defs.push(Def {
            name: name.to_string(),
            kind: if shouts { "const" } else { "variable" },
            line,
            end: line_at(self.t, end.saturating_sub(1)).max(line),
            uses: used,
        });
        true
    }

    fn statements(&mut self) {
        let count = self.lines.len();
        let mut at = 0;
        while at < count {
            let Line { first, end, .. } = self.lines[at];
            let body_end = self.block_end(at, count);
            let def_at = if self.word(first) == "async" { first + 1 } else { first };
            match self.word(def_at) {
                "def" => {
                    self.function(at, body_end, None);
                    at = body_end;
                }
                "class" => {
                    self.class(at, body_end);
                    at = body_end;
                }
                "import" | "from" => {
                    self.import(first, end);
                    at += 1;
                }
                _ => {
                    // Anything else runs when the file loads. Lines under an `if` or a `try`
                    // are read one by one, so an import or a function inside is still found.
                    if !self.assignment(at) {
                        let used = self.uses(first, end);
                        self.facts.loose.extend(used);
                    }
                    at += 1;
                }
            }
        }
    }
}

/// Cuts the tokens into statements and notes how far each is indented.
fn logical_lines(text: &str, tokens: &[Token], partner: &[usize]) -> Vec<Line> {
    // A tab moves to the next multiple of eight, as Python itself counts it.
    let indents: Vec<u32> = text
        .lines()
        .map(|line| {
            let mut width = 0;
            for byte in line.bytes() {
                match byte {
                    b' ' => width += 1,
                    b'\t' => width = (width / 8 + 1) * 8,
                    _ => break,
                }
            }
            width
        })
        .collect();
    let mut lines = Vec::new();
    let mut at = 0;
    while at < tokens.len() {
        let first = at;
        let mut line = tokens[at].line;
        while at < tokens.len() && tokens[at].line == line {
            // An open bracket carries the statement on to the line where it closes.
            let close = partner[at];
            let opens = tokens[at].is("(") || tokens[at].is("[") || tokens[at].is("{");
            if opens && close != ALONE && close > at {
                line = tokens[close].line;
                at = close + 1;
            } else {
                at += 1;
            }
        }
        let indent = indents.get(tokens[first].line as usize - 1).copied().unwrap_or(0);
        lines.push(Line { first, end: at, indent });
    }
    lines
}

pub fn scan(text: &str) -> Facts {
    let tokens = lex(text, Syntax::Python);
    let partner = pairs(&tokens);
    let lines = logical_lines(text, &tokens, &partner);
    let mut reader = Reader { t: &tokens, partner: &partner, lines, facts: Facts::default() };
    reader.statements();
    reader.facts
}

#[cfg(test)]
mod tests {
    use super::super::scan::testing::{calls, defs, imports, lines, names};
    use super::*;

    const APP: &str = r#""""Module docs.

def fake_in_docstring(): pass
"""
import os, sys as system
import app.services.users as users
from .config import limits, DEFAULT as DEF
from ..core import (
    Base,
    helper,
)
from . import sibling
from typing import *

LIMIT = 5
router = APIRouter()
count: int = 0

# def commented_out(): pass
def login(request, user=None):
    text = "def inside_a_string(): pass"
    found = users.find_user(request.name)
    if not found:
        return respond(401)
    return check_password(found, request.password)

async def refresh(token):
    return await helper(token).run()

class Session(Base):
    timeout = limits.session

    def __init__(self, id):
        self.id = id
        self.touch()

    @property
    def age(self):
        return now() - self.started

    def touch(self):
        self.started = now()

if DEBUG:
    import debug_tools
    def only_in_debug():
        return debug_tools.dump()

def test_login():
    assert login(None)

class TestSession:
    def test_open(self):
        Session(1)
    def helper(self):
        pass
"#;

    #[test]
    fn imports_in_every_spelling() {
        let facts = scan(APP);
        assert_eq!(
            imports(&facts),
            vec![
                "os: os<-*",
                "sys: system<-*",
                "app.services.users: users<-*",
                ".config: limits, DEF<-DEFAULT",
                "..core: Base, helper",
                ".: sibling",
                "typing",
                "debug_tools: debug_tools<-*",
            ]
        );
        assert_eq!(facts.imports[4].line, 8);
    }

    #[test]
    fn definitions_with_their_kinds_and_lines() {
        let facts = scan(APP);
        assert_eq!(
            defs(&facts),
            vec![
                "const LIMIT",
                "variable router",
                "variable count",
                "function login",
                "function refresh",
                "class Session",
                "function Session.__init__",
                "function Session.age",
                "function Session.touch",
                "function only_in_debug",
                "class TestSession",
                "function TestSession.helper",
            ]
        );
        assert_eq!(lines(&facts, "login"), (20, 25));
        assert_eq!(lines(&facts, "Session"), (30, 42));
        assert_eq!(lines(&facts, "Session.age"), (38, 39));
        assert_eq!(facts.tests, 2);
    }

    #[test]
    fn calls_inside_functions_and_not_inside_strings_or_comments() {
        let facts = scan(APP);
        assert_eq!(calls(&facts, "login"), vec!["users.find_user", "respond", "check_password"]);
        assert_eq!(calls(&facts, "refresh"), vec!["helper", ".run"]);
        assert_eq!(calls(&facts, "Session.__init__"), vec!["self.touch"]);
        assert_eq!(calls(&facts, "Session.touch"), vec!["now"]);
        let own = names(&facts, "Session");
        assert!(own.contains(&"Base".to_string()) && own.contains(&"limits.session".to_string()), "{own:?}");
        assert_eq!(calls(&facts, "router"), vec!["APIRouter"]);
        let loose: Vec<_> = facts.loose.iter().map(|u| u.chain.as_str()).collect();
        assert!(loose.contains(&"login") && loose.contains(&"Session"), "{loose:?}");
    }

    #[test]
    fn a_statement_runs_on_while_a_bracket_is_open_and_tabs_indent() {
        let facts = scan(
            "def a(\n    x,\n    y,\n):\n\treturn call(\n\t\tx,\n\t)\n\ndef b(): return 1\nVALUE = [\n    1,\n]\n",
        );
        assert_eq!(defs(&facts), vec!["function a", "function b", "const VALUE"]);
        assert_eq!(lines(&facts, "a"), (1, 7));
        assert_eq!(lines(&facts, "VALUE"), (10, 12));
        assert_eq!(calls(&facts, "a"), vec!["call"]);
    }

    #[test]
    fn names_a_function_gives_to_its_own_values_are_not_calls_it_makes() {
        let facts = scan(
            "def report(rows, out):\n    lines = [format_row(row) for row in rows]\n    total, count = summarize(rows)\n    def write(text):\n        out.write(text)\n    with open_log() as log:\n        log.info(total)\n    write(lines)\n    count()\n    return finish(total=total)\n",
        );
        assert_eq!(
            calls(&facts, "report"),
            vec!["format_row", "summarize", "out.write", "open_log", "log.info", "finish"]
        );
    }
}
