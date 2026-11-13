//! Go.
//!
//! Read: the package name, imports, functions, methods (named
//! `Type.Method`), types, constants and variables of the package. In a
//! `_test.go` file the functions the test runner finds by name are counted as
//! tests, not listed.

use super::lex::{Kind, Syntax, Token, lex, pairs};
use super::scan::{Binding, Def, Facts, Import, Spelling, Use, close_of, line_at, uses};

const KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "false",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "iota",
    "map",
    "nil",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "true",
    "type",
    "var",
];

pub const SPELLING: Spelling = Spelling {
    keywords: KEYWORDS,
    roots: &[],
    defines: &["func"],
    variables: &["var"],
    naming_sign: Some(":="),
    colons: false,
    bang_macros: false,
    short_methods: false,
};

/// Names Go has without an import. A call to one of them tells nothing about
/// the project, so the map leaves it out.
pub fn builtin(root: &str) -> bool {
    const NAMES: &[&str] = &[
        "any",
        "append",
        "bool",
        "byte",
        "cap",
        "clear",
        "close",
        "complex",
        "complex128",
        "complex64",
        "copy",
        "delete",
        "error",
        "float32",
        "float64",
        "imag",
        "int",
        "int16",
        "int32",
        "int64",
        "int8",
        "len",
        "make",
        "max",
        "min",
        "new",
        "panic",
        "print",
        "println",
        "real",
        "recover",
        "rune",
        "string",
        "uint",
        "uint16",
        "uint32",
        "uint64",
        "uint8",
        "uintptr",
    ];
    NAMES.contains(&root)
}

/// The name a package most likely has, from the path it is imported by:
/// the last part, unless that is a version such as `v2`.
pub fn package_name(spec: &str) -> &str {
    let mut parts = spec.rsplit('/');
    let last = parts.next().unwrap_or(spec);
    let is_version = last.len() > 1 && last.starts_with('v') && last[1..].bytes().all(|b| b.is_ascii_digit());
    if is_version { parts.next().unwrap_or(last) } else { last }
}

