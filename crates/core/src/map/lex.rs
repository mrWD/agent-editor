//! Cuts source text into words, names and signs, and throws away what only
//! looks like code: comments and the insides of strings.
//!
//! This is not a parser. It knows just enough of each language to tell where a
//! string or a comment ends, because a brace inside a string must not count as
//! a brace. Everything after that works on the pieces made here.

/// The family a language's spelling belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// JavaScript and TypeScript, with or without JSX.
    Js,
    Rust,
    Go,
    Python,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A name or a keyword.
    Word,
    Number,
    /// A string, a character or a regular expression. `text` is what is between the quotes.
    Text,
    /// Any other sign: a bracket, a dot, an operator.
    Sign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: Kind,
    pub text: &'a str,
    /// The line the token starts on, from 1.
    pub line: u32,
}

impl Token<'_> {
    pub fn is(&self, sign: &str) -> bool {
        self.kind == Kind::Sign && self.text == sign
    }

    pub fn word(&self, word: &str) -> bool {
        self.kind == Kind::Word && self.text == word
    }

    pub fn is_word(&self) -> bool {
        self.kind == Kind::Word
    }
}

fn starts_name(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' || byte >= 0x80
}

fn in_name(byte: u8) -> bool {
    starts_name(byte) || byte.is_ascii_digit()
}

/// Words after which a slash starts a regular expression, not a division.
const BEFORE_REGEX: &[&str] = &[
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
];

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    at: usize,
    line: u32,
    syntax: Syntax,
    out: Vec<Token<'a>>,
    /// How many `{` are open, for telling the end of `${ ... }` in a template.
    braces: i32,
    /// For each `${` still open: how many braces were open when it started.
    templates: Vec<i32>,
}

impl<'a> Lexer<'a> {
    fn peek(&self, ahead: usize) -> u8 {
        self.bytes.get(self.at + ahead).copied().unwrap_or(0)
    }

    fn push(&mut self, kind: Kind, from: usize, to: usize, line: u32) {
        self.out.push(Token { kind, text: &self.src[from..to], line });
    }

    /// Moves past bytes up to `to`, counting the lines on the way.
    fn skip_to(&mut self, to: usize) {
        let to = to.min(self.bytes.len());
        self.line += self.bytes[self.at..to].iter().filter(|b| **b == b'\n').count() as u32;
        self.at = to;
    }

    fn line_comment(&mut self) {
        while self.at < self.bytes.len() && self.bytes[self.at] != b'\n' {
            self.at += 1;
        }
    }

    fn block_comment(&mut self) {
        // Rust lets comments nest; the others end at the first `*/`.
        let nests = self.syntax == Syntax::Rust;
        let mut depth = 1;
        let mut i = self.at + 2;
        while i < self.bytes.len() && depth > 0 {
            if self.bytes[i] == b'*' && self.bytes.get(i + 1) == Some(&b'/') {
                depth -= 1;
                i += 2;
            } else if nests && self.bytes[i] == b'/' && self.bytes.get(i + 1) == Some(&b'*') {
                depth += 1;
                i += 2;
            } else {
                i += 1;
            }
        }
        self.skip_to(i);
    }

    /// A string in quotes. `multiline` says whether it may run over a line end.
    /// Returns false when the quote was not a string after all: it is then a sign.
    fn quoted(&mut self, quote: u8, multiline: bool) -> bool {
        let start = self.at;
        let line = self.line;
        let mut i = start + 1;
        while i < self.bytes.len() {
            match self.bytes[i] {
                b'\\' => i += 2,
                b'\n' if !multiline => break,
                byte if byte == quote => {
                    self.skip_to(i + 1);
                    self.push(Kind::Text, start + 1, i, line);
                    return true;
                }
                _ => i += 1,
            }
        }
        if multiline {
            // The file ends inside the string: nothing more to read.
            let end = self.bytes.len();
            self.skip_to(end);
            self.push(Kind::Text, start + 1, end, line);
            return true;
        }
        // No closing quote on this line. In JSX this is an apostrophe in plain
        // text ("Don't"); taking the rest of the file as a string would be worse.
        false
    }

