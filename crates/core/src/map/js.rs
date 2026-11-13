//! JavaScript and TypeScript, with or without JSX.
//!
//! Read at the top level of a file: imports (`import`, `export ... from`,
//! `require`), functions, classes with their methods, constants and variables,
//! types, interfaces, enums, and routes written as `app.post('/path', ...)`.
//! Inside each of them: the names it uses and which of those it calls.

use super::lex::{ALONE, Kind, Syntax, Token, lex, pairs};
use super::scan::{Binding, Def, Facts, Import, Spelling, Use, close_of, line_at, uses};

const KEYWORDS: &[&str] = &[
    "abstract",
    "any",
    "as",
    "async",
    "await",
    "boolean",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "declare",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "get",
    "if",
    "implements",
    "import",
    "in",
    "infer",
    "instanceof",
    "interface",
    "is",
    "keyof",
    "let",
    "namespace",
    "never",
    "new",
    "null",
    "number",
    "object",
    "of",
    "private",
    "protected",
    "public",
    "readonly",
    "return",
    "satisfies",
    "set",
    "static",
    "string",
    "super",
    "switch",
    "symbol",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "undefined",
    "unknown",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

pub const SPELLING: Spelling = Spelling {
    keywords: KEYWORDS,
    roots: &["this", "super"],
    defines: &["function", "class", "interface", "enum", "namespace"],
    variables: &["const", "let", "var"],
    naming_sign: None,
    colons: false,
    bang_macros: false,
    short_methods: true,
};

/// Things the language or its usual hosts provide. A call to one of them tells
/// nothing about the project, so the map leaves it out.
pub fn builtin(root: &str) -> bool {
    const NAMES: &[&str] = &[
        "AbortController",
        "Array",
        "BigInt",
        "Boolean",
        "Buffer",
        "Date",
        "Error",
        "Intl",
        "JSON",
        "Map",
        "Math",
        "Number",
        "Object",
        "Promise",
        "Proxy",
        "RangeError",
        "Reflect",
        "RegExp",
        "Set",
        "String",
        "Symbol",
        "TextDecoder",
        "TextEncoder",
        "TypeError",
        "URL",
        "URLSearchParams",
        "WeakMap",
        "WeakSet",
        "alert",
        "atob",
        "btoa",
        "cancelAnimationFrame",
        "clearInterval",
        "clearTimeout",
        "console",
        "decodeURI",
        "decodeURIComponent",
        "document",
        "encodeURI",
        "encodeURIComponent",
        "fetch",
        "globalThis",
        "isFinite",
        "isNaN",
        "localStorage",
        "navigator",
        "parseFloat",
        "parseInt",
        "process",
        "queueMicrotask",
        "requestAnimationFrame",
        "require",
        "sessionStorage",
        "setImmediate",
        "setInterval",
        "setTimeout",
        "structuredClone",
        "window",
    ];
    NAMES.contains(&root)
}

/// The ways a router names the kind of request it answers.
const VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options", "all", "use"];

/// Words that cannot be the last word of a statement: a new line after them goes on.
const GOES_ON_AFTER: &[&str] = &[
    "return",
    "typeof",
    "in",
    "of",
    "instanceof",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "else",
    "extends",
    "as",
    "satisfies",
    "await",
    "yield",
    "export",
    "default",
    "import",
    "from",
    "const",
    "let",
    "var",
    "async",
    "function",
    "class",
    "type",
    "keyof",
    "is",
    "implements",
];

/// Words that cannot be the first word of a statement: they continue the line before.
const CONTINUES: &[&str] =
    &["in", "of", "instanceof", "as", "satisfies", "extends", "implements", "from", "else", "catch", "finally"];

struct Reader<'a> {
    t: &'a [Token<'a>],
    partner: &'a [usize],
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

    fn text(&self, index: usize) -> Option<&'a str> {
        self.t.get(index).filter(|token| token.kind == Kind::Text).map(|token| token.text)
    }

    fn close(&self, index: usize) -> usize {
        close_of(self.t, self.partner, index)
    }

    fn uses(&self, from: usize, to: usize) -> Vec<Use> {
        uses(self.t, self.partner, from, to, &SPELLING)
    }

    fn define(&mut self, name: &str, kind: &'static str, line: u32, last: usize, uses: Vec<Use>) {
        if name.is_empty() {
            self.facts.loose.extend(uses);
            return;
        }
        self.facts.defs.push(Def { name: name.to_string(), kind, line, end: line_at(self.t, last).max(line), uses });
    }

    /// Whether the token at `index` may be the last of a statement.
    fn can_end(&self, index: usize) -> bool {
        let token = &self.t[index];
        match token.kind {
            Kind::Word => !GOES_ON_AFTER.contains(&token.text),
            Kind::Number | Kind::Text => true,
            Kind::Sign => matches!(token.text, ")" | "]" | "}" | ">"),
        }
    }

    /// Whether the token at `index`, first on its line, starts a new statement.
    fn starts_statement(&self, index: usize) -> bool {
        let token = &self.t[index];
        match token.kind {
            Kind::Word => !CONTINUES.contains(&token.text),
            Kind::Sign => matches!(token.text, "@" | "#"),
            _ => false,
        }
    }

    /// Where the statement that starts at `from` ends: after its `;`, or before
    /// the first token on a new line that cannot belong to it (JavaScript lets
    /// the `;` be left out), or before a bracket that closes something outside.
    fn statement_end(&self, from: usize) -> usize {
        let mut at = from;
        while at < self.t.len() {
            let token = &self.t[at];
            if at > from && token.line > self.t[at - 1].line && self.can_end(at - 1) && self.starts_statement(at) {
                return at;
            }
            if token.is(";") {
                return at + 1;
            }
            if token.is("(") || token.is("[") || token.is("{") {
                at = match self.partner[at] {
                    ALONE => at + 1,
                    close => close + 1,
                };
                continue;
            }
            if token.is(")") || token.is("]") || token.is("}") {
                return at;
            }
            at += 1;
        }
        self.t.len()
    }

    /// The first `sign` between two tokens that is not inside brackets.
    fn outside_brackets(&self, from: usize, to: usize, sign: &str) -> Option<usize> {
        let mut at = from;
        while at < to {
            let token = &self.t[at];
            if token.is(sign) {
                return Some(at);
            }
            at = if token.is("(") || token.is("[") || token.is("{") { self.close(at) + 1 } else { at + 1 };
        }
        None
    }

    /// Where the body of a function or a class starts, looking from `from`.
    /// A return type may have braces of its own, `(): { ok: boolean } {`,
    /// and a declaration without a body has none at all.
    fn body_open(&self, from: usize) -> Option<usize> {
        let limit = (from + 400).min(self.t.len());
        let mut at = from;
        while at < limit {
            let token = &self.t[at];
            let previous = &self.t[at.saturating_sub(1)];
            if token.is("{") {
                let in_a_type = match previous.kind {
                    Kind::Sign => matches!(previous.text, ":" | "|" | "&" | "<" | "," | "=>" | "(" | "["),
                    Kind::Word => matches!(previous.text, "extends" | "keyof" | "typeof" | "is" | "readonly"),
                    _ => false,
                };
                if !in_a_type || at == from {
                    return Some(at);
                }
                at = self.close(at) + 1;
                continue;
            }
            if token.is(";") || token.is(")") || token.is("]") || token.is("}") || token.is("=") {
                return None;
            }
            if token.is("(") || token.is("[") {
                at = self.close(at) + 1;
                continue;
            }
            // A new line that starts something else: this declaration had no body.
            let goes_on = matches!(token.text, "extends" | "implements");
            if at > from && token.is_word() && !goes_on && token.line > previous.line && self.can_end(at - 1) {
                return None;
            }
            at += 1;
        }
        None
    }

    /// Whether the value that starts at `index` is a function: `function`, or an arrow.
    fn is_function_value(&self, index: usize) -> bool {
        let mut at = index;
        if self.word(at) == "async" {
            at += 1;
        }
        if self.word(at) == "function" {
            return true;
        }
        if self.sign(at, "<") {
            // A generic arrow: `<T,>(value: T) => value`.
            match (at..(at + 24).min(self.t.len())).find(|i| self.sign(*i, "(")) {
                Some(open) => at = open,
                None => return false,
            }
        }
        if self.sign(at, "(") {
            let after = self.close(at) + 1;
            if self.sign(after, "=>") {
                return true;
            }
            // `(a: T): R => ...`. A colon right after the parameters can only be a return type.
            if self.sign(after, ":") {
                let limit = (after + 40).min(self.t.len());
                return (after..limit).take_while(|i| !self.sign(*i, ";")).any(|i| self.sign(i, "=>"));
            }
            return false;
        }
        self.is_word(at) && self.sign(at + 1, "=>")
    }

    /// `require('x')` or `import('x')` at `index`: the `x`.
    fn required(&self, index: usize) -> Option<&'a str> {
        let at = if self.word(index) == "await" { index + 1 } else { index };
        let asks = matches!(self.word(at), "require" | "import");
        if asks && self.sign(at + 1, "(") && self.sign(at + 3, ")") { self.text(at + 2) } else { None }
    }

    fn import_of(&mut self, spec: &str, line: u32, bindings: Vec<Binding>) {
        self.facts.imports.push(Import { spec: spec.to_string(), line, bindings });
    }

    /// The names between the braces of `import { a, b as c }`.
    fn named(&self, from: usize, to: usize, out: &mut Vec<Binding>) {
        let mut at = from;
        while at < to {
            // `type X` inside the braces imports only a type. `type` alone, or `type as t`, is a name.
            if self.word(at) == "type" && at + 1 < to && self.is_word(at + 1) && self.word(at + 1) != "as" {
                at += 1;
            }
            let item =
                self.t.get(at).filter(|token| token.is_word() || token.kind == Kind::Text).map(|token| token.text);
            if let Some(item) = item {
                let mut local = item;
                if self.word(at + 1) == "as" && at + 2 < to {
                    local = self.t[at + 2].text;
                }
                out.push(Binding { local: local.to_string(), item: Some(item.to_string()) });
            }
            while at < to && !self.sign(at, ",") {
                at += 1;
            }
            at += 1;
        }
    }

    /// What may follow the address of an import: `with { type: 'json' }` and a `;`.
    fn after_import(&self, index: usize) -> usize {
        let mut at = index;
        if matches!(self.word(at), "with" | "assert") && self.sign(at + 1, "{") {
            at = self.close(at + 1) + 1;
        }
        if self.sign(at, ";") { at + 1 } else { at }
    }

    fn import(&mut self, start: usize) -> usize {
        let line = self.t[start].line;
        let mut at = start + 1;
        if let Some(spec) = self.text(at) {
            // `import './setup'` brings no name in; the file is still needed.
            self.import_of(spec, line, vec![]);
            return self.after_import(at + 1);
        }
        if self.word(at) == "type" && self.word(at + 1) != "from" && !self.sign(at + 1, ",") && !self.sign(at + 1, "=")
        {
            at += 1;
        }
        let mut bindings = Vec::new();
        if self.is_word(at) && self.word(at) != "from" {
            let local = self.t[at].text.to_string();
            if self.sign(at + 1, "=") {
                // TypeScript's `import fs = require('fs')`.
                if let Some(spec) = self.required(at + 2) {
                    self.import_of(spec, line, vec![Binding { local, item: None }]);
                }
                return self.statement_end(at).max(at + 1);
            }
            bindings.push(Binding { local, item: Some("default".into()) });
            at += 1;
            if self.sign(at, ",") {
                at += 1;
            }
        }
        if self.sign(at, "*") && self.word(at + 1) == "as" && self.is_word(at + 2) {
            bindings.push(Binding { local: self.t[at + 2].text.to_string(), item: None });
            at += 3;
        } else if self.sign(at, "{") {
            let close = self.close(at);
            self.named(at + 1, close, &mut bindings);
            at = close + 1;
        }
        if self.word(at) == "from"
            && let Some(spec) = self.text(at + 1)
        {
            self.import_of(spec, line, bindings);
            return self.after_import(at + 2);
        }
        // Not an import this scanner knows. Leave it and read on.
        self.statement_end(start + 1).max(start + 1)
    }

    fn function(&mut self, start: usize, line: u32, default: bool) -> usize {
        let mut at = start + 1;
        if self.sign(at, "*") {
            at += 1;
        }
        let name = if self.is_word(at) {
            at += 1;
            self.t[at - 1].text
        } else if default {
            "default"
        } else {
            ""
        };
        let Some(open) = (at..(at + 64).min(self.t.len())).find(|i| self.sign(*i, "(")) else {
            return self.statement_end(start + 1).max(start + 1);
        };
        let after = self.close(open) + 1;
        match self.body_open(after) {
            Some(body) => {
                let close = self.close(body);
                let used = self.uses(open, close);
                self.define(name, "function", line, close, used);
                close + 1
            }
            // An overload or a `declare function`: it says a function exists but holds no code.
            None => self.statement_end(after).max(after),
        }
    }

    /// One member of a class, starting at `start`. Returns where the next one starts.
    fn member(&mut self, start: usize, close: usize, class: &str, own: &mut Vec<Use>) -> usize {
        let mut at = start;
        while self.sign(at, "@") {
            // A decorator: `@Input()`, `@a.b`.
            at += 1;
            while at < close && (self.is_word(at) || self.sign(at, ".")) {
                at += 1;
            }
            if self.sign(at, "(") {
                at = self.close(at) + 1;
            }
        }
        loop {
            let modifier = matches!(
                self.word(at),
                "static"
                    | "async"
                    | "public"
                    | "private"
                    | "protected"
                    | "readonly"
                    | "override"
                    | "abstract"
                    | "declare"
                    | "accessor"
                    | "get"
                    | "set"
            );
            // `get()` is a method called "get"; `get age()` reads a property.
            let is_the_name = ["(", "=", ":", ";", "<", "?", "!"].iter().any(|sign| self.sign(at + 1, sign));
            if modifier && !is_the_name && at + 1 < close {
                at += 1;
            } else {
                break;
            }
        }
        if self.sign(at, "*") || self.sign(at, "#") {
            at += 1;
        }
        if self.sign(at, "{") {
            // `static { ... }` runs when the class is made.
            let end = self.close(at);
            own.extend(self.uses(at + 1, end));
            return end + 1;
        }
        let mut name = None;
        let line = self.t.get(at).map_or(1, |token| token.line);
        if self.sign(at, "[") {
            // A name worked out at run time, `[Symbol.iterator]`: it has no name to show.
            at = self.close(at) + 1;
        } else if self.t.get(at).is_some_and(|token| token.kind != Kind::Sign) {
            name = Some(self.t[at].text);
            at += 1;
        }
        if self.sign(at, "?") || self.sign(at, "!") {
            at += 1;
        }
        if self.sign(at, "<") {
            match (at..(at + 32).min(close)).find(|i| self.sign(*i, "(")) {
                Some(open) => at = open,
                None => return self.statement_end(at).clamp(at + 1, close.max(at + 1)),
            }
        }
        if self.sign(at, "(") {
            let after = self.close(at) + 1;
            return match self.body_open(after) {
                Some(body) if body < close => {
                    let end = self.close(body);
                    let used = self.uses(at, end);
                    match name {
                        Some(name) => self.define(&format!("{class}.{name}"), "function", line, end, used),
                        None => own.extend(used),
                    }
                    end + 1
                }
                _ => self.statement_end(after).clamp(after, close.max(after)),
            };
        }
        // A property: `name = value;`, `name: Type = value;`.
        let end = self.statement_end(at).clamp(at, close.max(at));
        let value = self.outside_brackets(at, end, "=").map(|eq| eq + 1);
        match (name, value) {
            // `close = async () => { ... }` is a method in all but spelling.
            (Some(name), Some(value)) if self.is_function_value(value) => {
                let used = self.uses(value, end);
                self.define(&format!("{class}.{name}"), "function", line, end.saturating_sub(1), used);
            }
            _ => own.extend(self.uses(at, end)),
        }
        end.max(start + 1)
    }

    fn class(&mut self, start: usize, line: u32, default: bool) -> usize {
        let named = self.is_word(start + 1) && !matches!(self.word(start + 1), "extends" | "implements");
        let name = if named {
            self.t[start + 1].text.to_string()
        } else if default {
            "default".to_string()
        } else {
            String::new()
        };
        let head = start + 1 + usize::from(named);
        let Some(open) = self.body_open(head) else {
            return self.statement_end(start + 1).max(start + 1);
        };
        let close = self.close(open);
        // What the class itself uses: what it extends and what its properties start as.
        let mut own = self.uses(head, open);
        let index = self.facts.defs.len();
        self.define(if name.is_empty() { "default" } else { &name }, "class", line, close, vec![]);
        let mut at = open + 1;
        while at < close {
            at = self.member(at, close, &name, &mut own).max(at + 1);
        }
        self.facts.defs[index].uses = own;
        close + 1
    }

    fn variable(&mut self, start: usize, line: u32) -> usize {
        let word = if self.word(start) == "const" { "const" } else { "variable" };
        let at = start + 1;
        let end = self.statement_end(at).max(at);
        if self.is_word(at) {
            let name = self.t[at].text;
            let value = self.outside_brackets(at + 1, end, "=").map(|eq| eq + 1);
            if let Some(spec) = value.and_then(|value| self.required(value)) {
                // `const fs = require('fs')` is an import written the older way.
                self.import_of(spec, line, vec![Binding { local: name.to_string(), item: None }]);
                return end;
            }
            let kind = if value.is_some_and(|value| self.is_function_value(value)) { "function" } else { word };
            let used = self.uses(at + 1, end);
            self.define(name, kind, line, end.saturating_sub(1), used);
        } else if self.sign(at, "{") || self.sign(at, "[") {
            let close = self.close(at);
            let spec = if self.sign(close + 1, "=") { self.required(close + 2) } else { None };
            match spec {
                // `const { join, resolve: res } = require('path')`.
                Some(spec) if self.sign(at, "{") => {
                    let mut bindings = Vec::new();
                    let mut name = at + 1;
                    while name < close {
                        if self.is_word(name) {
                            let item = self.t[name].text;
                            let renamed = self.sign(name + 1, ":") && self.is_word(name + 2);
                            let local = if renamed { self.t[name + 2].text } else { item };
                            bindings.push(Binding { local: local.to_string(), item: Some(item.to_string()) });
                        }
                        while name < close && !self.sign(name, ",") {
                            name += 1;
                        }
                        name += 1;
                    }
                    self.import_of(spec, line, bindings);
                }
                // Names taken out of a value: plain code that runs when the file loads.
                _ => {
                    let used = self.uses(at, end);
                    self.facts.loose.extend(used);
                }
            }
        }
        end
    }

    /// `type Name = ...`: runs to the end of the statement.
    fn alias(&mut self, start: usize, line: u32) -> usize {
        let name = self.t[start + 1].text;
        let end = self.statement_end(start + 1).max(start + 2);
        let used = self.uses(start + 2, end);
        self.define(name, "type", line, end.saturating_sub(1), used);
        end
    }

    /// `interface Name { ... }` and `enum Name { ... }`.
    fn block(&mut self, start: usize, line: u32, kind: &'static str) -> usize {
        let name = self.t[start + 1].text;
        let Some(open) = self.body_open(start + 2) else {
            return self.statement_end(start + 1).max(start + 2);
        };
        let close = self.close(open);
        let used = self.uses(start + 2, close);
        self.define(name, kind, line, close, used);
        close + 1
    }

    /// `router.post('/login', limiter, handler)`: an object, a verb, a path
    /// and at least one handler. The handlers are what the route uses.
    fn route(&mut self, at: usize, line: u32) -> Option<usize> {
        let shaped = self.is_word(at) && self.sign(at + 1, ".") && self.is_word(at + 2) && self.sign(at + 3, "(");
        let verb = self.word(at + 2);
        if !shaped || !VERBS.contains(&verb) || !self.sign(at + 5, ",") {
            return None;
        }
        let path = self.text(at + 4).filter(|path| path.starts_with('/'))?;
        let end = self.statement_end(at).max(at + 6);
        let used = self.uses(at + 5, end);
        self.define(&format!("{} {path}", verb.to_uppercase()), "route", line, end.saturating_sub(1), used);
        Some(end)
    }

    /// `exports.name = ...` and `module.exports.name = ...`.
    fn old_export(&mut self, at: usize, line: u32) -> Option<usize> {
        let exports = if self.word(at) == "module" && self.sign(at + 1, ".") && self.word(at + 2) == "exports" {
            at + 2
        } else if self.word(at) == "exports" {
            at
        } else {
            return None;
        };
        let named = self.sign(exports + 1, ".") && self.is_word(exports + 2) && self.sign(exports + 3, "=");
        if !named || self.sign(exports + 4, "=") {
            return None;
        }
        let name = self.t[exports + 2].text;
        let end = self.statement_end(at).max(exports + 4);
        let kind = if self.is_function_value(exports + 4) { "function" } else { "const" };
        let used = self.uses(exports + 4, end);
        self.define(name, kind, line, end.saturating_sub(1), used);
        Some(end)
    }

    /// Reads one statement at the top level. Returns where the next one starts.
    fn statement(&mut self, start: usize) -> usize {
        let line = self.t[start].line;
        let mut at = start;
        let mut default = false;
        loop {
            match self.word(at) {
                "export" | "declare" | "abstract" => at += 1,
                "default" if at > start => {
                    default = true;
                    at += 1;
                }
                "async" if self.word(at + 1) == "function" => at += 1,
                _ => break,
            }
        }
        if at >= self.t.len() {
            return self.t.len();
        }
        let exported = at > start;
        match self.word(at) {
            "import" if !self.sign(at + 1, "(") && !self.sign(at + 1, ".") => return self.import(at),
            "function" => return self.function(at, line, default),
            "class" => return self.class(at, line, default),
            "const" if self.word(at + 1) == "enum" && self.is_word(at + 2) => return self.block(at + 1, line, "enum"),
            "const" | "let" | "var" => return self.variable(at, line),
            "type" if self.is_word(at + 1) && (self.sign(at + 2, "=") || self.sign(at + 2, "<")) => {
                return self.alias(at, line);
            }
            "interface" if self.is_word(at + 1) => return self.block(at, line, "interface"),
            "enum" if self.is_word(at + 1) => return self.block(at, line, "enum"),
            "namespace" | "module" | "global" => {
                // What is inside belongs to the namespace, not to the file's top level.
                if let Some(open) = (at + 1..(at + 4).min(self.t.len())).find(|i| self.sign(*i, "{")) {
                    return self.close(open) + 1;
                }
            }
            _ => {}
        }
        if exported && (self.sign(at, "{") || self.sign(at, "*")) {
            // `export { a } from './x'` and `export * from './x'` pass another file's names on.
            let end = self.statement_end(at).max(at + 1);
            let from = (at..end).find(|i| self.word(*i) == "from" && self.text(i + 1).is_some());
            if let Some(from) = from {
                let spec = self.text(from + 1).unwrap_or_default();
                self.import_of(spec, line, vec![]);
                return self.after_import(from + 2).max(end);
            }
            let used = self.uses(at, end);
            self.facts.loose.extend(used);
            return end;
        }
        if default {
            let end = self.statement_end(at).max(at + 1);
            // `export default name;` points at something the file defines elsewhere.
            let only_a_name = self.is_word(at) && (at + 1 >= end || self.sign(at + 1, ";"));
            let used = self.uses(at, end);
            if only_a_name {
                self.facts.loose.extend(used);
            } else {
                let kind = if self.is_function_value(at) { "function" } else { "const" };
                self.define("default", kind, line, end.saturating_sub(1), used);
            }
            return end;
        }
        if let Some(end) = self.old_export(at, line) {
            return end;
        }
        if let Some(end) = self.route(at, line) {
            return end;
        }
        // Anything else runs when the file loads: tests, set-up, calls.
        let end = self.statement_end(at).max(at + 1);
        let used = self.uses(start, end);
        self.facts.loose.extend(used);
        end
    }

    /// `require('x')` and `import('x')` anywhere in the file are needed files too.
    fn imports_in_code(&mut self) {
        for at in 0..self.t.len() {
            let asks =
                matches!(self.word(at), "require" | "import") && self.sign(at + 1, "(") && self.sign(at + 3, ")");
            let Some(spec) = self.text(at + 2).filter(|_| asks) else { continue };
            if !self.facts.imports.iter().any(|import| import.spec == spec) {
                self.import_of(spec, self.t[at].line, vec![]);
            }
        }
    }

    /// `test('name', ...)` and `it('name', ...)`, with `.only`, `.skip` and the like.
    fn count_tests(&self) -> usize {
        let mut count = 0;
        for at in 0..self.t.len() {
            if !matches!(self.word(at), "test" | "it") || (at > 0 && self.sign(at - 1, ".")) {
                continue;
            }
            let plain = self.sign(at + 1, "(") && self.text(at + 2).is_some();
            let marked = self.sign(at + 1, ".")
                && matches!(self.word(at + 2), "only" | "skip" | "concurrent" | "fails" | "failing" | "todo")
                && self.sign(at + 3, "(")
                && self.text(at + 4).is_some();
            count += usize::from(plain || marked);
        }
        count
    }
}

