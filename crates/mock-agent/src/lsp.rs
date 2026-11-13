//! A practice language server: `ae-mock-agent --lsp`.
//!
//! It speaks the Language Server Protocol over stdio, as real language
//! servers do, and knows nothing about any language. Everything it says is
//! scripted, so tests are repeatable and the editor can be tried without
//! installing a server.
//!
//! What it does with a file it is given:
//! - every line with the word `FIXME` gets an error under that word;
//! - completion always offers the same three words;
//! - the word `limits` has hover text, and its definition is in
//!   `src/limits/config.ts` of the folder the server was started for, when that
//!   file is there; the word `outside` is "defined" in a file outside that folder;
//! - formatting takes the spaces and tabs off the end of every line.
//!
//! Words in a file that make it misbehave, to see what the editor does then:
//! - `CRASH-NOW`: the server ends at once, as a crash would;
//! - `SLOW-FORMAT`: formatting takes three seconds;
//! - `BAD-FORMAT`: formatting is refused with an error.
//!
//! Right after it has started it asks its client for settings, as real
//! servers do, and says `practice/configured` when the answer arrives.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

/// The words completion offers, whatever was typed.
const WORDS: [&str; 3] = ["practiceAlpha", "practiceBeta", "practiceGamma"];
/// The word that has hover text and a definition.
const KNOWN_WORD: &str = "limits";
/// The file the known word is defined in, from the folder the server works in.
const DEFINED_IN: &str = "src/limits/config.ts";
/// The word whose definition is outside the folder the server works in.
const OUTSIDE_WORD: &str = "outside";

struct Server {
    out: io::Stdout,
    /// The text of every open file, by its address.
    docs: HashMap<String, String>,
    /// The folder the server was started for.
    root: Option<PathBuf>,
}

/// Runs the server until its input closes or it is told to exit.
pub fn run() {
    let mut server = Server { out: io::stdout(), docs: HashMap::new(), root: None };
    let stdin = io::stdin();
    let mut input = stdin.lock();
    while let Some(body) = read_message(&mut input) {
        match serde_json::from_slice::<Value>(&body) {
            Ok(message) => server.handle(message),
            Err(_) => eprintln!("practice-lsp: not JSON: {}", String::from_utf8_lossy(&body)),
        }
    }
}

/// Reads one message: header lines, an empty line, then as many bytes as the header says.
fn read_message(input: &mut impl BufRead) -> Option<Vec<u8>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            // An empty line ends the headers. Without a length there is nothing to read yet.
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok();
        }
    }
    let mut body = vec![0; length?];
    input.read_exact(&mut body).ok()?;
    Some(body)
}

/// A file address as a path on this machine. Only what this server itself needs: plain `file://` addresses.
fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut at = 0;
    while at < raw.len() {
        if raw[at] == b'%'
            && at + 2 < raw.len()
            && let Ok(byte) = u8::from_str_radix(&rest[at + 1..at + 3], 16)
        {
            bytes.push(byte);
            at += 3;
            continue;
        }
        bytes.push(raw[at]);
        at += 1;
    }
    let text = String::from_utf8(bytes).ok()?;
    // "file:///C:/folder" on Windows: the slash before the drive letter is not part of the path.
    let text = match text.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => text[1..].to_string(),
        _ => text,
    };
    Some(PathBuf::from(text))
}

/// A path as a file address. The other way round from `path_of`.
fn uri_of(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => uri.push(byte as char),
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    uri
}

/// How far into a line a byte offset is, counted the way the protocol counts: in UTF-16 units.
fn column_of(line: &str, byte: usize) -> usize {
    line[..byte].encode_utf16().count()
}

/// The word around a column of a line, with the columns it starts and ends at.
fn word_at(line: &str, column: usize) -> Option<(String, usize, usize)> {
    let units: Vec<u16> = line.encode_utf16().collect();
    let is_word = |unit: u16| char::from_u32(u32::from(unit)).is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let column = column.min(units.len());
    let mut from = column;
    while from > 0 && is_word(units[from - 1]) {
        from -= 1;
    }
    let mut to = column;
    while to < units.len() && is_word(units[to]) {
        to += 1;
    }
    (from < to).then(|| (String::from_utf16_lossy(&units[from..to]), from, to))
}

/// An error for every line that holds the word FIXME, under that word.
fn diagnostics_of(text: &str) -> Vec<Value> {
    text.lines()
        .enumerate()
        .filter_map(|(number, line)| {
            let at = line.find("FIXME")?;
            let from = column_of(line, at);
            Some(json!({
                "range": {
                    "start": { "line": number, "character": from },
                    "end": { "line": number, "character": from + "FIXME".len() }
                },
                "severity": 1,
                "source": "practice",
                "message": "FIXME: finish this before it ships"
            }))
        })
        .collect()
}

