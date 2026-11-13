//! The practice agent as a program in a terminal: no protocol, only lines.
//!
//! This is what an agent without ACP looks like to the editor. It prints a
//! greeting and a prompt, reads a line, answers, and prints the prompt again.
//!
//! What a line does:
//! - always: the answer `You said: <the line>`;
//! - `WRITE <file> <text>` anywhere in the line: `<text>` (to the end of the
//!   line) is written to `<file>` in the folder the program was started in,
//!   and the answer says so;
//! - `/exit` alone on a line: the program ends.
//!
//! Switches, through the environment:
//! - `AE_MOCK_DELAY_MS`: how long it "thinks" before an answer. While it
//!   thinks it prints a dot every 50 ms, as a spinner would;
//! - `AE_MOCK_PASTE=1`: it asks the terminal for bracketed paste, as programs
//!   that take a message of several lines do, and reads a pasted message as one.

use std::io::{self, BufRead, Write};
use std::time::Duration;

/// A terminal marks pasted text with these when the program asked it to.
const PASTE_START: &str = "\u{1b}[200~";
const PASTE_END: &str = "\u{1b}[201~";

pub fn run() {
    let delay_ms: u64 = std::env::var("AE_MOCK_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let paste = std::env::var("AE_MOCK_PASTE").is_ok_and(|v| v == "1");
    let mut out = io::stdout();
    if paste {
        let _ = write!(out, "\u{1b}[?2004h");
    }
    let _ = writeln!(out, "Practice agent in a terminal. Say something.");
    prompt(&mut out);

    let stdin = io::stdin();
    let mut message = String::new();
    let mut pasting = false;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        // A pasted message may span several lines: it is one message, ended by the line after it.
        let mut rest = line.as_str();
        if let Some(at) = rest.find(PASTE_START) {
            pasting = true;
            rest = &rest[at + PASTE_START.len()..];
        }
        if pasting {
            match rest.find(PASTE_END) {
                Some(at) => {
                    message.push_str(&rest[..at]);
                    message.push_str(&rest[at + PASTE_END.len()..]);
                    pasting = false;
                }
                None => {
                    message.push_str(rest);
                    message.push('\n');
                    continue;
                }
            }
        } else {
            message.push_str(rest);
        }
        let said = std::mem::take(&mut message);
        let said = said.trim();
        if said.is_empty() {
            prompt(&mut out);
            continue;
        }
        if said == "/exit" {
            let _ = writeln!(out, "Bye.");
            break;
        }
        think(&mut out, delay_ms);
        if let Some(at) = said.find("WRITE ") {
            let order = said[at + "WRITE ".len()..].lines().next().unwrap_or("");
            let (file, text) = order.split_once(' ').unwrap_or((order, ""));
            match write_file(file, text) {
                Ok(()) => {
                    let _ = writeln!(out, "Wrote {file}.");
                }
                Err(error) => {
                    let _ = writeln!(out, "Could not write {file}: {error}");
                }
            }
        }
        let _ = writeln!(out, "You said: {}", said.replace('\n', " / "));
        prompt(&mut out);
    }
}

fn prompt(out: &mut io::Stdout) {
    let _ = write!(out, "> ");
    let _ = out.flush();
}

/// Prints a dot now and then for as long as the answer takes.
fn think(out: &mut io::Stdout, delay_ms: u64) {
    let mut left = delay_ms;
    while left > 0 {
        let step = left.min(50);
        std::thread::sleep(Duration::from_millis(step));
        left -= step;
        let _ = write!(out, ".");
        let _ = out.flush();
    }
    if delay_ms > 0 {
        let _ = writeln!(out);
    }
}

/// Writes inside the folder the program runs in, and nowhere else.
fn write_file(file: &str, text: &str) -> io::Result<()> {
    let path = std::path::Path::new(file);
    let outside = path.is_absolute() || path.components().any(|part| matches!(part, std::path::Component::ParentDir));
    if file.is_empty() || outside {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file of this folder"));
    }
    if let Some(folder) = path.parent().filter(|folder| !folder.as_os_str().is_empty()) {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(path, format!("{text}\n"))
}