    /// Python's `'''` and `"""`, which run until the same three quotes.
    fn triple_quoted(&mut self, quote: u8) {
        let start = self.at + 3;
        let line = self.line;
        let mut i = start;
        while i < self.bytes.len() {
            if self.bytes[i] == b'\\' {
                i += 2;
            } else if self.bytes[i] == quote
                && self.bytes.get(i + 1) == Some(&quote)
                && self.bytes.get(i + 2) == Some(&quote)
            {
                self.skip_to(i + 3);
                self.push(Kind::Text, start, i, line);
                return;
            } else {
                i += 1;
            }
        }
        let end = self.bytes.len();
        self.skip_to(end);
        self.push(Kind::Text, start.min(end), end, line);
    }

    /// The text of a JavaScript template from `self.at` on. Stops after the
    /// closing backtick, or after a `${`, which is remembered so that its `}`
    /// brings the lexer back here. Returns where the text ended.
    fn template(&mut self) -> usize {
        let mut i = self.at;
        while i < self.bytes.len() {
            match self.bytes[i] {
                b'\\' => i += 2,
                b'`' => {
                    self.skip_to(i + 1);
                    return i;
                }
                b'$' if self.bytes.get(i + 1) == Some(&b'{') => {
                    self.skip_to(i + 2);
                    self.templates.push(self.braces);
                    return i;
                }
                _ => i += 1,
            }
        }
        let end = self.bytes.len();
        self.skip_to(end);
        end
    }

    /// Whether a slash here starts a regular expression.
    fn regex_may_start(&self) -> bool {
        match self.out.last() {
            None => true,
            Some(last) => match last.kind {
                Kind::Word => BEFORE_REGEX.contains(&last.text),
                Kind::Number | Kind::Text => false,
                // After a closing bracket a slash divides. After `<` it closes a JSX tag.
                Kind::Sign => !matches!(last.text, ")" | "]" | "}" | "<"),
            },
        }
    }

    /// A regular expression literal. Returns false when the line ends before it does.
    fn regex(&mut self) -> bool {
        let start = self.at;
        let mut i = start + 1;
        let mut in_class = false;
        while i < self.bytes.len() {
            match self.bytes[i] {
                b'\\' => i += 2,
                b'\n' => return false,
                b'[' => {
                    in_class = true;
                    i += 1;
                }
                b']' => {
                    in_class = false;
                    i += 1;
                }
                b'/' if !in_class => {
                    let end = i;
                    i += 1;
                    while i < self.bytes.len() && self.bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    self.at = i;
                    self.push(Kind::Text, start + 1, end, self.line);
                    return true;
                }
                _ => i += 1,
            }
        }
        false
    }

    /// Rust's raw strings, `r"..."` and `r#"..."#`, starting at the quote or the first `#`.
    /// Returns false when what follows is not a raw string.
    fn raw_string(&mut self, from: usize) -> bool {
        let mut i = from;
        let mut hashes = 0;
        while self.bytes.get(i) == Some(&b'#') {
            hashes += 1;
            i += 1;
        }
        if self.bytes.get(i) != Some(&b'"') {
            return false;
        }
        let start = i + 1;
        let line = self.line;
        let mut j = start;
        while j < self.bytes.len() {
            if self.bytes[j] == b'"' && (0..hashes).all(|n| self.bytes.get(j + 1 + n) == Some(&b'#')) {
                self.skip_to(j + 1 + hashes);
                self.push(Kind::Text, start, j, line);
                return true;
            }
            j += 1;
        }
        let end = self.bytes.len();
        self.skip_to(end);
        self.push(Kind::Text, start.min(end), end, line);
        true
    }

    /// In Rust a single quote starts a character (`'a'`, `'\n'`) or a lifetime (`'a`).
    fn rust_quote(&mut self) {
        let next = self.peek(1);
        if next == b'\\' {
            let mut i = self.at + 3;
            while i < self.bytes.len() && self.bytes[i] != b'\'' && self.bytes[i] != b'\n' {
                i += 1;
            }
            let line = self.line;
            self.push(Kind::Text, self.at + 1, i.min(self.bytes.len()), line);
            self.at = (i + 1).min(self.bytes.len());
            return;
        }
        // One character, which may take several bytes, and then the closing quote.
        let width = match next {
            0..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            _ => 4,
        };
        if self.bytes.get(self.at + 1 + width) == Some(&b'\'') && next != b'\n' {
            let line = self.line;
            self.push(Kind::Text, self.at + 1, self.at + 1 + width, line);
            self.at += width + 2;
            return;
        }
        // A lifetime or a loop label: it names nothing the map shows.
        self.at += 1;
        while self.at < self.bytes.len() && in_name(self.bytes[self.at]) {
            self.at += 1;
        }
    }