/// The edits that take the spaces and tabs off the end of every line.
fn trailing_space_edits(text: &str) -> Vec<Value> {
    text.split('\n')
        .enumerate()
        .filter_map(|(number, line)| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let kept = line.trim_end_matches([' ', '\t']);
            (kept.len() < line.len()).then(|| {
                json!({
                    "range": {
                        "start": { "line": number, "character": column_of(line, kept.len()) },
                        "end": { "line": number, "character": column_of(line, line.len()) }
                    },
                    "newText": ""
                })
            })
        })
        .collect()
}

impl Server {
    fn send(&mut self, message: &Value) {
        let body = message.to_string();
        // A broken pipe means the client is gone; there is nobody to tell.
        let _ = write!(self.out, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = self.out.flush();
    }

    fn respond(&mut self, id: &Value, result: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn respond_error(&mut self, id: &Value, code: i64, message: &str) {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }));
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Keeps the new text of a file and says what is wrong in it.
    fn took(&mut self, uri: &str, text: String) {
        if text.contains("CRASH-NOW") {
            eprintln!("practice-lsp: asked to crash");
            std::process::exit(3);
        }
        let diagnostics = diagnostics_of(&text);
        self.docs.insert(uri.to_string(), text);
        self.notify("textDocument/publishDiagnostics", json!({ "uri": uri, "diagnostics": diagnostics }));
    }

    /// The word a request points at: the file's address, and the word with where it starts and ends.
    fn pointed_at(&self, params: &Value) -> Option<(u64, String, usize, usize)> {
        let uri = params.pointer("/textDocument/uri")?.as_str()?;
        let line_number = params.pointer("/position/line")?.as_u64()?;
        let column = params.pointer("/position/character")?.as_u64()? as usize;
        let line = self.docs.get(uri)?.split('\n').nth(line_number as usize)?;
        let (word, from, to) = word_at(line.strip_suffix('\r').unwrap_or(line), column)?;
        Some((line_number, word, from, to))
    }

    fn hover(&self, params: &Value) -> Value {
        match self.pointed_at(params) {
            Some((line, word, from, to)) if word == KNOWN_WORD => json!({
                "contents": {
                    "kind": "markdown",
                    "value": "**limits**\n\nHow many requests each route allows. This text comes from the practice language server."
                },
                "range": {
                    "start": { "line": line, "character": from },
                    "end": { "line": line, "character": to }
                }
            }),
            _ => Value::Null,
        }
    }

    fn definition(&self, params: &Value) -> Value {
        let Some((_, word, _, _)) = self.pointed_at(params) else { return Value::Null };
        if word == OUTSIDE_WORD {
            // A place no task's folder holds: the editor must not open it.
            let start = json!({ "line": 0, "character": 0 });
            return json!({ "uri": "file:///ae-practice/outside.txt", "range": { "start": start, "end": start } });
        }
        if word != KNOWN_WORD {
            return Value::Null;
        }
        let Some(target) = self.root.as_ref().map(|root| root.join(DEFINED_IN)) else { return Value::Null };
        let Ok(text) = std::fs::read_to_string(&target) else { return Value::Null };
        // The first place the file names the word; the top of the file when it never does.
        let (line, column) = text
            .lines()
            .enumerate()
            .find_map(|(number, line)| line.find(KNOWN_WORD).map(|at| (number, column_of(line, at))))
            .unwrap_or((0, 0));
        json!({
            "uri": uri_of(&target),
            "range": {
                "start": { "line": line, "character": column },
                "end": { "line": line, "character": column + KNOWN_WORD.len() }
            }
        })
    }

    fn handle(&mut self, message: Value) {
        let id = message.get("id").cloned().filter(|id| !id.is_null());
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // An answer to the one question this server asks.
            if id == Some(json!("practice-settings")) {
                self.notify("practice/configured", json!({ "answer": message.get("result") }));
            }
            return;
        };
        let text_uri = || params.pointer("/textDocument/uri").and_then(Value::as_str).map(str::to_string);

