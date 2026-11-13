//! Terminals: real programs running in a task's folder, each inside a pseudo-terminal.
//!
//! A terminal is either the user's shell or one command, such as a project's
//! dev server. What the program prints is sent to the interface as raw bytes;
//! what the user types comes back the same way. The last screens of output are
//! kept, so an interface that connects later can show what is already there.

use anyhow::{Context, Result, anyhow};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// How much output of one terminal is kept for late joiners.
const SCROLLBACK_BYTES: usize = 256 * 1024;
/// How much recent text of a command is looked through for the address it serves on.
const SNIFF_CHARS: usize = 4096;

/// What a terminal is for. The interface shows each kind in its own place.
pub const SHELL: &str = "shell";

/// Something a terminal did.
#[derive(Debug, Clone)]
pub enum TermEvent {
    /// The program printed these bytes. `offset` counts every byte the terminal
    /// printed before them, so a listener can tell what it already has.
    Data { term_id: u32, offset: u64, bytes: Arc<[u8]> },
    /// The program ended.
    Exit { term_id: u32, task_id: String, purpose: String, code: u32 },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TermInfo {
    pub term_id: u32,
    pub task_id: String,
    pub title: String,
    /// "shell" for the user's terminal; "run:web", "logs:db" and the like for what the Run tab starts.
    pub purpose: String,
}

struct Term {
    task_id: String,
    title: String,
    purpose: String,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// Shared with what is remembered of an ended program, so its last words can still be read.
    kept: Arc<Mutex<Kept>>,
    /// The program's process, for asking the system what it uses.
    pid: Option<u32>,
    /// The address the program said it serves on, once it has said so.
    address: Mutex<Option<String>>,
    /// Set when the user stopped it on purpose: then how it ended is not worth remembering.
    stopped_on_purpose: std::sync::atomic::AtomicBool,
}

/// The tail of a terminal's output and how much it printed in all.
#[derive(Default)]
struct Kept {
    bytes: VecDeque<u8>,
    total: u64,
    /// The newest text without colour codes, for finding an address in it.
    plain: String,
}

/// How a program ended and what it printed up to then.
struct Ended {
    code: u32,
    kept: Arc<Mutex<Kept>>,
}

/// Owns every open terminal.
pub struct Terminals {
    next_id: AtomicU32,
    terms: Mutex<HashMap<u32, Arc<Term>>>,
    /// The last program of each (task, purpose) that ended by itself.
    ended: Mutex<HashMap<(String, String), Ended>>,
    /// Variables of the core's own environment that programs started here must not inherit.
    scrub: Vec<String>,
    events: broadcast::Sender<TermEvent>,
}

/// The shell to start: the user's own on Unix, PowerShell on Windows.
fn default_shell() -> String {
    if cfg!(windows) {
        for name in ["pwsh.exe", "powershell.exe"] {
            if let Ok(found) = which::which(name) {
                return found.to_string_lossy().into_owned();
            }
        }
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize { rows: rows.max(2), cols: cols.max(2), pixel_width: 0, pixel_height: 0 }
}

/// Removes the codes programs print to colour and move text, leaving the words.
pub fn strip_codes(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            plain.push(c);
            continue;
        }
        match chars.peek() {
            // "ESC [ … letter": the common kind. Everything up to the final letter belongs to it.
            Some('[') => {
                chars.next();
                for inner in chars.by_ref() {
                    if ('@'..='~').contains(&inner) {
                        break;
                    }
                }
            }
            // "ESC ] … BEL" sets a title or a link.
            Some(']') => {
                chars.next();
                while let Some(inner) = chars.next() {
                    if inner == '\u{7}' {
                        break;
                    }
                    if inner == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    plain
}

/// The first local web address in `text`, as `localhost:PORT`.
pub fn find_address(text: &str) -> Option<String> {
    for prefix in ["http://localhost:", "http://127.0.0.1:", "http://0.0.0.0:", "http://[::1]:", "https://localhost:"] {
        let mut from = 0;
        while let Some(at) = text[from..].find(prefix) {
            let digits: String = text[from + at + prefix.len()..].chars().take_while(char::is_ascii_digit).collect();
            if (2..=5).contains(&digits.len()) {
                return Some(format!("localhost:{digits}"));
            }
            from += at + prefix.len();
        }
    }
    None
}

impl Default for Terminals {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminals {
    pub fn new() -> Self {
        Self::without(vec![])
    }

    /// Terminals whose programs do not see the named variables. The core's own
    /// settings, such as the port it listens on, are meant for the core alone:
    /// a dev server that inherited `PORT` would try to listen on the core's port.
    pub fn without(scrub: Vec<String>) -> Self {
        let (events, _) = broadcast::channel(4096);
        Self {
            next_id: AtomicU32::new(1),
            terms: Mutex::new(HashMap::new()),
            ended: Mutex::new(HashMap::new()),
            scrub,
            events,
        }
    }

    fn scrub_env(&self, command: &mut CommandBuilder) {
        for name in &self.scrub {
            command.env_remove(name);
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TermEvent> {
        self.events.subscribe()
    }

    /// Starts the user's shell in `cwd`.
    pub fn open(self: &Arc<Self>, task_id: &str, cwd: &Path, cols: u16, rows: u16) -> Result<TermInfo> {
        let shell = default_shell();
        let mut command = CommandBuilder::new(&shell);
        // On macOS every terminal starts a login shell; that is where PATH is set up.
        if cfg!(target_os = "macos") {
            command.arg("-l");
        }
        self.scrub_env(&mut command);
        let title =
            Path::new(&shell).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "shell".into());
        self.start(task_id, cwd, command, &title, SHELL, cols, rows)
    }

    /// Runs one command line in `cwd`, the way a shell would. `env` is added to the environment.
    pub fn run(
        self: &Arc<Self>,
        task_id: &str,
        cwd: &Path,
        command_line: &str,
        purpose: &str,
        env: &[(String, String)],
    ) -> Result<TermInfo> {
        let mut command = if cfg!(windows) {
            let mut command = CommandBuilder::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()));
            command.args(["/C", command_line]);
            command
        } else {
            // A login shell, so tools installed by version managers are found as in the user's terminal.
            let mut command = CommandBuilder::new(default_shell());
            command.args(["-l", "-c", command_line]);
            command
        };
        self.scrub_env(&mut command);
        for (name, value) in env {
            command.env(name, value);
        }
        self.start(task_id, cwd, command, command_line, purpose, 120, 30)
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        self: &Arc<Self>,
        task_id: &str,
        cwd: &Path,
        mut command: CommandBuilder,
        title: &str,
        purpose: &str,
        cols: u16,
        rows: u16,
    ) -> Result<TermInfo> {
        if !cwd.is_dir() {
            return Err(anyhow!("The task's folder is gone: {}", cwd.display()));
        }
        let pair = native_pty_system().openpty(size(cols, rows)).context("Could not open a terminal")?;
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let mut child = pair.slave.spawn_command(command).with_context(|| format!("Could not start: {title}"))?;
        // The program holds its own end; ours is no longer needed.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("Could not read from the terminal")?;
        let writer = pair.master.take_writer().context("Could not write to the terminal")?;
        let term_id = self.next_id.fetch_add(1, Ordering::SeqCst);

        let term = Arc::new(Term {
            task_id: task_id.to_string(),
            title: title.to_string(),
            purpose: purpose.to_string(),
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            killer: Mutex::new(child.clone_killer()),
            kept: Arc::new(Mutex::new(Kept::default())),
            pid: child.process_id(),
            address: Mutex::new(None),
            stopped_on_purpose: std::sync::atomic::AtomicBool::new(false),
        });
        self.terms.lock().unwrap().insert(term_id, term.clone());
        self.ended.lock().unwrap().remove(&(task_id.to_string(), purpose.to_string()));

        // Reading a pseudo-terminal blocks, so it gets a thread of its own.
        let events = self.events.clone();
        let reading = term.clone();
        let sniff = purpose != SHELL;
        std::thread::Builder::new()
            .name(format!("term-{term_id}-read"))
            .spawn(move || {
                let mut buffer = [0u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let chunk = &buffer[..n];
                            // Sent while the lock is held, so the kept tail and the
                            // stream of events never disagree about the order.
                            let mut kept = reading.kept.lock().unwrap();
                            let offset = kept.total;
                            kept.bytes.extend(chunk.iter().copied());
                            kept.total += n as u64;
                            let extra = kept.bytes.len().saturating_sub(SCROLLBACK_BYTES);
                            kept.bytes.drain(..extra);
                            if sniff && reading.address.lock().unwrap().is_none() {
                                kept.plain.push_str(&strip_codes(&String::from_utf8_lossy(chunk)));
                                if let Some(address) = find_address(&kept.plain) {
                                    *reading.address.lock().unwrap() = Some(address);
                                }
                                let over = kept.plain.len().saturating_sub(SNIFF_CHARS);
                                if over > 0 {
                                    let cut = (over..=kept.plain.len())
                                        .find(|&i| kept.plain.is_char_boundary(i))
                                        .unwrap_or(0);
                                    kept.plain.drain(..cut);
                                }
                            }
                            let _ = events.send(TermEvent::Data { term_id, offset, bytes: Arc::from(chunk) });
                        }
                    }
                }
            })
            .context("Could not start the terminal reader")?;

        // Waiting for the program to end also blocks.
        let manager = self.clone();
        let watched = term.clone();
        let (task, why) = (task_id.to_string(), purpose.to_string());
        std::thread::Builder::new()
            .name(format!("term-{term_id}-wait"))
            .spawn(move || {
                let code = child.wait().map(|status| status.exit_code()).unwrap_or(1);
                // How it ended is known before the terminal is gone: whoever waits for
                // "gone" can then ask "how" without a gap in between.
                if !watched.stopped_on_purpose.load(Ordering::SeqCst) {
                    let ended = Ended { code, kept: watched.kept.clone() };
                    manager.ended.lock().unwrap().insert((task.clone(), why.clone()), ended);
                }
                manager.terms.lock().unwrap().remove(&term_id);
                let _ = manager.events.send(TermEvent::Exit { term_id, task_id: task, purpose: why, code });
            })
            .context("Could not watch the program")?;

        Ok(TermInfo { term_id, task_id: task_id.to_string(), title: title.to_string(), purpose: purpose.to_string() })
    }

    fn get(&self, term_id: u32) -> Result<Arc<Term>> {
        self.terms.lock().unwrap().get(&term_id).cloned().ok_or_else(|| anyhow!("This terminal has ended"))
    }

    /// Sends what the user typed to the program.
    pub fn input(&self, term_id: u32, data: &[u8]) -> Result<()> {
        let term = self.get(term_id)?;
        let mut writer = term.writer.lock().unwrap();
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, term_id: u32, cols: u16, rows: u16) -> Result<()> {
        let term = self.get(term_id)?;
        term.master.lock().unwrap().resize(size(cols, rows))?;
        Ok(())
    }

    /// Ends the program. The terminal disappears once the program is gone.
    pub fn close(&self, term_id: u32) -> Result<()> {
        let term = self.get(term_id)?;
        let _ = term.killer.lock().unwrap().kill();
        Ok(())
    }

    /// Ends a program the user asked to stop. Unlike a crash, this leaves no "it failed" behind.
    pub fn stop(&self, term_id: u32) -> Result<()> {
        let term = self.get(term_id)?;
        term.stopped_on_purpose.store(true, Ordering::SeqCst);
        let _ = term.killer.lock().unwrap().kill();
        Ok(())
    }

    fn info(id: u32, term: &Term) -> TermInfo {
        TermInfo {
            term_id: id,
            task_id: term.task_id.clone(),
            title: term.title.clone(),
            purpose: term.purpose.clone(),
        }
    }

    /// Every terminal of a task, oldest first.
    pub fn list(&self, task_id: &str) -> Vec<TermInfo> {
        let mut list: Vec<_> = self
            .terms
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, term)| term.task_id == task_id)
            .map(|(id, term)| Self::info(*id, term))
            .collect();
        list.sort_by_key(|info| info.term_id);
        list
    }