    fn name(&mut self) {
        let start = self.at;
        let mut i = start;
        while i < self.bytes.len() && in_name(self.bytes[i]) {
            i += 1;
        }
        let word = &self.src[start..i];
        let after = self.bytes.get(i).copied().unwrap_or(0);

        if self.syntax == Syntax::Rust {
            // Letters in front of a string say how to read it: `b"..."`, `r#"..."#`, `c"..."`.
            if matches!(word, "r" | "br" | "cr") && (after == b'"' || after == b'#') && self.raw_string(i) {
                return;
            }
            if matches!(word, "b" | "c") && after == b'"' {
                self.at = i;
                self.quoted(b'"', true);
                return;
            }
            if word == "b" && after == b'\'' {
                self.at = i;
                self.rust_quote();
                return;
            }
            // `r#type` is the name `type`.
            if word == "r" && after == b'#' && self.bytes.get(i + 1).is_some_and(|b| starts_name(*b)) {
                self.at = i + 1;
                return self.name();
            }
        }
        if self.syntax == Syntax::Python
            && (after == b'"' || after == b'\'')
            && word.len() <= 2
            && word.bytes().all(|b| b"rRbBfFuU".contains(&b))
        {
            // `f"..."`, `rb'...'`: the letters belong to the string, they are not a name.
            self.at = i;
            return;
        }
        self.at = i;
        self.push(Kind::Word, start, i, self.line);
    }

    fn number(&mut self) {
        let start = self.at;
        let mut i = start;
        while i < self.bytes.len() {
            let byte = self.bytes[i];
            let dot_in_number = byte == b'.' && self.bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
            if byte.is_ascii_alphanumeric() || byte == b'_' || dot_in_number {
                i += 1;
            } else {
                break;
            }
        }
        self.at = i;
        self.push(Kind::Number, start, i, self.line);
    }

    fn sign(&mut self) {
        let start = self.at;
        let (a, b, c) = (self.peek(0), self.peek(1), self.peek(2));
        let width = match (a, b, c) {
            (b'.', b'.', b'.') => 3,
            (b'=', b'>', _) | (b'-', b'>', _) | (b':', b':', _) => 2,
            // `a?.b` reads as `a.b`. `a ? .5 : 1` is not that.
            (b'?', b'.', next) if self.syntax == Syntax::Js && !next.is_ascii_digit() => {
                self.at += 1;
                return self.sign();
            }
            // A character that is not ASCII and not part of a name cannot be cut in the middle.
            _ => 1,
        };
        self.at += width;
        self.push(Kind::Sign, start, start + width, self.line);
    }