        match (method, id) {
            ("initialize", Some(id)) => {
                self.root = params.get("rootUri").and_then(Value::as_str).and_then(path_of);
                self.respond(
                    &id,
                    json!({
                        "capabilities": {
                            "positionEncoding": "utf-16",
                            "textDocumentSync": { "openClose": true, "change": 1, "save": true },
                            "completionProvider": { "triggerCharacters": ["."] },
                            "hoverProvider": true,
                            "definitionProvider": true,
                            "documentFormattingProvider": true
                        },
                        "serverInfo": { "name": "Practice language server", "version": env!("CARGO_PKG_VERSION") }
                    }),
                );
            }
            ("initialized", _) => {
                // Real servers ask for their settings at this point and wait for the answer.
                self.send(&json!({
                    "jsonrpc": "2.0",
                    "id": "practice-settings",
                    "method": "workspace/configuration",
                    "params": { "items": [{ "section": "practice" }] }
                }));
            }
            ("textDocument/didOpen", _) => {
                let text = params.pointer("/textDocument/text").and_then(Value::as_str).unwrap_or_default();
                if let Some(uri) = text_uri() {
                    self.took(&uri, text.to_string());
                }
            }
            ("textDocument/didChange", _) => {
                // Only whole texts are taken: that is what this server said it wants.
                let last = params.get("contentChanges").and_then(Value::as_array).and_then(|list| list.last());
                let text = last.and_then(|change| change.get("text")).and_then(Value::as_str);
                if let (Some(uri), Some(text)) = (text_uri(), text) {
                    self.took(&uri, text.to_string());
                }
            }
            ("textDocument/didClose", _) => {
                if let Some(uri) = text_uri() {
                    self.docs.remove(&uri);
                    // A closed file has no problems to show any more.
                    self.notify("textDocument/publishDiagnostics", json!({ "uri": uri, "diagnostics": [] }));
                }
            }
            ("textDocument/completion", Some(id)) => {
                let items: Vec<Value> = WORDS
                    .iter()
                    .map(|word| json!({ "label": word, "kind": 6, "detail": "practice word", "insertText": word }))
                    .collect();
                self.respond(&id, json!({ "isIncomplete": false, "items": items }));
            }
            ("textDocument/hover", Some(id)) => {
                let answer = self.hover(&params);
                self.respond(&id, answer);
            }
            ("textDocument/definition", Some(id)) => {
                let answer = self.definition(&params);
                self.respond(&id, answer);
            }
            ("textDocument/formatting", Some(id)) => {
                let text = text_uri().and_then(|uri| self.docs.get(&uri).cloned()).unwrap_or_default();
                if text.contains("BAD-FORMAT") {
                    self.respond_error(&id, -32803, "The practice server was asked to fail at formatting");
                    return;
                }
                if text.contains("SLOW-FORMAT") {
                    thread::sleep(Duration::from_secs(3));
                }
                self.respond(&id, json!(trailing_space_edits(&text)));
            }
            ("shutdown", Some(id)) => self.respond(&id, Value::Null),
            ("exit", _) => std::process::exit(0),
            (_, Some(id)) => self.respond_error(&id, -32601, &format!("Not supported: {method}")),
            // A notification this server has no use for.
            (_, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_read_from_its_header_and_body() {
        let mut input: &[u8] = b"Content-Type: x\r\ncontent-length: 2\r\n\r\n{}Content-Length: 4\r\n\r\nnull";
        assert_eq!(read_message(&mut input).unwrap(), b"{}");
        assert_eq!(read_message(&mut input).unwrap(), b"null");
        assert!(read_message(&mut input).is_none(), "the input is over");
    }

    #[test]
    fn every_line_with_fixme_gets_an_error_under_the_word() {
        let found = diagnostics_of("fine\n  // FIXME later\nпривет FIXME\n");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0]["range"]["start"], json!({ "line": 1, "character": 5 }));
        assert_eq!(found[0]["range"]["end"], json!({ "line": 1, "character": 10 }));
        // Columns count UTF-16 units, not bytes: six letters and a space come before the word.
        assert_eq!(found[1]["range"]["start"], json!({ "line": 2, "character": 7 }));
    }

    #[test]
    fn formatting_takes_the_spaces_off_the_ends_of_lines() {
        let edits = trailing_space_edits("a  \nb\n\tc\t \r\n");
        assert_eq!(edits.len(), 2);
        assert_eq!(
            edits[0]["range"],
            json!({ "start": { "line": 0, "character": 1 }, "end": { "line": 0, "character": 3 } })
        );
        assert_eq!(
            edits[1]["range"],
            json!({ "start": { "line": 2, "character": 2 }, "end": { "line": 2, "character": 4 } })
        );
        assert!(trailing_space_edits("clean\nlines\n").is_empty());
    }

    #[test]
    fn the_word_under_a_column_is_found() {
        assert_eq!(word_at("use limits here", 6), Some(("limits".into(), 4, 10)));
        assert_eq!(word_at("use limits here", 10), Some(("limits".into(), 4, 10)), "right after the word counts");
        assert_eq!(word_at("a  b", 2), None);
    }

    #[test]
    fn a_file_address_and_its_path_are_the_same_place() {
        let path = PathBuf::from("/tmp/a folder/файл.ts");
        let uri = uri_of(&path);
        assert!(uri.starts_with("file:///tmp/a%20folder/"), "{uri}");
        assert_eq!(path_of(&uri).unwrap(), path);
        assert_eq!(path_of("file:///C:/work/a.ts").unwrap(), PathBuf::from("C:/work/a.ts"));
        assert!(path_of("https://example.com/a.ts").is_none());
    }
}