pub fn scan(text: &str) -> Facts {
    let tokens = lex(text, Syntax::Js);
    let partner = pairs(&tokens);
    let mut reader = Reader { t: &tokens, partner: &partner, facts: Facts::default() };
    let mut at = 0;
    while at < tokens.len() {
        at = reader.statement(at).max(at + 1);
    }
    reader.imports_in_code();
    reader.facts.tests = reader.count_tests();
    reader.facts
}

#[cfg(test)]
mod tests {
    use super::super::scan::testing::{calls, defs, imports, lines, names};
    use super::*;

    #[test]
    fn imports_in_every_spelling() {
        let facts = scan(
            r#"
import express, { Router, type Request as Req } from 'express';
import * as limits from './limits/config';
import './setup';
import type { Order } from "../types";
import data from './data.json' with { type: 'json' };
import fs = require('fs');
export { helper } from './helper';
export * from './all';
const os = require('node:os');
const { join, resolve: res } = require('path');
const lazy = () => import('./lazy');
"#,
        );
        assert_eq!(
            imports(&facts),
            vec![
                "express: express<-default, Router, Req<-Request",
                "./limits/config: limits<-*",
                "./setup",
                "../types: Order",
                "./data.json: data<-default",
                "fs: fs<-*",
                "./helper",
                "./all",
                "node:os: os<-*",
                "path: join, res<-resolve",
                "./lazy",
            ]
        );
        assert_eq!(facts.imports[1].line, 3);
        assert_eq!(defs(&facts), vec!["function lazy"]);
    }