    /// The running terminal of a task that serves a purpose, if there is one.
    pub fn find(&self, task_id: &str, purpose: &str) -> Option<TermInfo> {
        self.list(task_id).into_iter().find(|info| info.purpose == purpose)
    }

    /// The address a running command said it serves on.
    pub fn address(&self, term_id: u32) -> Option<String> {
        self.get(term_id).ok()?.address.lock().unwrap().clone()
    }

    /// How the last program of a task with this purpose ended, if it has ended.
    pub fn last_exit(&self, task_id: &str, purpose: &str) -> Option<u32> {
        self.ended.lock().unwrap().get(&(task_id.to_string(), purpose.to_string())).map(|ended| ended.code)
    }

    /// What the last program of a task with this purpose printed before it ended
    /// by itself, without colour codes: the tail that was kept.
    pub fn last_output(&self, task_id: &str, purpose: &str) -> Option<String> {
        let key = (task_id.to_string(), purpose.to_string());
        let kept = self.ended.lock().unwrap().get(&key).map(|ended| ended.kept.clone())?;
        let bytes: Vec<u8> = kept.lock().unwrap().bytes.iter().copied().collect();
        Some(strip_codes(&String::from_utf8_lossy(&bytes)))
    }

    /// The process a terminal's program runs as.
    pub fn pid(&self, term_id: u32) -> Option<u32> {
        self.get(term_id).ok()?.pid
    }

