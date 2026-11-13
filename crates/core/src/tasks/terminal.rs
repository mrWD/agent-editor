//! Agents that do not speak ACP: a program in a terminal of the task.
//!
//! Such an agent is started in a terminal in the task's folder, and a message
//! is typed into it the way the user would type it. The user sees that
//! terminal in place of the conversation and can type into it as well.
//!
//! The editor cannot read what such a program means, only that it prints. So
//! a turn is a guess:
//!
//! - it begins when a message is typed into the program, by the editor or by
//!   the user (a line they ended with Enter);
//! - it ends when the program has printed nothing for a while. A program that
//!   waits for an answer to a question of its own is quiet too, and "your
//!   turn" is the right thing to say then;
//! - a program that prints again by itself, without anyone typing, is at work
//!   again: the task says so, and nobody is told when it stops.
//!
//! What a turn changed on disk is the agent's, as with any agent. What the
//! protocol would carry is missing: no tool calls in the conversation, no
//! questions to answer there, no settings, no pictures, no cost, and none of
//! the editor's own tools. A program started afresh is told what was said
//! before, as one long message.

use super::{Manager, TaskInfo, TaskState, manual_edit_note, now_ms, recap, title_from};
use crate::agents::AgentDef;
use crate::git;
use crate::marks::Author;
use crate::store::{Project, TaskIssue, TaskRow};
use crate::terminals::{AGENT, TermEvent, Terminals};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// How long a program must print nothing before its turn is taken to be over,
/// unless the manager was told otherwise. `AE_TERMINAL_QUIET_MS` shortens it for tests.
fn quiet_by_default() -> Duration {
    let ms = std::env::var("AE_TERMINAL_QUIET_MS").ok().and_then(|text| text.parse::<u64>().ok()).unwrap_or(2500);
    Duration::from_millis(ms.max(50))
}

/// A program that prints nothing at all at its start is typed into after this long.
const SILENT_START: Duration = Duration::from_secs(3);
/// However long a program keeps printing at its start, the message is typed after this long.
const START_LIMIT: Duration = Duration::from_secs(30);
/// What a program prints this soon after the user typed or resized is the
/// echo of that, not work of its own.
const ECHO: Duration = Duration::from_secs(1);
/// How often the quiet is measured.
const TICK: Duration = Duration::from_millis(50);

/// What a program prints to ask its terminal for pasted text to be marked, and to stop asking.
const PASTE_ON: &[u8] = b"\x1b[?2004h";
const PASTE_OFF: &[u8] = b"\x1b[?2004l";

/// A turn of a program in a terminal, as far as the editor can tell.
struct Turn {
    /// The user's message that began it. `None` when the user typed into the
    /// terminal themselves, or the program began to print by itself.
    text: Option<String>,
    /// Its number among all turns, so its end is never taken for another's.
    number: u64,
    /// When the message was in the program's hands. `None` while the program is still starting.
    typed_at: Option<Instant>,
    /// Whether the conversation hears of its end. Not for a program that began to print by itself.
    said: bool,
    /// Set by "Stop this turn".
    cancelled: bool,
}

/// The running program of one task.
pub(super) struct Seat {
    /// The terminal it runs in.
    term_id: u32,
    /// The agent's name, for what is said about it.
    name: String,
    state: Mutex<TaskState>,
    turn: Mutex<Option<Turn>>,
    /// When the user last typed into the terminal or changed its size.
    touched: Mutex<Instant>,
    /// True once what the program printed at its start has stood still.
    ready: AtomicBool,
    /// True while the program wants pasted text marked: a message of several lines can then go in as one.
    paste: AtomicBool,
    /// The project's rules as this program was last told them.
    rules_told: Mutex<Option<String>>,
}

impl Seat {
    pub(super) fn state(&self) -> TaskState {
        *self.state.lock().unwrap()
    }

    /// What the interface needs to show the program: no settings, no commands, and which terminal it is.
    pub(super) fn config(&self) -> Value {
        json!({ "options": [], "commands": [], "images": false, "terminal": self.term_id })
    }
}