    const SHOP: &str = r#"export const loginLimiter = rateLimit({
  windowMs: 60 * 1000,
  limit: limits.login,
  handler(req, res) {
    res.status(429).json({ error: 'Too many attempts' });
  },
});

export async function login(req: Req, res): Promise<{ ok: boolean }> {
  const user = await findUser(req.body.name);
  if (!user) return res.status(401).end();
  return checkPassword(user, req.body.password) ? res.json({ ok: true }) : null;
}

export default class Session extends Base<{ id: string }> {
  static open(id: string) { return new Session(id); }
  #secret = hash(this.id);
  close = async () => { await store.remove(this.id); };
  get age(): number { return now() - this.started; }
  abstract check(): void;
}

export type Order = { id: number; total: number }
export interface Shape { area(): number }
export enum Color { Red, Green }
const answer = 42
let counter = 0
function* walk() { yield* inner(); }
export const App = () => <Layout title="x"><Row items={load()} /></Layout>;
router.post('/login', loginLimiter, login);
exports.legacy = function () { return old(); };
"#;

    #[test]
    fn definitions_with_their_kinds_and_lines() {
        let facts = scan(SHOP);
        assert_eq!(
            defs(&facts),
            vec![
                "const loginLimiter",
                "function login",
                "class Session",
                "function Session.open",
                "function Session.close",
                "function Session.age",
                "type Order",
                "interface Shape",
                "enum Color",
                "const answer",
                "variable counter",
                "function walk",
                "function App",
                "route POST /login",
                "function legacy",
            ]
        );
        assert_eq!(lines(&facts, "loginLimiter"), (1, 7));
        assert_eq!(lines(&facts, "login"), (9, 13));
        assert_eq!(lines(&facts, "Session"), (15, 21));
        assert_eq!(lines(&facts, "Session.close"), (18, 18));
        assert_eq!(lines(&facts, "answer"), (26, 26));
        assert_eq!(lines(&facts, "POST /login"), (30, 30));
    }

    #[test]
    fn calls_and_names_inside_a_definition() {
        let facts = scan(SHOP);
        // `handler(req, res) {` defines a method; it is not a call.
        assert_eq!(calls(&facts, "loginLimiter"), vec!["rateLimit", "res.status", ".json"]);
        assert!(names(&facts, "loginLimiter").contains(&"limits.login".to_string()));
        assert_eq!(calls(&facts, "login"), vec!["findUser", "res.status", ".end", "checkPassword", "res.json"]);
        assert_eq!(calls(&facts, "Session.open"), vec!["Session"]);
        assert_eq!(calls(&facts, "Session.close"), vec!["store.remove"]);
        assert_eq!(calls(&facts, "Session.age"), vec!["now"]);
        // What a class uses itself: what it extends and what its properties start as.
        assert_eq!(names(&facts, "Session"), vec!["Base", "hash", "this.id"]);
        assert_eq!(calls(&facts, "walk"), vec!["inner"]);
        // JSX names the components it draws; code in braces is read as code.
        assert_eq!(names(&facts, "App"), vec!["Layout", "title", "Row", "items", "load"]);
        assert_eq!(calls(&facts, "App"), vec!["load"]);
        // A route uses its handlers without calling them itself.
        assert_eq!(names(&facts, "POST /login"), vec!["loginLimiter", "login"]);
        assert_eq!(calls(&facts, "legacy"), vec!["old"]);
    }

    #[test]
    fn comments_strings_templates_and_regexes_that_look_like_code() {
        let facts = scan(
            r#"// function fake() {}
/* class AlsoFake { method() {} } */
const text = "function inString() {}";
const quote = 'import x from "nowhere"';
const tpl = `import y from 'nowhere' ${real()} function no() {}`;
const re = /function\s+(\w+)\(/g;
function real() { return "}" + '{'; }
"#,
        );
        assert_eq!(defs(&facts), vec!["const text", "const quote", "const tpl", "const re", "function real"]);
        assert!(facts.imports.is_empty(), "{:?}", facts.imports);
        assert_eq!(calls(&facts, "tpl"), vec!["real"]);
        assert_eq!(lines(&facts, "real"), (7, 7));
    }

    #[test]
    fn statements_without_semicolons_end_where_the_next_one_starts() {
        let facts = scan("const a = 1\nconst b = a\n  .toString()\ntype T =\n  | A\n  | B\nexport function c() {}\n");
        assert_eq!(defs(&facts), vec!["const a", "const b", "type T", "function c"]);
        assert_eq!(lines(&facts, "b"), (2, 3));
        assert_eq!(lines(&facts, "T"), (4, 6));
    }

    #[test]
    fn overloads_and_declarations_without_a_body_are_not_definitions() {
        let facts = scan(
            "export function pick(a: string): string;\nexport function pick(a: number): number;\nexport function pick(a: any) { return a; }\ndeclare function later(): void;\n",
        );
        assert_eq!(defs(&facts), vec!["function pick"]);
        assert_eq!(lines(&facts, "pick"), (3, 3));
    }

    #[test]
    fn tests_are_counted_and_what_they_use_belongs_to_the_file() {
        let facts = scan(
            r#"import { test, expect } from 'vitest';
describe('limiter', () => {
  test('allows five', () => { expect(loginLimiter).toBeDefined(); });
  it('blocks the sixth', async () => { await hit(6); });
  it.skip('later', () => {});
  test.each([1, 2])('n=%d', () => {});
  obj.test('not a test');
});
"#,
        );
        assert_eq!(facts.tests, 3);
        assert!(facts.defs.is_empty());
        let loose: Vec<_> = facts.loose.iter().map(|u| u.chain.as_str()).collect();
        assert!(loose.contains(&"loginLimiter") && loose.contains(&"hit"), "{loose:?}");
    }

    #[test]
    fn default_exports_and_arrow_functions() {
        let facts = scan(
            "export default async (req, res) => { await handle(req); };\nexport const typed = (a: number): number => a * 2;\nconst generic = <T,>(value: T) => value;\nexport const wrapped = memo(() => null);\n",
        );
        assert_eq!(defs(&facts), vec!["function default", "function typed", "function generic", "const wrapped"]);
        assert_eq!(calls(&facts, "default"), vec!["handle"]);
    }

    #[test]
    fn names_a_function_gives_to_its_own_values_are_not_calls_it_makes() {
        let facts = scan(
            r#"export function Panel(props) {
  const [open, setOpen] = createSignal(false);
  const toggle = () => setOpen(!open());
  function helper() { return format(props.title); }
  let { a, b: renamed } = load();
  for (const item of items()) { item.run(); show(item); }
  toggle(); helper(); renamed(); a();
  return render(open());
}
"#,
        );
        // `toggle`, `helper` and the signal's two halves live inside Panel. A method of a value of its own still shows.
        assert_eq!(
            calls(&facts, "Panel"),
            vec!["createSignal", "format", "load", "items", "item.run", "show", "render"]
        );
    }
}