struct Reader<'a> {
    t: &'a [Token<'a>],
    partner: &'a [usize],
    facts: Facts,
    test_file: bool,
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

    /// Where the line that `from` is on ends, counting a bracket and all it holds as one piece.
    fn line_end(&self, from: usize, limit: usize) -> usize {
        let mut at = from;
        while at < limit {
            if at > from && self.t[at].line > self.t[at - 1].line {
                break;
            }
            at = if self.opens(at) { self.close(at) + 1 } else { at + 1 };
        }
        at.min(limit)
    }

    fn define(&mut self, name: &str, kind: &'static str, line: u32, last: usize, used: Vec<Use>) {
        if name.is_empty() || name == "_" {
            self.facts.loose.extend(used);
            return;
        }
        self.facts.defs.push(Def {
            name: name.to_string(),
            kind,
            line,
            end: line_at(self.t, last).max(line),
            uses: used,
        });
    }

    /// One import: `"path"`, `name "path"`, `. "path"` or `_ "path"`.
    fn import_spec(&mut self, at: usize) -> usize {
        let token = &self.t[at];
        let (alias, spec_at) = if token.kind == Kind::Text { (None, at) } else { (Some(token.text), at + 1) };
        let Some(spec) = self.t.get(spec_at).filter(|token| token.kind == Kind::Text) else {
            return at + 1;
        };
        let bindings = match alias {
            // `_` loads the package for what it does when loaded; `.` pours its names in unnamed.
            Some("_" | ".") => vec![],
            Some(name) => vec![Binding { local: name.to_string(), item: None }],
            None => vec![Binding { local: package_name(spec.text).to_string(), item: None }],
        };
        self.facts.imports.push(Import { spec: spec.text.to_string(), line: spec.line, bindings });
        spec_at + 1
    }

    /// `keyword spec` or `keyword ( spec spec ... )`: runs `one` for each spec.
    fn each(&mut self, at: usize, mut one: impl FnMut(&mut Self, usize, usize) -> usize) -> usize {
        // The keyword is the last thing in the file: there is nothing to read after it.
        if at + 1 >= self.t.len() {
            return self.t.len();
        }
        if self.sign(at + 1, "(") {
            let close = self.close(at + 1);
            let mut index = at + 2;
            while index < close {
                index = one(self, index, close).max(index + 1);
            }
            return close + 1;
        }
        one(self, at + 1, self.t.len()).max(at + 1)
    }

    fn function(&mut self, at: usize) -> usize {
        let line = self.t[at].line;
        let mut index = at + 1;
        let mut owner = None;
        if self.sign(index, "(") {
            // The receiver: `(s *Server)`, `(Server)`, `(p Pair[T])`. Its type is the last
            // name before any `[`.
            let close = self.close(index);
            let stop = (index + 1..close).find(|i| self.sign(*i, "[")).unwrap_or(close);
            owner = (index + 1..stop).rev().map(|i| self.word(i)).find(|word| !word.is_empty());
            index = close + 1;
        }
        let name = self.word(index);
        index += 1;
        if self.sign(index, "[") {
            index = self.close(index) + 1;
        }
        if name.is_empty() || !self.sign(index, "(") {
            return index;
        }
        // The body is the first `{` after the results that is not the brace of `interface{}` or `struct{}`.
        let mut search = self.close(index) + 1;
        let body = loop {
            if search >= self.t.len() {
                break None;
            }
            if self.sign(search, "{") {
                if matches!(self.word(search - 1), "interface" | "struct") {
                    search = self.close(search) + 1;
                    continue;
                }
                break Some(search);
            }
            // A new line with a new declaration: this function is only declared here, written elsewhere.
            let new_line = self.t[search].line > self.t[search - 1].line;
            if new_line && matches!(self.word(search), "func" | "type" | "var" | "const" | "import") {
                break None;
            }
            search = if self.opens(search) { self.close(search) + 1 } else { search + 1 };
        };
        let Some(body) = body else { return search };
        let close = self.close(body);
        let used = self.uses(index, close);
        let found_by_runner = ["Test", "Benchmark", "Example", "Fuzz"].iter().any(|prefix| name.starts_with(prefix));
        if self.test_file && owner.is_none() && found_by_runner {
            self.facts.tests += 1;
            self.facts.loose.extend(used);
        } else {
            let name = match owner {
                Some(owner) => format!("{owner}.{name}"),
                None => name.to_string(),
            };
            self.define(&name, "function", line, close, used);
        }
        close + 1
    }

    /// `Name struct { ... }`, `Name interface { ... }`, `Name = Other`, `Name Other`.
    fn type_spec(&mut self, at: usize, limit: usize) -> usize {
        let name = self.word(at);
        let line = self.t[at].line;
        let mut index = at + 1;
        if self.sign(index, "[") {
            index = self.close(index) + 1;
        }
        if self.sign(index, "=") {
            index += 1;
        }
        let kind = match self.word(index) {
            "struct" => "struct",
            "interface" => "interface",
            _ => "type",
        };
        let end = self.line_end(at, limit);
        let used = self.uses(index, end);
        self.define(name, kind, line, end.saturating_sub(1), used);
        end
    }

    /// `A = 1`, `A, B = 1, 2`, `A int`: every name before the value is defined.
    fn value_spec(&mut self, at: usize, limit: usize, kind: &'static str) -> usize {
        let line = self.t[at].line;
        let end = self.line_end(at, limit);
        let mut names = vec![self.word(at)];
        let mut index = at + 1;
        while self.sign(index, ",") && self.is_word(index + 1) && index + 1 < end {
            names.push(self.word(index + 1));
            index += 2;
        }
        let used = self.uses(index, end);
        for (position, name) in names.into_iter().enumerate() {
            // What the value uses is noted once, with the first name.
            let used = if position == 0 { used.clone() } else { vec![] };
            self.define(name, kind, line, end.saturating_sub(1), used);
        }
        end
    }

    fn file(&mut self) {
        let mut at = 0;
        while at < self.t.len() {
            at = match self.word(at) {
                "package" => {
                    self.facts.package = Some(self.word(at + 1).to_string());
                    at + 2
                }
                "import" => self.each(at, |reader, index, _| reader.import_spec(index)),
                "func" => self.function(at),
                "type" => self.each(at, |reader, index, limit| reader.type_spec(index, limit)),
                "const" => self.each(at, |reader, index, limit| reader.value_spec(index, limit, "const")),
                "var" => self.each(at, |reader, index, limit| reader.value_spec(index, limit, "variable")),
                _ if self.opens(at) => self.close(at) + 1,
                _ => at + 1,
            };
        }
    }
}

pub fn scan(path: &str, text: &str) -> Facts {
    let tokens = lex(text, Syntax::Go);
    let partner = pairs(&tokens);
    let test_file = path.ends_with("_test.go");
    let mut reader = Reader { t: &tokens, partner: &partner, facts: Facts::default(), test_file };
    reader.file();
    reader.facts
}