/// A message as keystrokes. Nothing in it may act as a key, so control
/// characters become spaces. A program that marks pasted text gets the message
/// whole; any other gets it as one line, for to it every line end means "send".
fn as_typed(text: &str, paste: bool) -> Vec<u8> {
    let clean: String = text
        .chars()
        .map(|letter| match letter {
            '\n' => '\n',
            letter if letter.is_control() => ' ',
            letter => letter,
        })
        .collect();
    if paste {
        format!("\x1b[200~{}\x1b[201~", clean.trim_end()).into_bytes()
    } else {
        clean.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join(" ").into_bytes()
    }
}

/// Whether the program now wants pasted text marked, going by what it just
/// printed. `before` is the end of what it printed earlier: a request may be
/// cut in two.
fn paste_wish(before: &[u8], bytes: &[u8]) -> Option<bool> {
    let joined = [before, bytes].concat();
    let last = |needle: &[u8]| joined.windows(needle.len()).rposition(|window| window == needle);
    match (last(PASTE_ON), last(PASTE_OFF)) {
        (Some(on), Some(off)) => Some(on > off),
        (Some(_), None) => Some(true),
        (None, Some(_)) => Some(false),
        (None, None) => None,
    }
}

/// The last thing a program printed, cut to fit a sentence.
fn last_words(said: &str) -> Option<String> {
    let line = said.lines().map(str::trim).rfind(|line| !line.is_empty())?;
    let mut words: String = line.chars().take(200).collect();
    if line.chars().count() > 200 {
        words.push('…');
    }
    Some(words)
}

impl Manager {
    /// How long a program in a terminal must print nothing before its turn is taken to be over.
    fn quiet(&self) -> Duration {
        match self.terminal_quiet_ms.load(Ordering::SeqCst) {
            0 => quiet_by_default(),
            ms => Duration::from_millis(ms),
        }
    }

    /// A program that has just started is ready for a message once what it
    /// printed first has stood still for this long.
    fn settled(&self) -> Duration {
        self.quiet().min(Duration::from_millis(600))
    }

    /// Sets how long a program in a terminal must be quiet for its turn to be
    /// over. Tests shorten it; nothing else needs to.
    pub fn set_terminal_quiet(&self, quiet: Duration) {
        self.terminal_quiet_ms.store((quiet.as_millis() as u64).max(50), Ordering::SeqCst);
    }

    /// Gives the task manager the terminals. Without them an agent that runs in a terminal cannot be started.
    pub fn lend_terminals(&self, terminals: Arc<Terminals>) {
        let _ = self.terminals.set(terminals);
    }

    fn terminals(&self) -> Result<Arc<Terminals>> {
        self.terminals.get().cloned().ok_or_else(|| anyhow!("This core has no terminals to run an agent in"))
    }

    pub(super) fn seat_of(&self, task_id: &str) -> Option<Arc<Seat>> {
        self.seated.read().unwrap().get(task_id).cloned()
    }

    fn seat_in(&self, term_id: u32) -> Option<(String, Arc<Seat>)> {
        let seated = self.seated.read().unwrap();
        seated.iter().find(|(_, seat)| seat.term_id == term_id).map(|(task, seat)| (task.clone(), seat.clone()))
    }

    /// Whether the task's agent is, or would be, a program in a terminal.
    pub(super) fn in_terminal(&self, row: &TaskRow) -> bool {
        self.seat_of(&row.id).is_some()
            || self.registry().get(&row.agent_id).is_some_and(|agent| agent.speaks == crate::agents::Speaks::Terminal)
    }

    fn seat_state(&self, task_id: &str, seat: &Seat, state: TaskState) {
        {
            let mut current = seat.state.lock().unwrap();
            if *current == state {
                return;
            }
            *current = state;
        }
        self.emit(task_id, json!({ "type": "state", "state": state }));
    }

    /// Starts the agent's program in a terminal in `cwd`. What the terminal
    /// does is listened to from before the start, so nothing it prints is missed.
    fn start_seat(
        &self,
        task_id: &str,
        agent: &AgentDef,
        cwd: &Path,
    ) -> Result<(Arc<Seat>, broadcast::Receiver<TermEvent>)> {
        let terminals = self.terminals()?;
        let events = terminals.subscribe();
        let info = terminals
            .program(task_id, cwd, &agent.program, &agent.args, &agent.env, &agent.name, AGENT)
            .map_err(|error| anyhow!("{} did not start: {error:#}", agent.name))?;
        let seat = Arc::new(Seat {
            term_id: info.term_id,
            name: agent.name.clone(),
            state: Mutex::new(TaskState::Idle),
            turn: Mutex::new(None),
            touched: Mutex::new(Instant::now()),
            ready: AtomicBool::new(false),
            paste: AtomicBool::new(false),
            rules_told: Mutex::new(None),
        });
        Ok((seat, events))
    }