    /// The output kept so far, for an interface that connects later, and how
    /// many bytes the terminal has printed in all. Events with a smaller
    /// offset are already part of what is returned.
    pub fn scrollback(&self, term_id: u32) -> Result<(Vec<u8>, u64)> {
        let term = self.get(term_id)?;
        let kept = term.kept.lock().unwrap();
        Ok((kept.bytes.iter().copied().collect(), kept.total))
    }

    /// The task a terminal belongs to.
    pub fn task_of(&self, term_id: u32) -> Result<String> {
        Ok(self.get(term_id)?.task_id.clone())
    }

    /// Ends every terminal of a task.
    pub fn close_task(&self, task_id: &str) {
        for info in self.list(task_id) {
            // Ended on purpose: how they end is not worth remembering.
            let _ = self.stop(info.term_id);
        }
        // What its earlier programs printed is of no use to anyone now.
        self.ended.lock().unwrap().retain(|(task, _), _| task != task_id);
    }

    /// Ends every terminal. Called when the core shuts down.
    pub fn close_all(&self) {
        let all: Vec<_> = self.terms.lock().unwrap().values().cloned().collect();
        for term in all {
            let _ = term.killer.lock().unwrap().kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_codes_are_removed_and_the_words_stay() {
        assert_eq!(
            strip_codes("\u{1b}[32m➜\u{1b}[0m  Local: http://localhost:\u{1b}[1m5173\u{1b}[22m/"),
            "➜  Local: http://localhost:5173/"
        );
        assert_eq!(strip_codes("\u{1b}]0;title\u{7}ready"), "ready");
        assert_eq!(strip_codes("plain"), "plain");
    }

    #[test]
    fn the_address_a_program_serves_on_is_found_in_what_it_prints() {
        assert_eq!(find_address("  ➜  Local:   http://localhost:5173/"), Some("localhost:5173".into()));
        assert_eq!(find_address("Listening on http://127.0.0.1:3000"), Some("localhost:3000".into()));
        assert_eq!(find_address("ready - started server on http://0.0.0.0:8080, url"), Some("localhost:8080".into()));
        // A bare host without a port, and a port that is not one, are not addresses to open.
        assert_eq!(find_address("see http://localhost:/docs and http://localhost:x"), None);
        assert_eq!(find_address("nothing here"), None);
    }
}