#[cfg(test)]
mod tests {
    use super::super::scan::testing::{calls, defs, imports, lines};
    use super::*;

    const SHOP: &str = r#"// Package shop: func fakeInComment() {}
package shop

import (
	"fmt"
	"net/http"
	limits "example.com/shop/internal/limits"
	_ "embed"
	"github.com/gorilla/mux"
	yaml "gopkg.in/yaml.v3"
	"example.com/shop/api/v2"
)

import "strings"

const Limit = 5
var (
	counter int
	names, labels = load(), map[string]struct{}{}
)

type Server struct {
	router *mux.Router
}

type Handler interface{ Serve() }
type ID = string
type (
	Count int
	Pair[T any] struct{ a, b T }
)

func New(r *mux.Router) *Server { return &Server{router: r} }

func (s *Server) Login(w http.ResponseWriter, req *http.Request) {
	text := `func insideRawString() {`
	if limits.Over(req.RemoteAddr) {
		http.Error(w, "too many {", 429)
		return
	}
	s.check(w, strings.TrimSpace(text))
}

func (s Server) check(w http.ResponseWriter, name string) interface{} { return fmt.Sprint(name) }

func Map[T any](items []T, f func(T) T) []T { return items }
"#;

    #[test]
    fn the_package_and_its_imports() {
        let facts = scan("shop/server.go", SHOP);
        assert_eq!(facts.package.as_deref(), Some("shop"));
        assert_eq!(
            imports(&facts),
            vec![
                "fmt: fmt<-*",
                "net/http: http<-*",
                "example.com/shop/internal/limits: limits<-*",
                "embed",
                "github.com/gorilla/mux: mux<-*",
                "gopkg.in/yaml.v3: yaml<-*",
                "example.com/shop/api/v2: api<-*",
                "strings: strings<-*",
            ]
        );
        assert_eq!(facts.imports[2].line, 7);
    }

    #[test]
    fn definitions_with_their_kinds_and_lines() {
        let facts = scan("shop/server.go", SHOP);
        assert_eq!(
            defs(&facts),
            vec![
                "const Limit",
                "variable counter",
                "variable names",
                "variable labels",
                "struct Server",
                "interface Handler",
                "type ID",
                "type Count",
                "struct Pair",
                "function New",
                "function Server.Login",
                "function Server.check",
                "function Map",
            ]
        );
        assert_eq!(lines(&facts, "Server"), (22, 24));
        assert_eq!(lines(&facts, "Server.Login"), (35, 42));
        assert_eq!(lines(&facts, "Server.check"), (44, 44));
    }

    #[test]
    fn calls_inside_functions_and_not_inside_strings_or_comments() {
        let facts = scan("shop/server.go", SHOP);
        assert_eq!(calls(&facts, "Server.Login"), vec!["limits.Over", "http.Error", "s.check", "strings.TrimSpace"]);
        // `interface{}` in the results is not the body.
        assert_eq!(calls(&facts, "Server.check"), vec!["fmt.Sprint"]);
        assert_eq!(calls(&facts, "names"), vec!["load"]);
        assert!(calls(&facts, "Map").is_empty());
    }

    #[test]
    fn tests_are_counted_in_test_files_only() {
        let text = "package shop\nfunc TestLogin(t *testing.T) { New(nil) }\nfunc BenchmarkLogin(b *testing.B) {}\nfunc helper() {}\n";
        let facts = scan("shop/server_test.go", text);
        assert_eq!((facts.tests, defs(&facts)), (2, vec!["function helper".to_string()]));
        assert!(facts.loose.iter().any(|u| u.chain == "New" && u.call));
        let facts = scan("shop/server.go", text);
        assert_eq!(facts.tests, 0);
        assert_eq!(defs(&facts).len(), 3);
    }

    #[test]
    fn names_a_function_gives_to_its_own_values_are_not_calls_it_makes() {
        let facts = scan(
            "shop/total.go",
            "package shop\n\nfunc Total(orders []Order) int {\n\tsum := 0\n\tadd := func(n int) { sum += n }\n\tfirst, rest := split(orders)\n\tvar limit = load()\n\tfor _, order := range rest {\n\t\tadd(price(order))\n\t}\n\tadd(first.Total())\n\treturn limit(sum)\n}\n",
        );
        assert_eq!(calls(&facts, "Total"), vec!["split", "load", "price", "first.Total"]);
    }
}