    /// Makes a started program the task's agent and begins watching its terminal.
    fn take_seat(
        self: &Arc<Self>,
        task_id: &str,
        agent: &AgentDef,
        seat: &Arc<Seat>,
        events: broadcast::Receiver<TermEvent>,
        resumed: bool,
    ) {
        self.seated.write().unwrap().insert(task_id.to_string(), seat.clone());
        self.emit(
            task_id,
            json!({
                "type": "agent_started",
                "agentId": agent.id,
                "resumed": resumed,
                // It never picks a session up: what was said before is typed to it again.
                "loaded": false,
                "terminal": true,
                "agentInfo": Value::Null
            }),
        );
        self.broadcast("task.config", json!({ "taskId": task_id, "config": seat.config() }));
        let (manager, id, seat) = (self.clone(), task_id.to_string(), seat.clone());
        tokio::spawn(async move { manager.watch_seat(id, seat, events).await });
    }

    /// Creates a task whose agent runs in a terminal, and types the first message to it.
    pub(super) async fn create_seated(
        self: &Arc<Self>,
        project: Project,
        agent: AgentDef,
        text: &str,
        issue: Option<TaskIssue>,
    ) -> Result<TaskInfo> {
        let id = uuid::Uuid::new_v4().to_string();
        let title = match &issue {
            Some(issue) => title_from(&issue.title),
            None => title_from(text),
        };
        let (cwd, branch, base_branch) = self.prepare_folder(&project, &id, &title).await?;
        let (seat, events) = match self.start_seat(&id, &agent, &cwd) {
            Ok(started) => started,
            Err(error) => {
                // Leave nothing behind for a task that never existed.
                if let Some(branch) = &branch {
                    git::remove_worktree(Path::new(&project.path), &cwd, branch).await;
                }
                return Err(error);
            }
        };
        let row = TaskRow {
            id: id.clone(),
            project_id: project.id.clone(),
            title,
            agent_id: agent.id.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            branch,
            base_branch,
            issue,
            agent_session: None,
            created_at: now_ms(),
            seen_seq: -1,
        };
        self.store.insert_task(&row)?;
        self.broadcast(
            "task.created",
            json!(TaskInfo { row: row.clone(), state: TaskState::Idle, config: Some(seat.config()), race: None }),
        );
        self.take_seat(&id, &agent, &seat, events, false);
        self.prompt_seated(row, text.to_string(), None).await?;
        self.task(&id)
    }