    fn run(mut self) -> Vec<Token<'a>> {
        while self.at < self.bytes.len() {
            let byte = self.bytes[self.at];
            match byte {
                b'\n' => {
                    self.line += 1;
                    self.at += 1;
                }
                b' ' | b'\t' | b'\r' => self.at += 1,
                b'#' if self.syntax == Syntax::Python => self.line_comment(),
                b'/' if self.syntax != Syntax::Python && self.peek(1) == b'/' => self.line_comment(),
                b'/' if self.syntax != Syntax::Python && self.peek(1) == b'*' => self.block_comment(),
                b'/' if self.syntax == Syntax::Js && self.regex_may_start() => {
                    if !self.regex() {
                        self.sign();
                    }
                }
                b'"' | b'\'' if self.syntax == Syntax::Python => {
                    if self.peek(1) == byte && self.peek(2) == byte {
                        self.triple_quoted(byte);
                    } else if !self.quoted(byte, false) {
                        self.sign();
                    }
                }
                b'"' => {
                    if !self.quoted(b'"', self.syntax == Syntax::Rust) {
                        self.sign();
                    }
                }
                b'\'' if self.syntax == Syntax::Rust => self.rust_quote(),
                b'\'' => {
                    if !self.quoted(b'\'', false) {
                        self.sign();
                    }
                }
                b'`' if self.syntax == Syntax::Js => {
                    // The template is one piece of text; code inside `${ }` is read as code.
                    let start = self.at + 1;
                    let line = self.line;
                    self.at += 1;
                    let end = self.template();
                    self.push(Kind::Text, start, end.max(start), line);
                }
                b'`' if self.syntax == Syntax::Go => {
                    let start = self.at + 1;
                    let line = self.line;
                    let end =
                        self.bytes[start..].iter().position(|b| *b == b'`').map_or(self.bytes.len(), |n| start + n);
                    self.skip_to((end + 1).min(self.bytes.len()));
                    self.push(Kind::Text, start, end, line);
                }
                b'{' => {
                    self.braces += 1;
                    self.sign();
                }
                b'}' => {
                    if self.templates.last() == Some(&self.braces) {
                        // The end of `${ ... }`: back to the template's text.
                        self.templates.pop();
                        self.at += 1;
                        self.template();
                    } else {
                        self.braces -= 1;
                        self.sign();
                    }
                }
                b'0'..=b'9' => self.number(),
                byte if starts_name(byte) => self.name(),
                _ => self.sign(),
            }
        }
        self.out
    }
}

/// Cuts `src` into tokens. Comments are left out; a string is one token.
pub fn lex(src: &str, syntax: Syntax) -> Vec<Token<'_>> {
    let lexer = Lexer {
        src,
        bytes: src.as_bytes(),
        at: 0,
        line: 1,
        syntax,
        out: Vec::with_capacity(src.len() / 5),
        braces: 0,
        templates: Vec::new(),
    };
    lexer.run()
}

/// No partner: the bracket is not closed, or closes nothing.
pub const ALONE: usize = usize::MAX;