    /// Types a message to the task's program, starting the program first when the task is asleep.
    pub(super) async fn prompt_seated(
        self: &Arc<Self>,
        row: TaskRow,
        text: String,
        shown: Option<Value>,
    ) -> Result<()> {
        let task_id = row.id.as_str();
        let (seat, fresh) = match self.seat_of(task_id) {
            Some(seat) => (seat, false),
            None => {
                let agent = self.agent(&row.agent_id)?;
                let (seat, events) = self.start_seat(task_id, &agent, Path::new(&row.cwd))?;
                self.take_seat(task_id, &agent, &seat, events, true);
                (seat, true)
            }
        };
        {
            // A program that began to print by itself does not keep the user from speaking.
            let turn = seat.turn.lock().unwrap();
            let busy = seat.state() != TaskState::Idle && turn.as_ref().is_none_or(|turn| turn.said);
            if busy {
                bail!("The agent is still working. Wait for it or stop the turn.");
            }
        }

        // The code as the user leaves it for this turn: a point to look back at and rewind to.
        {
            let (manager, id, said) = (self.clone(), task_id.to_string(), text.clone());
            let _ = tokio::task::spawn_blocking(move || manager.checkpoint_before_message(&id, &said)).await;
        }
        // Whatever changed on disk before this turn is not the agent's doing.
        self.sweep_in_background(task_id, Author::You, vec![]).await;
        let edited: Vec<String> =
            self.manual.lock().unwrap().remove(task_id).map(|set| set.into_iter().collect()).unwrap_or_default();

        // A program started afresh knows nothing of the task: tell it what was said before.
        let mut sent = match fresh.then(|| self.store.events(task_id).ok().and_then(|events| recap(&events))).flatten()
        {
            Some(before) => format!(
                "This task continues an earlier conversation. What was said before:\n\n{before}\n\nNow the user says:\n\n{text}"
            ),
            None => text.clone(),
        };
        if !edited.is_empty() {
            sent = format!("{}\n\n{sent}", manual_edit_note(&edited));
        }
        if let Some(note) = self.plan_note(task_id) {
            sent = format!("{note}\n\n{sent}");
        }
        let rules = self.rules_note(&row.project_id, &row.agent_id);
        {
            let mut told = seat.rules_told.lock().unwrap();
            if *told != rules {
                if let Some(note) = &rules {
                    sent = format!("{note}\n\n{sent}");
                }
                *told = rules;
            }
        }

        match shown {
            Some(event) => self.emit(task_id, event),
            None => self.emit(task_id, json!({ "type": "user_message", "text": text, "edited": edited })),
        }
        *seat.turn.lock().unwrap() = Some(Turn {
            text: Some(text),
            number: crate::plan::turn_number(),
            typed_at: None,
            said: true,
            cancelled: false,
        });
        self.seat_state(task_id, &seat, TaskState::Running);

        let (manager, task_id) = (self.clone(), task_id.to_string());
        tokio::spawn(async move {
            // The program reads once it has printed what it starts with. Typed
            // earlier, a message may be thrown away as the program sets its terminal up.
            let asked = Instant::now();
            while !seat.ready.load(Ordering::SeqCst) && asked.elapsed() < START_LIMIT {
                tokio::time::sleep(TICK / 2).await;
            }
            let still_there = manager.seat_of(&task_id).is_some_and(|current| Arc::ptr_eq(&current, &seat));
            let Ok(terminals) = manager.terminals() else { return };
            // A program that has ended is told about by whoever watches its terminal.
            if !still_there
                || terminals.input(seat.term_id, &as_typed(&sent, seat.paste.load(Ordering::SeqCst))).is_err()
            {
                return;
            }
            // Enter comes a moment later: a program that takes pasted text needs to have taken it in.
            tokio::time::sleep(manager.settled() / 5).await;
            let _ = terminals.input(seat.term_id, b"\r");
            if let Some(turn) = seat.turn.lock().unwrap().as_mut() {
                turn.typed_at = Some(Instant::now());
            }
        });
        Ok(())
    }

    /// "Stop this turn": the key that stops what a program is doing, in every terminal.
    pub(super) fn cancel_seated(&self, seat: &Seat) -> Result<()> {
        if let Some(turn) = seat.turn.lock().unwrap().as_mut() {
            turn.cancelled = true;
        }
        self.terminals()?.input(seat.term_id, b"\x03")
    }

    /// Ends the program. Its terminal goes with it, and the task falls asleep.
    pub(super) fn stop_seated(&self, seat: &Seat) {
        if let Ok(terminals) = self.terminals() {
            let _ = terminals.stop(seat.term_id);
        }
    }

    /// Ends every program that runs in a terminal. Called when the core shuts down.
    pub(super) fn stop_all_seated(&self) {
        let all: Vec<Arc<Seat>> = self.seated.read().unwrap().values().cloned().collect();
        for seat in all {
            self.stop_seated(&seat);
        }
    }