/// For every bracket, where its partner is. Other tokens, and brackets
/// without a partner, get `ALONE`.
pub fn pairs(tokens: &[Token]) -> Vec<usize> {
    let mut partner = vec![ALONE; tokens.len()];
    let mut open: Vec<usize> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != Kind::Sign {
            continue;
        }
        let wanted = match token.text {
            "(" | "[" | "{" => {
                open.push(index);
                continue;
            }
            ")" => "(",
            "]" => "[",
            "}" => "{",
            _ => continue,
        };
        // A closer takes the nearest opener of its own kind. Openers above it
        // were never closed (a slip of the lexer, or broken code) and are dropped.
        if let Some(at) = open.iter().rposition(|i| tokens[*i].text == wanted) {
            let opener = open[at];
            open.truncate(at);
            partner[opener] = index;
            partner[index] = opener;
        }
    }
    partner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(src: &str, syntax: Syntax) -> Vec<&str> {
        lex(src, syntax).into_iter().filter(|t| t.kind != Kind::Text).map(|t| t.text).collect()
    }

    #[test]
    fn comments_and_strings_do_not_reach_the_tokens() {
        let src = "const a = 'x { y'; // } not code\n/* { */ call(\"}\");";
        assert_eq!(texts(src, Syntax::Js), vec!["const", "a", "=", ";", "call", "(", ")", ";"]);
        let strings: Vec<_> =
            lex(src, Syntax::Js).into_iter().filter(|t| t.kind == Kind::Text).map(|t| t.text).collect();
        assert_eq!(strings, vec!["x { y", "}"]);
    }

    #[test]
    fn lines_are_counted_through_comments_and_strings() {
        let src = "/* one\n two */\nlet a = `x\ny`;\nb";
        let tokens = lex(src, Syntax::Js);
        assert_eq!(tokens.first().map(|t| (t.text, t.line)), Some(("let", 3)));
        assert_eq!(tokens.last().map(|t| (t.text, t.line)), Some(("b", 5)));
    }

    #[test]
    fn code_inside_a_template_is_read_and_its_text_is_not() {
        let src = "const s = `a { ${ f(x, `in ${g()} }`) } b }`; after()";
        assert_eq!(
            texts(src, Syntax::Js),
            vec!["const", "s", "=", "f", "(", "x", ",", "g", "(", ")", ")", ";", "after", "(", ")"]
        );
    }

    #[test]
    fn a_slash_is_a_regex_only_where_a_value_may_start() {
        assert_eq!(texts("a = b / c / d;", Syntax::Js), vec!["a", "=", "b", "/", "c", "/", "d", ";"]);
        assert_eq!(texts("x = /[{/]}/g.test(s); y", Syntax::Js), vec!["x", "=", ".", "test", "(", "s", ")", ";", "y"]);
        assert_eq!(texts("return /}/;", Syntax::Js), vec!["return", ";"]);
        // A closing tag is not a regex, and a brace after it still counts.
        assert_eq!(
            texts("<p>{a && <b>x</b>}</p>", Syntax::Js),
            vec!["<", "p", ">", "{", "a", "&", "&", "<", "b", ">", "x", "<", "/", "b", ">", "}", "<", "/", "p", ">"]
        );
    }

    #[test]
    fn an_apostrophe_in_jsx_text_does_not_swallow_the_code_after_it() {
        let src = "<p>Don't</p>\nconst next = 1;";
        let tokens = texts(src, Syntax::Js);
        assert!(tokens.ends_with(&["const", "next", "=", "1", ";"]), "{tokens:?}");
    }

    #[test]
    fn optional_chaining_reads_as_a_dot() {
        assert_eq!(texts("a?.b(c ? .5 : 1)", Syntax::Js), vec!["a", ".", "b", "(", "c", "?", ".", "5", ":", "1", ")"]);
    }

    #[test]
    fn rust_strings_characters_and_lifetimes() {
        let src = r####"fn f<'a>(x: &'a str) -> char { let s = r#"a "quoted" { brace"#; let c = '{'; let n = '\n'; b"}"; c }"####;
        assert_eq!(
            texts(src, Syntax::Rust),
            vec![
                "fn", "f", "<", ">", "(", "x", ":", "&", "str", ")", "->", "char", "{", "let", "s", "=", ";", "let",
                "c", "=", ";", "let", "n", "=", ";", ";", "c", "}"
            ]
        );
        assert_eq!(texts("/* a /* nested */ still */ x", Syntax::Rust), vec!["x"]);
        assert_eq!(texts("let r#type = 1;", Syntax::Rust), vec!["let", "type", "=", "1", ";"]);
    }

    #[test]
    fn go_raw_strings_and_runes() {
        let src = "x := `a {\n b` + \"}\" + string('{')\ny()";
        let tokens = lex(src, Syntax::Go);
        let words: Vec<_> = tokens.iter().filter(|t| t.kind != Kind::Text).map(|t| t.text).collect();
        assert_eq!(words, vec!["x", ":", "=", "+", "+", "string", "(", ")", "y", "(", ")"]);
        assert_eq!(tokens.last().map(|t| t.line), Some(3));
    }

    #[test]
    fn python_comments_prefixes_and_triple_quotes() {
        let src = "x = f\"{a}\"  # not { code\ndoc = \"\"\"\ndef fake(): pass\n\"\"\"\nreal()";
        let tokens = lex(src, Syntax::Python);
        let words: Vec<_> = tokens.iter().filter(|t| t.kind != Kind::Text).map(|t| t.text).collect();
        assert_eq!(words, vec!["x", "=", "doc", "=", "real", "(", ")"]);
        assert_eq!(tokens.last().map(|t| t.line), Some(5));
    }

    #[test]
    fn brackets_find_their_partners_even_when_one_is_missing() {
        let tokens = lex("f(a[1], {b: (c)}) }", Syntax::Js);
        let partner = pairs(&tokens);
        let at = |text: &str| tokens.iter().position(|t| t.text == text).unwrap();
        assert_eq!(partner[at("(")], tokens.len() - 2);
        assert_eq!(partner[at("[")], at("]"));
        assert_eq!(partner[tokens.len() - 1], ALONE, "a closer with nothing to close");
        let broken = lex("f(a { b )", Syntax::Js);
        let partner = pairs(&broken);
        assert_eq!(partner[1], 5, "the unclosed brace does not keep the parenthesis open");
        assert_eq!(partner[3], ALONE);
    }
}