    /// The user typed into a terminal. In an agent's terminal, a line they
    /// ended is a message of theirs: the agent has the turn from then on.
    pub fn typed_into(self: &Arc<Self>, term_id: u32, bytes: &[u8]) {
        let Some((task_id, seat)) = self.seat_in(term_id) else { return };
        *seat.touched.lock().unwrap() = Instant::now();
        let sent = bytes.iter().any(|byte| *byte == b'\r' || *byte == b'\n');
        if !sent || !seat.ready.load(Ordering::SeqCst) {
            return;
        }
        {
            let mut turn = seat.turn.lock().unwrap();
            match turn.as_mut() {
                // Their answer to something the program asked in the middle of a turn belongs to that turn.
                Some(turn) => {
                    turn.typed_at = turn.typed_at.map(|_| Instant::now());
                    return;
                }
                None => {
                    *turn = Some(Turn {
                        text: None,
                        number: crate::plan::turn_number(),
                        typed_at: Some(Instant::now()),
                        said: true,
                        cancelled: false,
                    });
                }
            }
        }
        self.seat_state(&task_id, &seat, TaskState::Running);
        // Whatever changed on disk before this line was sent is not the agent's doing.
        let manager = self.clone();
        tokio::spawn(async move { manager.sweep_in_background(&task_id, Author::You, vec![]).await });
    }

    /// The size of a terminal changed. A program redraws itself then, which is not work.
    pub fn terminal_resized(&self, term_id: u32) {
        if let Some((_, seat)) = self.seat_in(term_id) {
            *seat.touched.lock().unwrap() = Instant::now();
        }
    }

    /// Follows what the program prints until it ends: when it is ready for a
    /// message, when its turn is over, and when it has gone.
    async fn watch_seat(self: Arc<Self>, task_id: String, seat: Arc<Seat>, mut events: broadcast::Receiver<TermEvent>) {
        let started = Instant::now();
        let mut last_output: Option<Instant> = None;
        let mut tail: Vec<u8> = Vec::new();
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // How the program ended: its code, and whether it was ended on purpose.
        let ended: Option<(u32, bool)> = loop {
            tokio::select! {
                event = events.recv() => match event {
                    Ok(TermEvent::Data { term_id, bytes, .. }) if term_id == seat.term_id => {
                        last_output = Some(Instant::now());
                        if let Some(wanted) = paste_wish(&tail, &bytes) {
                            seat.paste.store(wanted, Ordering::SeqCst);
                        }
                        let keep = bytes.len().min(PASTE_ON.len() - 1);
                        tail = bytes[bytes.len() - keep..].to_vec();
                        self.printed_by_itself(&task_id, &seat);
                    }
                    Ok(TermEvent::Exit { term_id, code, closed, .. }) if term_id == seat.term_id => break Some((code, closed)),
                    Ok(_) => {}
                    // So much was printed that some of it was missed here: it printed, and that is all that matters.
                    Err(broadcast::error::RecvError::Lagged(_)) => last_output = Some(Instant::now()),
                    Err(broadcast::error::RecvError::Closed) => break None,
                },
                _ = tick.tick() => {
                    if !seat.ready.load(Ordering::SeqCst) {
                        let stood_still = match last_output {
                            Some(at) => at.elapsed() >= self.settled(),
                            None => started.elapsed() >= SILENT_START,
                        };
                        if stood_still {
                            seat.ready.store(true, Ordering::SeqCst);
                        }
                    }
                    let over = seat.turn.lock().unwrap().as_ref().and_then(|turn| turn.typed_at).is_some_and(|typed| {
                        last_output.map_or(typed, |printed| printed.max(typed)).elapsed() >= self.quiet()
                    });
                    if over {
                        self.end_seated_turn(&task_id, &seat).await;
                    }
                }
            }
        };

        // The program is gone. Forget it only if it is still the task's current agent.
        let was_current = {
            let mut seated = self.seated.write().unwrap();
            match seated.get(&task_id) {
                Some(current) if Arc::ptr_eq(current, &seat) => {
                    seated.remove(&task_id);
                    true
                }
                _ => false,
            }
        };
        if !was_current {
            return;
        }
        let turn = seat.turn.lock().unwrap().take();
        let (code, closed) = ended.unwrap_or((0, true));
        let name = &seat.name;
        let stopped = closed || turn.as_ref().is_some_and(|turn| turn.cancelled);
        if turn.as_ref().is_some_and(|turn| turn.text.is_some() && turn.typed_at.is_none()) {
            self.emit(
                &task_id,
                json!({ "type": "error", "message": format!("{name} ended before it took your message.") }),
            );
        } else if !stopped && code != 0 {
            let said = self.terminals().ok().and_then(|terminals| terminals.last_output(&task_id, AGENT));
            let words = said.as_deref().and_then(last_words).map(|words| format!(" Its last words: {words}"));
            self.emit(
                &task_id,
                json!({ "type": "error", "message": format!("{name} ended by itself with code {code}.{}", words.unwrap_or_default()) }),
            );
        }
        if let Some(turn) = turn {
            // What it changed before it went is still its doing.
            self.sweep_in_background(&task_id, Author::Agent, vec![]).await;
            if let Some(text) = turn.text {
                self.plan_turn_ended(&task_id, &text, turn.number, false).await;
            }
        }
        *seat.state.lock().unwrap() = TaskState::Asleep;
        self.emit(&task_id, json!({ "type": "state", "state": TaskState::Asleep }));
    }

    /// The program printed while nobody had given it anything to do: it is at
    /// work again, by itself. What it prints right after the user typed, or
    /// after its terminal changed size, is not work.
    fn printed_by_itself(&self, task_id: &str, seat: &Seat) {
        if !seat.ready.load(Ordering::SeqCst) || seat.touched.lock().unwrap().elapsed() < ECHO {
            return;
        }
        {
            let mut turn = seat.turn.lock().unwrap();
            if turn.is_some() {
                return;
            }
            *turn = Some(Turn {
                text: None,
                number: crate::plan::turn_number(),
                typed_at: Some(Instant::now()),
                said: false,
                cancelled: false,
            });
        }
        self.seat_state(task_id, seat, TaskState::Running);
    }

    /// The program has gone quiet: its turn is over, as far as anyone can tell.
    async fn end_seated_turn(self: &Arc<Self>, task_id: &str, seat: &Seat) {
        let Some(turn) = seat.turn.lock().unwrap().take() else { return };
        if turn.said {
            let stop = if turn.cancelled { "cancelled" } else { "end_turn" };
            // `guessed`: the program did not say so, it only stopped printing.
            self.emit(task_id, json!({ "type": "turn_end", "stopReason": stop, "guessed": true }));
        }
        // Everything that changed during the turn is the agent's.
        self.sweep_in_background(task_id, Author::Agent, vec![]).await;
        if let Some(text) = &turn.text {
            self.plan_turn_ended(task_id, text, turn.number, !turn.cancelled).await;
        }
        if seat.state() != TaskState::Asleep {
            self.seat_state(task_id, seat, TaskState::Idle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_of_several_lines_is_one_line_for_a_program_that_reads_lines() {
        let typed = as_typed("First line.\n\n  Second line.\r\nThird\tline.\n", false);
        assert_eq!(String::from_utf8(typed).unwrap(), "First line. Second line. Third line.");
    }

    #[test]
    fn a_program_that_marks_pasted_text_gets_the_message_whole() {
        let typed = as_typed("First line.\nSecond line.\n", true);
        assert_eq!(String::from_utf8(typed).unwrap(), "\u{1b}[200~First line.\nSecond line.\u{1b}[201~");
    }

    #[test]
    fn nothing_in_a_message_acts_as_a_key() {
        // An escape in the text would end the paste early, or be read as a key; Ctrl+C would stop the program.
        let typed = String::from_utf8(as_typed("stop\u{3} here\u{1b}[201~rm -rf", true)).unwrap();
        assert_eq!(typed, "\u{1b}[200~stop  here [201~rm -rf\u{1b}[201~");
        assert!(!as_typed("a\u{1b}b\u{3}c", false).iter().any(|byte| *byte < 0x20));
    }

    #[test]
    fn the_wish_for_marked_paste_is_read_even_when_cut_in_two() {
        assert_eq!(paste_wish(b"", b"hello \x1b[?2004h"), Some(true));
        assert_eq!(paste_wish(b"", b"\x1b[?2004h then \x1b[?2004l"), Some(false));
        assert_eq!(paste_wish(b"\x1b[?20", b"04h"), Some(true));
        assert_eq!(paste_wish(b"", b"nothing of the kind"), None);
    }

    #[test]
    fn the_last_words_are_the_last_line_that_says_something() {
        assert_eq!(last_words("one\ntwo  \n\n   \n").as_deref(), Some("two"));
        assert_eq!(last_words(" \n"), None);
    }
}
