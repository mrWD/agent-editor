//! The plan of a task: what is to be built, block by block.
//!
//! Map shows the code as it came out; the plan shows what the task is meant
//! to become. It is a flow chart: steps, questions and notes, with arrows
//! between them. The user draws it or lets the task's agent draft it, and the
//! agent then builds it one block at a time, when the user says so.
//!
//! One plan per task, kept as one setting (`task.<id>.plan`), so it goes away
//! with the task's project like everything else of the task.
//!
//! Agents read and write plans in the flow format of the Map view (see
//! `ui/src/core/flow.ts`). The same format is written and read here, by the
//! same rules and with the same words for what is wrong.
//!
//! A block is built by one turn of the agent: the core sends a message that
//! holds the plan and names the one block, and when that turn ends well the
//! block is built, with the files the turn changed. While "Agents build only
//! what is in the plan" is on, every other message carries the plan too, and
//! an agent that misses something ends its answer with `PROPOSE: <text>`
//! lines. Those become blocks that wait for the user's yes.

use crate::gitops;
use crate::review;
use crate::store::TaskRow;
use crate::tasks::{Manager, TaskState, now_ms};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

// ----- the flow format -----

/// The limits of a flow an agent answers with: the same as in `flow.ts`.
const MAX_NODES: usize = 30;
const MAX_TEXT: usize = 60;
const MAX_LABEL: usize = 12;

/// Where in the code something is: a file of the task, and a line when it is known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub path: String,
    #[serde(default)]
    pub line: Option<u32>,
    /// The last line, when the place is a stretch of lines and not one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<u32>,
}

/// One step or question of a flow, as the format writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowNode {
    pub id: String,
    /// A question (`ask`), not a step.
    pub ask: bool,
    pub text: String,
    /// The `*` after the word. In a plan it marks a block that is built already.
    pub changed: bool,
    pub place: Option<Place>,
}

/// An arrow between two blocks, with the short label an answer of a question has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrow {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Flow {
    pub nodes: Vec<FlowNode>,
    pub arrows: Vec<Arrow>,
}

/// Writes a flow as one block in the flow format, the way an agent is shown it.
pub fn write_flow(flow: &Flow) -> String {
    let mut lines = vec!["```flow".to_string()];
    for node in &flow.nodes {
        let word = if node.ask { "ask" } else { "step" };
        let star = if node.changed { "*" } else { "" };
        // A place is written only when it can be read back: a line, and a path without spaces.
        let place = match &node.place {
            Some(Place { path, line: Some(line), .. }) if !path.contains(char::is_whitespace) => {
                format!(" @ {path}:{line}")
            }
            _ => String::new(),
        };
        lines.push(format!("{word}{star} {}: {}{place}", node.id, node.text));
    }
    for arrow in &flow.arrows {
        lines.push(match &arrow.label {
            Some(label) => format!("{} -> {}: {label}", arrow.from, arrow.to),
            None => format!("{} -> {}", arrow.from, arrow.to),
        });
    }
    lines.push("```".to_string());
    lines.join("\n")
}

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Splits an id off the front of a text: 1 to 12 letters, digits or underscores.
fn take_id(text: &str) -> Option<(&str, &str)> {
    let end = text.find(|c: char| !is_id_char(c)).unwrap_or(text.len());
    (1..=12).contains(&end).then(|| text.split_at(end))
}

/// Reads `step <id>: <text>` or `ask <id>: <text>`, with an optional `*` after the word.
fn node_line(line: &str) -> Option<(bool, bool, &str, &str)> {
    let (ask, rest) = match (line.strip_prefix("step"), line.strip_prefix("ask")) {
        (Some(rest), _) => (false, rest),
        (_, Some(rest)) => (true, rest),
        _ => return None,
    };
    let (changed, rest) = match rest.strip_prefix('*') {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    // The word and the id are apart: "stepa: …" is not a step.
    let after = rest.trim_start();
    if after.len() == rest.len() {
        return None;
    }
    let (id, rest) = take_id(after)?;
    let text = rest.trim_start().strip_prefix(':')?.trim_start();
    (!text.is_empty()).then_some((ask, changed, id, text))
}

/// Reads `<from> -> <to>` or `<from> -> <to>: <label>`.
fn arrow_line(line: &str) -> Option<(&str, &str, Option<&str>)> {
    let (from, rest) = take_id(line)?;
    let rest = rest.trim_start().strip_prefix("->")?.trim_start();
    let (to, rest) = take_id(rest)?;
    if rest.is_empty() {
        return Some((from, to, None));
    }
    let label = rest.trim_start().strip_prefix(':')?.trim();
    (!label.is_empty()).then_some((from, to, Some(label)))
}

/// Splits ` @ <path>:<line>` off the end of a node's text, when it is there.
fn split_place(rest: &str) -> (&str, Option<Place>) {
    for (at, _) in rest.rmatch_indices('@') {
        let (head, tail) = (&rest[..at], &rest[at + 1..]);
        let apart = head.ends_with(char::is_whitespace) && tail.starts_with(char::is_whitespace);
        let (head, tail) = (head.trim_end(), tail.trim_start());
        if !apart || head.is_empty() || tail.contains(char::is_whitespace) {
            continue;
        }
        if let Some((path, line)) = tail.rsplit_once(':')
            && !path.is_empty()
            && !line.is_empty()
            && line.bytes().all(|byte| byte.is_ascii_digit())
            && let Ok(line) = line.parse()
        {
            return (head, Some(Place { path: path.to_string(), line: Some(line), to: None }));
        }
    }
    (rest, None)
}

/// Reads the flow out of an agent's answer, or says in plain words what is wrong with it.
pub fn parse_flow(answer: &str) -> Result<Flow, String> {
    let lines: Vec<&str> = answer.lines().collect();
    let Some(open) = lines.iter().position(|line| line.trim() == "```flow") else {
        return Err("The answer has no flow block.".into());
    };
    let Some(length) = lines[open + 1..].iter().position(|line| line.trim() == "```") else {
        return Err("The flow block is not closed.".into());
    };

    let mut flow = Flow::default();
    for (index, raw) in lines[open + 1..open + 1 + length].iter().enumerate() {
        let line = raw.trim();
        // Counted inside the block, as the agent wrote it.
        let at = format!("Line {} of the flow", index + 1);
        if line.is_empty() {
            continue;
        }
        if let Some((ask, changed, id, rest)) = node_line(line) {
            let (text, place) = split_place(rest);
            if text.chars().count() > MAX_TEXT {
                return Err(format!("{at} has more than {MAX_TEXT} characters of text."));
            }
            if flow.nodes.iter().any(|known| known.id == id) {
                return Err(format!("{at} uses the id \"{id}\" a second time."));
            }
            flow.nodes.push(FlowNode { id: id.to_string(), ask, text: text.to_string(), changed, place });
            continue;
        }
        if let Some((from, to, label)) = arrow_line(line) {
            if label.is_some_and(|label| label.chars().count() > MAX_LABEL) {
                return Err(format!("{at} has a label of more than {MAX_LABEL} characters."));
            }
            flow.arrows.push(Arrow { from: from.to_string(), to: to.to_string(), label: label.map(str::to_string) });
            continue;
        }
        let shown: String = line.chars().take(60).collect();
        return Err(format!("{at} is not a step, a question or an arrow: \"{shown}\"."));
    }

    if flow.nodes.len() < 2 {
        return Err("A flow needs at least two steps.".into());
    }
    if flow.nodes.len() > MAX_NODES {
        return Err(format!("The flow has more than {MAX_NODES} steps."));
    }
    let known = |id: &str| flow.nodes.iter().any(|node| node.id == id);
    for arrow in &flow.arrows {
        if let Some(missing) = [&arrow.from, &arrow.to].into_iter().find(|id| !known(id)) {
            return Err(format!("An arrow names \"{missing}\", which is not a step of the flow."));
        }
        if arrow.from == arrow.to {
            return Err(format!("The step \"{}\" has an arrow to itself.", arrow.from));
        }
    }
    for (index, node) in flow.nodes.iter().enumerate() {
        let out: Vec<&Arrow> = flow.arrows.iter().filter(|arrow| arrow.from == node.id).collect();
        if !node.ask && out.len() > 1 {
            return Err(format!(
                "The step \"{}\" leads to {} places. Only a question may do that.",
                node.id,
                out.len()
            ));
        }
        if node.ask && out.len() < 2 {
            return Err(format!("The question \"{}\" needs at least two answers.", node.id));
        }
        if node.ask && out.iter().any(|arrow| arrow.label.is_none()) {
            return Err(format!("An answer to the question \"{}\" has no label.", node.id));
        }
        if index > 0 && !flow.arrows.iter().any(|arrow| arrow.to == node.id) {
            return Err(format!("No arrow leads to the step \"{}\".", node.id));
        }
    }
    Ok(flow)
}

// ----- the plan -----

/// A plan is a picture a person reads, so it stays small.
const MAX_BLOCKS: usize = 60;
/// A note is not written in the flow format, so it may say more than a block's name.
const MAX_NOTE: usize = 200;
const MAX_DETAILS: usize = 1000;
/// How many blocks one answer may propose. More than a few is a new plan, not a missing block.
const MAX_PROPOSALS: usize = 5;
/// How many files a built block lists.
const MAX_FILES: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Something that happens.
    Step,
    /// A question with labelled answers, drawn as a diamond.
    Ask,
    /// A remark on the plan. It is not built and has no arrows.
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// In the plan, not built yet.
    Planned,
    /// The agent is building it right now.
    Building,
    Built,
    /// An agent thinks it is missing. Not in the plan until the user says yes.
    Proposed,
    /// The user took it for themselves. Agents leave it alone.
    Yours,
    /// It was in the code before the task: drawn from the code, nothing to build.
    Before,
}

/// What is done as far as the blocks after it are concerned.
fn settled(state: State) -> bool {
    matches!(state, State::Built | State::Yours | State::Before)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    /// Short and fit for the flow format: "b1", "b2" and so on.
    pub id: String,
    pub kind: Kind,
    /// The name on the block: one short line.
    pub text: String,
    /// "What this block should do", when the name does not say enough.
    #[serde(default)]
    pub details: String,
    pub state: State,
    /// The agent that built or proposed it, and its name at that time.
    #[serde(default)]
    pub by: Option<String>,
    #[serde(default)]
    pub by_name: Option<String>,
    /// When it was built or proposed.
    #[serde(default)]
    pub at: Option<u64>,
    /// For a built block: the files that turn changed. For one drawn from the code: where it is.
    #[serde(default)]
    pub files: Vec<Place>,
    /// Where a block an agent wrote came from: "draft", "build" or "code". `None` for the user's own.
    #[serde(default)]
    pub from: Option<String>,
    /// How the block read when it was last built, so a later build can be told what changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_from: Option<String>,
    /// The tests the agent named as proof that the built block works.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proved: Vec<String>,
}

impl Block {
    fn new(id: String, kind: Kind, text: String, state: State) -> Self {
        Self {
            id,
            kind,
            text,
            details: String::new(),
            state,
            by: None,
            by_name: None,
            at: None,
            files: Vec::new(),
            from: None,
            built_from: None,
            proved: Vec::new(),
        }
    }

    /// Everything the block says, in one piece.
    fn wording(&self) -> String {
        if self.details.is_empty() { self.text.clone() } else { format!("{}. {}", self.text, self.details) }
    }

    /// Forgets who built it and where.
    fn unbuild(&mut self, state: State) {
        self.state = state;
        self.by = None;
        self.by_name = None;
        self.at = None;
        self.files.clear();
        self.proved.clear();
    }
}

/// Who wrote a block an agent added, and when.
#[derive(Debug, Clone)]
pub struct Author {
    pub id: String,
    pub name: String,
    pub at: u64,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub arrows: Vec<Arrow>,
    /// "Agents build only what is in the plan."
    #[serde(default = "yes")]
    pub only: bool,
    /// Why the agent's last draft could not be used, in plain words.
    #[serde(default)]
    pub problem: Option<String>,
    /// The number the last new block got in its id.
    #[serde(default)]
    next_id: u32,
    /// Counts the changes made to the plan, so a window can tell a newer plan from an older one.
    #[serde(default)]
    rev: u64,
}

impl Default for Plan {
    fn default() -> Self {
        Self { blocks: Vec::new(), arrows: Vec::new(), only: true, problem: None, next_id: 0, rev: 0 }
    }
}

/// One line of text: what was typed, with every run of spaces and line ends as one space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The name of a block as it is kept, or why it cannot be that.
fn name_of(kind: Kind, text: &str) -> Result<String> {
    let text = one_line(text);
    if text.is_empty() {
        bail!("Give the block a name");
    }
    let (most, hint) = match kind {
        Kind::Note => (MAX_NOTE, ""),
        _ => (MAX_TEXT, " Say the rest under “What this block should do”."),
    };
    if text.chars().count() > most {
        bail!("The name of a block has at most {most} characters.{hint}");
    }
    Ok(text)
}

fn label_of(label: Option<&str>) -> Result<Option<String>> {
    let label = one_line(label.unwrap_or_default());
    if label.chars().count() > MAX_LABEL {
        bail!("A label has at most {MAX_LABEL} characters, such as “yes” or “no”");
    }
    Ok((!label.is_empty()).then_some(label))
}

/// How many of the plan's blocks stand how. A note is not built, and what was
/// in the code before is not this task's work: neither is counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Counts {
    pub built: usize,
    pub total: usize,
    pub building: usize,
    /// Planned, the user's own among them.
    pub planned: usize,
    pub proposed: usize,
}

impl Plan {
    fn index_of(&self, id: &str) -> Result<usize> {
        self.blocks.iter().position(|block| block.id == id).ok_or_else(|| anyhow!("The plan has no such block"))
    }

    pub fn block(&self, id: &str) -> Result<&Block> {
        Ok(&self.blocks[self.index_of(id)?])
    }

    fn block_mut(&mut self, id: &str) -> Result<&mut Block> {
        let index = self.index_of(id)?;
        Ok(&mut self.blocks[index])
    }

    fn new_id(&mut self) -> String {
        loop {
            self.next_id += 1;
            let id = format!("b{}", self.next_id);
            if !self.blocks.iter().any(|block| block.id == id) {
                return id;
            }
        }
    }

    fn room_for(&self, more: usize) -> Result<()> {
        if self.blocks.len() + more > MAX_BLOCKS {
            bail!("A plan holds at most {MAX_BLOCKS} blocks");
        }
        Ok(())
    }

    /// Adds a block of the user's own: after the block `after`, joined to it by
    /// an arrow, or loose. Returns the new block's id.
    pub fn add(&mut self, kind: Kind, text: &str, after: Option<&str>, label: Option<&str>) -> Result<String> {
        self.room_for(1)?;
        let text = if text.trim().is_empty() {
            match kind {
                Kind::Step => STAND_IN_NAMES[0].to_string(),
                Kind::Ask => STAND_IN_NAMES[1].to_string(),
                Kind::Note => STAND_IN_NAMES[2].to_string(),
            }
        } else {
            name_of(kind, text)?
        };
        let label = label_of(label)?;
        if let Some(after) = after
            && (kind == Kind::Note || self.block(after)?.kind == Kind::Note)
        {
            bail!("A note stands on its own. Arrows join steps and questions.");
        }
        let id = self.new_id();
        self.put(Block::new(id.clone(), kind, text, State::Planned), after, label);
        Ok(id)
    }

    /// Puts a new block into the list: after `after` and everything that
    /// already follows it, with an arrow from `after`; or at the end. The list
    /// then reads in the order of the chart, which is the order blocks are built in.
    fn put(&mut self, block: Block, after: Option<&str>, label: Option<String>) {
        let Some(from) = after.and_then(|after| self.index_of(after).ok()) else {
            self.blocks.push(block);
            return;
        };
        let after = self.blocks[from].id.clone();
        let followers = self.reached_from(&after);
        let last = self.blocks.iter().rposition(|other| followers.contains(other.id.as_str())).unwrap_or(from);
        self.arrows.push(Arrow { from: after, to: block.id.clone(), label });
        self.blocks.insert(last.max(from) + 1, block);
    }

    /// Changes what a block says. A built block that says something else now is to be built again.
    pub fn edit(&mut self, id: &str, text: Option<&str>, details: Option<&str>) -> Result<()> {
        let block = self.block_mut(id)?;
        if block.state == State::Building {
            bail!("This block is being built. Stop the turn first, then change it.");
        }
        let mut changed = false;
        if let Some(text) = text {
            let text = name_of(block.kind, text)?;
            changed |= text != block.text;
            block.text = text;
        }
        if let Some(details) = details {
            let details = details.trim().to_string();
            if details.chars().count() > MAX_DETAILS {
                bail!("“What this block should do” has at most {MAX_DETAILS} characters");
            }
            changed |= details != block.details;
            block.details = details;
        }
        if changed && block.state == State::Built {
            block.unbuild(State::Planned);
        }
        Ok(())
    }

    /// Draws an arrow from one block to another, or changes the label of the one that is there.
    pub fn connect(&mut self, from: &str, to: &str, label: Option<&str>) -> Result<()> {
        let label = label_of(label)?;
        if from == to {
            bail!("A block cannot come after itself");
        }
        if self.block(from)?.kind == Kind::Note || self.block(to)?.kind == Kind::Note {
            bail!("A note stands on its own. Arrows join steps and questions.");
        }
        match self.arrows.iter_mut().find(|arrow| arrow.from == from && arrow.to == to) {
            Some(arrow) => arrow.label = label,
            None => self.arrows.push(Arrow { from: from.to_string(), to: to.to_string(), label }),
        }
        Ok(())
    }

    pub fn disconnect(&mut self, from: &str, to: &str) -> Result<()> {
        let before = self.arrows.len();
        self.arrows.retain(|arrow| !(arrow.from == from && arrow.to == to));
        if self.arrows.len() == before {
            bail!("There is no arrow between these two blocks");
        }
        Ok(())
    }

    /// Takes a block out of the plan. A block in the middle of a line leaves
    /// the line whole: what came before it now leads to what came after it.
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let index = self.index_of(id)?;
        if self.blocks[index].state == State::Building {
            bail!("This block is being built. Stop the turn first, then remove it.");
        }
        let into: Vec<Arrow> = self.arrows.iter().filter(|arrow| arrow.to == id).cloned().collect();
        let out: Vec<Arrow> = self.arrows.iter().filter(|arrow| arrow.from == id).cloned().collect();
        self.arrows.retain(|arrow| arrow.from != id && arrow.to != id);
        if let ([before], [after]) = (into.as_slice(), out.as_slice())
            && before.from != after.to
            && !self.arrows.iter().any(|arrow| arrow.from == before.from && arrow.to == after.to)
        {
            self.arrows.push(Arrow { from: before.from.clone(), to: after.to.clone(), label: before.label.clone() });
        }
        self.blocks.remove(index);
        Ok(())
    }

    /// "I will do it myself", and giving the block back.
    pub fn take(&mut self, id: &str, mine: bool) -> Result<()> {
        let block = self.block_mut(id)?;
        match (block.state, mine) {
            (State::Building, _) => bail!("This block is being built. Stop the turn first."),
            (State::Proposed, _) => bail!("This block is not in the plan yet. Add it to the plan first."),
            (_, true) if block.kind == Kind::Note => bail!("A note is not built, so there is nothing to take"),
            (State::Yours, false) => block.state = State::Planned,
            (_, false) => bail!("This block is not yours"),
            (_, true) => block.unbuild(State::Yours),
        }
        Ok(())
    }

    /// Says yes to a proposed block, or to every one when no block is named.
    pub fn accept(&mut self, id: Option<&str>) -> Result<usize> {
        if let Some(id) = id
            && self.block(id)?.state != State::Proposed
        {
            bail!("This block is in the plan already");
        }
        let mut accepted = 0;
        for block in self.blocks.iter_mut().filter(|block| block.state == State::Proposed) {
            if id.is_none_or(|id| id == block.id) {
                block.unbuild(State::Planned);
                block.from = None;
                accepted += 1;
            }
        }
        Ok(accepted)
    }

    /// Turns a proposed block down, or every one when no block is named. It leaves with its arrows.
    pub fn decline(&mut self, id: Option<&str>) -> Result<usize> {
        if let Some(id) = id
            && self.block(id)?.state != State::Proposed
        {
            bail!("Only a proposed block can be turned down. This one is in the plan: remove it instead.");
        }
        let gone: HashSet<String> = self
            .blocks
            .iter()
            .filter(|block| block.state == State::Proposed && id.is_none_or(|id| id == block.id))
            .map(|block| block.id.clone())
            .collect();
        self.blocks.retain(|block| !gone.contains(&block.id));
        self.arrows.retain(|arrow| !gone.contains(&arrow.from) && !gone.contains(&arrow.to));
        Ok(gone.len())
    }

    /// Every block the arrows lead to from `from`, however many steps on.
    fn reached_from(&self, from: &str) -> HashSet<&str> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut open = vec![from];
        while let Some(at) = open.pop() {
            for arrow in self.arrows.iter().filter(|arrow| arrow.from == at) {
                if seen.insert(&arrow.to) {
                    open.push(&arrow.to);
                }
            }
        }
        seen
    }

    /// The arrows that lead back up a loop, such as "no" from "Right?" back to
    /// "Ask again". They are found by walking the plan from where it starts:
    /// an arrow to a block the walk is still inside of closes a loop.
    fn loops(&self) -> HashSet<usize> {
        fn walk<'a>(
            plan: &'a Plan,
            at: &'a str,
            inside: &mut Vec<&'a str>,
            done: &mut HashSet<&'a str>,
            back: &mut HashSet<usize>,
        ) {
            inside.push(at);
            for (index, arrow) in plan.arrows.iter().enumerate().filter(|(_, arrow)| arrow.from == at) {
                if inside.contains(&arrow.to.as_str()) {
                    back.insert(index);
                } else if !done.contains(arrow.to.as_str()) {
                    walk(plan, &arrow.to, inside, done, back);
                }
            }
            inside.pop();
            done.insert(at);
        }
        let (mut done, mut back) = (HashSet::new(), HashSet::new());
        // First from the blocks nothing leads to, then from whatever a loop without a start left over.
        let starts = self.blocks.iter().filter(|block| !self.arrows.iter().any(|arrow| arrow.to == block.id));
        for block in starts.chain(self.blocks.iter()) {
            if !done.contains(block.id.as_str()) {
                walk(self, &block.id, &mut Vec::new(), &mut done, &mut back);
            }
        }
        back
    }

    fn allowed(&self, id: &str, loops: &HashSet<usize>) -> bool {
        self.arrows.iter().enumerate().filter(|(index, arrow)| arrow.to == id && !loops.contains(index)).all(
            |(_, arrow)| {
                self.block(&arrow.from).is_ok_and(|before| settled(before.state) || before.state == State::Proposed)
            },
        )
    }

    /// Whether everything before a block allows it to be built: each block an
    /// arrow comes from is built, the user's own or was there before. A block
    /// that is not in the plan yet holds nothing up, and neither does an arrow
    /// that leads back up a loop.
    pub fn allows(&self, id: &str) -> bool {
        self.allowed(id, &self.loops())
    }

    /// The block "Build the next block" builds: the first planned one that everything before it allows.
    pub fn next(&self) -> Option<&Block> {
        let loops = self.loops();
        self.blocks
            .iter()
            .find(|block| block.kind != Kind::Note && block.state == State::Planned && self.allowed(&block.id, &loops))
    }

    /// Why there is no next block, for the user.
    fn why_no_next(&self) -> &'static str {
        let built: Vec<&Block> = self.blocks.iter().filter(|block| block.kind != Kind::Note).collect();
        if built.is_empty() {
            "The plan has no blocks yet. Add a step first."
        } else if built.iter().any(|block| block.state == State::Planned) {
            "No block can be built next: each planned block comes after one that is not built yet."
        } else if built.iter().any(|block| block.state == State::Proposed) {
            "The blocks that are left wait for your yes. Add them to the plan first."
        } else if built.iter().any(|block| block.state == State::Building) {
            "The block that is left is being built right now."
        } else {
            "Nothing is left to build: every block of the plan is built or yours."
        }
    }

    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for block in self.blocks.iter().filter(|block| block.kind != Kind::Note) {
            match block.state {
                State::Before => continue,
                State::Built => counts.built += 1,
                State::Building => counts.building += 1,
                State::Planned | State::Yours => counts.planned += 1,
                State::Proposed => counts.proposed += 1,
            }
            counts.total += 1;
        }
        counts
    }

    /// The plan as agents are shown it: the steps and questions that are in
    /// it, with a `*` on what is built. What is only proposed is left out.
    pub fn to_flow(&self) -> Flow {
        let inside =
            |id: &str| self.block(id).is_ok_and(|block| block.kind != Kind::Note && block.state != State::Proposed);
        let mut nodes: Vec<FlowNode> = self
            .blocks
            .iter()
            .filter(|block| inside(&block.id))
            .map(|block| FlowNode {
                id: block.id.clone(),
                ask: block.kind == Kind::Ask,
                text: block.text.clone(),
                changed: block.state == State::Built,
                place: block.files.iter().find(|place| place.line.is_some()).cloned(),
            })
            .collect();
        let arrows: Vec<Arrow> =
            self.arrows.iter().filter(|arrow| inside(&arrow.from) && inside(&arrow.to)).cloned().collect();
        // The format starts a flow at its first line: a block nothing leads to goes there.
        let reached = |id: &str| arrows.iter().any(|arrow| arrow.to == id);
        if nodes.first().is_some_and(|first| reached(&first.id))
            && let Some(start) = nodes.iter().position(|node| !reached(&node.id))
        {
            let node = nodes.remove(start);
            nodes.insert(0, node);
        }
        Flow { nodes, arrows }
    }

    /// The plan in the flow format, with what the format has no words for
    /// under it: which blocks the user builds themselves, and the notes. No
    /// empty line inside, so it reads as one piece wherever it is put.
    pub fn written(&self) -> String {
        let mut text = write_flow(&self.to_flow());
        let own: Vec<String> = self
            .blocks
            .iter()
            .filter(|block| block.kind != Kind::Note && block.state == State::Yours)
            .map(|block| format!("`{}`", block.id))
            .collect();
        if !own.is_empty() {
            text.push_str(&format!(
                "\nThe user builds these blocks themselves, so leave them alone: {}",
                own.join(", ")
            ));
        }
        let notes: Vec<&Block> =
            self.blocks.iter().filter(|block| block.kind == Kind::Note && block.state != State::Proposed).collect();
        if !notes.is_empty() {
            text.push_str("\nNotes on the plan:");
            for note in notes {
                text.push_str(&format!("\n- {}", note.wording()));
            }
        }
        text
    }

    /// Whether there is anything to hold an agent to.
    fn has_blocks(&self) -> bool {
        !self.to_flow().nodes.is_empty()
    }

    /// Adds a block an agent thinks is missing. It hangs after the block the
    /// agent was building when that is a step nothing follows yet; a question
    /// needs a labelled answer and a step leads to one place, so anywhere else
    /// the new block stands on its own until the user says where it goes.
    /// Returns its id, or `None` when the plan has that block already or is full.
    pub fn propose(&mut self, said: &str, after: Option<&str>, author: &Author) -> Option<String> {
        let said = one_line(said);
        if said.is_empty() || self.blocks.len() >= MAX_BLOCKS {
            return None;
        }
        // A long proposal is a short name with the whole sentence under it.
        let (text, details) = if said.chars().count() <= MAX_TEXT {
            (said, String::new())
        } else {
            let mut name = String::new();
            for word in said.split(' ') {
                if name.chars().count() + word.chars().count() + 1 > MAX_TEXT - 1 {
                    break;
                }
                if !name.is_empty() {
                    name.push(' ');
                }
                name.push_str(word);
            }
            if name.is_empty() {
                name = said.chars().take(MAX_TEXT - 1).collect();
            }
            (format!("{name}…"), said)
        };
        let same =
            |block: &Block| block.text.eq_ignore_ascii_case(&text) || (!details.is_empty() && block.details == details);
        if self.blocks.iter().any(same) {
            return None;
        }
        let id = self.new_id();
        let mut block = Block::new(id.clone(), Kind::Step, text, State::Proposed);
        block.details = details;
        block.by = Some(author.id.clone());
        block.by_name = Some(author.name.clone());
        block.at = Some(author.at);
        block.from = Some("build".into());
        let end_of_line = |after: &&str| {
            self.block(after).is_ok_and(|block| block.kind == Kind::Step)
                && !self.arrows.iter().any(|arrow| arrow.from == **after)
        };
        let after = after.filter(end_of_line);
        self.put(block, after, None);
        Some(id)
    }

    /// Puts a flow an agent drew into the plan. A draft of what is to be built
    /// arrives as proposed blocks; a drawing of the code arrives as blocks
    /// that were there before. An earlier draft or drawing that nobody
    /// touched since is replaced by the new one.
    pub fn add_flow(&mut self, flow: &Flow, from_code: bool, author: &Author) -> Result<Vec<String>> {
        let (from, state) = if from_code { ("code", State::Before) } else { ("draft", State::Proposed) };
        let old: HashSet<String> = self
            .blocks
            .iter()
            .filter(|block| block.from.as_deref() == Some(from) && block.state == state)
            .map(|block| block.id.clone())
            .collect();
        self.blocks.retain(|block| !old.contains(&block.id));
        self.arrows.retain(|arrow| !old.contains(&arrow.from) && !old.contains(&arrow.to));
        self.room_for(flow.nodes.len())?;

        let mut ids: HashMap<&str, String> = HashMap::new();
        for node in &flow.nodes {
            let id = self.new_id();
            let kind = if node.ask { Kind::Ask } else { Kind::Step };
            let mut block = Block::new(id.clone(), kind, node.text.clone(), state);
            block.by = Some(author.id.clone());
            block.by_name = Some(author.name.clone());
            block.at = Some(author.at);
            block.from = Some(from.into());
            block.files = node.place.iter().cloned().collect();
            self.blocks.push(block);
            ids.insert(&node.id, id);
        }
        for arrow in &flow.arrows {
            if let (Some(from), Some(to)) = (ids.get(arrow.from.as_str()), ids.get(arrow.to.as_str())) {
                self.arrows.push(Arrow { from: from.clone(), to: to.clone(), label: arrow.label.clone() });
            }
        }
        Ok(flow.nodes.iter().filter_map(|node| ids.get(node.id.as_str()).cloned()).collect())
    }

    /// Takes in what an agent answered to a request for a draft: its blocks,
    /// or the reason there are none, in plain words.
    pub fn draft_arrived(&mut self, answer: &str, from_code: bool, author: &Author) {
        let read =
            parse_flow(answer).and_then(|flow| self.add_flow(&flow, from_code, author).map_err(|e| e.to_string()));
        self.problem = read.err().map(|problem| {
            format!(
                "{} answered, but not in the flow format, so there is nothing to add to the plan. {problem}",
                author.name
            )
        });
    }

    /// The end of the turn that was building a block: built, with the files
    /// the turn changed, or planned again when the turn was stopped or failed.
    pub fn build_ended(&mut self, id: &str, ended_well: bool, files: Vec<Place>, proved: Vec<String>, author: &Author) {
        let Ok(block) = self.block_mut(id) else { return };
        if block.state != State::Building {
            return;
        }
        if !ended_well {
            block.unbuild(State::Planned);
            return;
        }
        block.state = State::Built;
        block.by = Some(author.id.clone());
        block.by_name = Some(author.name.clone());
        block.at = Some(author.at);
        block.files = files.into_iter().take(MAX_FILES).collect();
        block.proved = proved;
        block.built_from = Some(block.wording());
    }
}

/// The blocks an agent says are missing: the `PROPOSE: <text>` lines of its answer.
pub fn proposals_in(answer: &str) -> Vec<String> {
    marked_lines(answer, "PROPOSE:")
}

/// The tests an agent names as proof of what it built: the `PROVED BY: <test>` lines of its answer.
pub fn proofs_in(answer: &str) -> Vec<String> {
    marked_lines(answer, "PROVED BY:")
}

/// The lines of an answer that start with a mark the editor asked for, without the mark.
fn marked_lines(answer: &str, mark: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in answer.lines() {
        // Agents like to dress a line up: as a list item, in bold, as code.
        let bare = line.trim().trim_start_matches(['-', '*', '`', '>', ' ']);
        let Some(said) = bare.strip_prefix(mark) else { continue };
        let said = one_line(said.trim_matches(['*', '`', ' ']));
        // The pattern shows the text in angle brackets, and some agents keep them.
        let said = said.strip_prefix('<').and_then(|inner| inner.strip_suffix('>')).unwrap_or(&said).trim().to_string();
        if !said.is_empty() && !found.contains(&said) && found.len() < MAX_PROPOSALS {
            found.push(said);
        }
    }
    found
}

/// What a block is called until the user names it: a step, a question, a note.
const STAND_IN_NAMES: [&str; 3] = ["New step", "New question?", "New note"];

/// How many stretches of lines one file of a built block is told with.
const MAX_STRETCHES: usize = 3;

/// The lines a change put in, as stretches "from line to line". Stretches that touch are one.
fn stretches_of_lines(changes: &[gitops::Stretch]) -> Vec<(u32, u32)> {
    let mut found: Vec<(u32, u32)> = Vec::new();
    for change in changes.iter().filter(|change| !change.added.is_empty()) {
        let first = change.new_start as u32;
        let last = first + change.added.len() as u32 - 1;
        match found.last_mut() {
            Some((_, end)) if first <= *end + 1 => *end = (*end).max(last),
            _ => found.push((first, last)),
        }
    }
    found
}

/// The message that asks an agent to build one block.
fn build_message(plan: &Plan, block: &Block) -> String {
    let mut which = format!("Build only this block: `{}`, “{}”.", block.id, block.text);
    if !block.details.is_empty() {
        which.push_str(&format!("\nWhat it should do: {}", block.details));
    }
    if let Some(before) = block.built_from.as_ref().filter(|before| **before != block.wording()) {
        which.push_str(&format!(
            "\nIt was built before, when it read: {before}\nChange what was built so that it fits how the block reads now."
        ));
    }
    [
        format!(
            "Build one block of this task's plan, and nothing else. The plan, in the editor's flow format (`*` marks a block that is built already):\n\n{}",
            plan.written()
        ),
        which,
        "If you think a block is missing from the plan, do not build it. End your answer with one line `PROPOSE: <what the block should do>` for each such block."
            .to_string(),
        "If a test shows that this block works, name it in one more line: `PROVED BY: <the name of the test>`."
            .to_string(),
    ]
    .join("\n\n")
}

/// The message that asks an agent for a plan of the task, or for a drawing of the code as it is.
fn draft_message(described: &str, from_code: bool) -> String {
    let mut parts = vec![if from_code {
        "Draw the code this task is about as a flow chart: what happens in it now, step by step. Do not change any file."
            .to_string()
    } else {
        "Draft a plan for this task as a flow chart: what the task is meant to become, step by step. Do not change any file."
            .to_string()
    }];
    let described: String = described.trim().chars().take(2000).collect();
    if !described.is_empty() {
        let quoted: Vec<String> = described.lines().map(|line| format!("> {line}")).collect();
        parts.push(format!("The task, as it was first described:\n{}", quoted.join("\n")));
    }
    let place = if from_code { " @ src/file.ts:12" } else { "" };
    parts.push(format!(
        "Answer with one block in exactly this format, so the editor can draw it:\n\n```flow\nstep a: The first thing that happens{place}\nask b: A question with two answers?\nstep c: What happens on yes\nstep d: What happens on no\na -> b\nb -> c: yes\nb -> d: no\n```"
    ));
    let mut rules = vec![
        "The rules:".to_string(),
        "- `step` is something that happens. `ask` is a question; give each of its arrows a short label.".to_string(),
    ];
    if from_code {
        rules.push("- ` @ path:line` is optional. It says where in the code the step happens.".to_string());
    }
    rules.push(
        "- Ids are short and used once. The first line is where the flow starts. A step leads to one place at most."
            .to_string(),
    );
    rules.push(format!("- Use at most {MAX_NODES} blocks and {MAX_TEXT} characters of text in each."));
    parts.push(rules.join("\n"));
    parts.join("\n\n")
}

/// Everything after the user's last message: the newest turn.
fn newest_turn(events: &[Value]) -> &[Value] {
    let start = events.iter().rposition(|event| event["type"] == "user_message").map_or(0, |index| index + 1);
    &events[start..]
}

/// What the agent said in a turn, glued back together from its pieces.
fn answer_of(turn: &[Value]) -> String {
    turn.iter()
        .filter(|event| event["type"] == "update" && event["update"]["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|event| event["update"]["content"]["text"].as_str())
        .collect()
}

/// The files the agent's finished edits of a turn name, as paths inside `root`.
fn edited_in(turn: &[Value], root: &Path) -> BTreeSet<String> {
    #[derive(Default)]
    struct Call {
        edits: bool,
        done: bool,
        paths: Vec<String>,
    }
    let mut calls: Vec<(String, Call)> = Vec::new();
    for event in turn.iter().filter(|event| event["type"] == "update") {
        let update = &event["update"];
        if !matches!(update["sessionUpdate"].as_str(), Some("tool_call" | "tool_call_update")) {
            continue;
        }
        let id = update["toolCallId"].as_str().unwrap_or_default();
        let index = calls.iter().position(|(known, _)| known == id).unwrap_or_else(|| {
            calls.push((id.to_string(), Call::default()));
            calls.len() - 1
        });
        let call = &mut calls[index].1;
        if matches!(update["kind"].as_str(), Some("edit" | "delete" | "move")) {
            call.edits = true;
        }
        if let Some(status) = update["status"].as_str() {
            call.done = status == "completed";
        }
        let located = update["locations"].as_array().into_iter().flatten().filter_map(|place| place["path"].as_str());
        call.paths.extend(located.map(str::to_string));
        for piece in update["content"].as_array().into_iter().flatten().filter(|piece| piece["type"] == "diff") {
            call.edits = true;
            call.paths.extend(piece["path"].as_str().map(str::to_string));
        }
    }
    calls
        .into_iter()
        .filter(|(_, call)| call.edits && call.done)
        .flat_map(|(_, call)| call.paths)
        .filter_map(|path| review::relative_to(root, &path))
        .collect()
}

// ----- what the core keeps and does -----

/// What a task's agent is doing for the plan right now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Doing {
    /// Building the block with this id.
    Build(String),
    /// Drafting a plan from the issue.
    Draft,
    /// Drawing the code as it is.
    Draw,
}

/// A number that only grows. A turn takes one when it starts and a job takes one when it is made, so what was
/// started later always has the larger one.
pub(crate) fn turn_number() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

/// The one message of the plan a task's agent is working on.
#[derive(Debug, Clone)]
struct Job {
    doing: Doing,
    /// The message as it was sent, so the end of its turn is told from the end of any other.
    message: String,
    /// Taken when the job was made. Only a turn that started after it can be this job's turn: the same block
    /// built twice in a row sends the same message twice, and the end of the first must not end the second.
    made: u64,
    started_at: u64,
}

/// The plans of every task: what is in flight, and the order changes are made in.
#[derive(Default)]
pub struct Plans {
    /// A plan is read, changed and written back as one step.
    changing: Mutex<()>,
    jobs: Mutex<HashMap<String, Job>>,
}

fn plan_key(task_id: &str) -> String {
    format!("task.{task_id}.plan")
}

impl Manager {
    fn plan_job(&self, task_id: &str) -> Option<Job> {
        self.plans.jobs.lock().unwrap().get(task_id).cloned()
    }

    /// The plan of a task as it is kept. A task without one has an empty plan.
    fn stored_plan(&self, task_id: &str) -> Plan {
        self.store()
            .setting(&plan_key(task_id))
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn load_plan(&self, task_id: &str) -> Result<Plan> {
        self.store().task(task_id)?;
        let mut plan = self.stored_plan(task_id);
        // A block that was being built when the core stopped is not being built any more.
        let building = match self.plan_job(task_id) {
            Some(Job { doing: Doing::Build(id), .. }) => Some(id),
            _ => None,
        };
        for block in plan.blocks.iter_mut().filter(|block| block.state == State::Building) {
            if building.as_deref() != Some(block.id.as_str()) {
                block.unbuild(State::Planned);
            }
        }
        Ok(plan)
    }

    /// The plan as the interface gets it: the blocks and arrows, and what can be done next.
    fn plan_view(&self, task_id: &str, plan: &Plan) -> Value {
        let busy = self.plan_job(task_id).map(|job| match job.doing {
            Doing::Build(id) => json!({ "kind": "build", "blockId": id }),
            Doing::Draft => json!({ "kind": "draft" }),
            Doing::Draw => json!({ "kind": "draw" }),
        });
        let next = plan.next().map(|block| block.id.clone());
        json!({
            "rev": plan.rev,
            "blocks": plan.blocks,
            "arrows": plan.arrows,
            "only": plan.only,
            "problem": plan.problem,
            "busy": busy,
            "next": next,
            // Why "Build the next block" has nothing to build, when it has not.
            "nextWhy": next.is_none().then(|| plan.why_no_next()),
            "counts": plan.counts(),
        })
    }

    /// Tells every window how the plan stands now.
    fn announce_plan(&self, task_id: &str, plan: &Plan) {
        self.broadcast("plan.changed", json!({ "taskId": task_id, "plan": self.plan_view(task_id, plan) }));
    }

    /// Makes one change to a task's plan, keeps it and tells every window.
    fn change_plan<T>(&self, task_id: &str, change: impl FnOnce(&mut Plan) -> Result<T>) -> Result<T> {
        let _one_at_a_time = self.plans.changing.lock().unwrap();
        let mut plan = self.load_plan(task_id)?;
        let done = change(&mut plan)?;
        plan.rev += 1;
        self.store().set_setting(&plan_key(task_id), Some(&serde_json::to_string(&plan)?))?;
        self.announce_plan(task_id, &plan);
        Ok(done)
    }

    /// The plan of a task.
    pub fn plan_of(&self, task_id: &str) -> Result<Value> {
        Ok(self.plan_view(task_id, &self.load_plan(task_id)?))
    }

    /// The agent of a task, as the author of what it adds to the plan now.
    fn plan_author(&self, task: &TaskRow) -> Author {
        let name =
            self.registry().get(&task.agent_id).map_or_else(|| task.agent_id.clone(), |agent| agent.name.clone());
        Author { id: task.agent_id.clone(), name, at: now_ms() }
    }

    /// Refuses when the task's agent cannot take a message of the plan now.
    fn plan_agent_is_free(&self, task: &TaskRow) -> Result<()> {
        let name = self.plan_author(task).name;
        if let Some(job) = self.plan_job(&task.id) {
            match job.doing {
                Doing::Build(id) => {
                    let text =
                        self.stored_plan(&task.id).block(&id).map(|block| block.text.clone()).unwrap_or_default();
                    bail!("{name} is building “{text}”. Wait for it or stop its turn.");
                }
                Doing::Draft | Doing::Draw => bail!("{name} is drawing the plan. Wait for it or stop its turn."),
            }
        }
        if matches!(self.state_of(&task.id), TaskState::Running | TaskState::WaitingPermission) {
            bail!("{name} is still working. Wait for it or stop its turn.");
        }
        Ok(())
    }

    /// Sends a message of the plan and remembers it until its turn ends.
    /// `undo` puts the plan back when the message could not be sent.
    async fn send_plan_message(
        self: &Arc<Self>,
        task_id: &str,
        message: String,
        undo: impl FnOnce(&mut Plan),
    ) -> Result<Value> {
        if let Err(error) = self.prompt(task_id, message.clone()).await {
            self.change_plan(task_id, |plan| {
                let mut jobs = self.plans.jobs.lock().unwrap();
                if jobs.get(task_id).is_some_and(|job| job.message == message) {
                    jobs.remove(task_id);
                }
                undo(plan);
                Ok(())
            })?;
            return Err(error);
        }
        self.plan_of(task_id)
    }

    /// Builds one block: the one named, or the next one the plan allows. The
    /// block is marked as being built and the agent gets the plan and the
    /// block's name. The end of that turn decides how the block stands.
    pub async fn plan_build(self: &Arc<Self>, task_id: &str, block_id: Option<&str>) -> Result<Value> {
        let task = self.store().task(task_id)?;
        let author = self.plan_author(&task);
        let (was, message) = self.change_plan(task_id, |plan| {
            // Asked here, where one change is made at a time: two windows cannot both start a block.
            self.plan_agent_is_free(&task)?;
            let id = match block_id {
                Some(id) => id.to_string(),
                None => plan.next().map(|block| block.id.clone()).ok_or_else(|| anyhow!(plan.why_no_next()))?,
            };
            let block = plan.block(&id)?.clone();
            match (block.kind, block.state) {
                (Kind::Note, _) => bail!("A note is not built"),
                (_, State::Proposed) => bail!("This block is not in the plan yet. Add it to the plan first."),
                (_, State::Yours) => bail!("You took this block for yourself. Give it back first."),
                _ => {}
            }
            let message = build_message(plan, &block);
            let building = plan.block_mut(&id)?;
            building.state = State::Building;
            building.by = Some(author.id.clone());
            building.by_name = Some(author.name.clone());
            // From here on the end of a turn with this message is the end of this build.
            let job =
                Job { doing: Doing::Build(id), message: message.clone(), made: turn_number(), started_at: now_ms() };
            self.plans.jobs.lock().unwrap().insert(task_id.to_string(), job);
            Ok((block, message))
        })?;
        self.send_plan_message(task_id, message, move |plan| {
            if let Ok(block) = plan.block_mut(&was.id) {
                *block = was;
            }
        })
        .await
    }

    /// Asks the agent to draw a plan: of what the task is to become, from its
    /// first message, or of the code as it is now.
    pub async fn plan_draft(self: &Arc<Self>, task_id: &str, from_code: bool) -> Result<Value> {
        let task = self.store().task(task_id)?;
        // The first thing said in a task is its issue, or the user's own description of it.
        let events = self.store().events(task_id)?;
        let first = events.iter().find(|event| event["type"] == "user_message");
        let message = draft_message(first.and_then(|event| event["text"].as_str()).unwrap_or_default(), from_code);
        self.change_plan(task_id, |plan| {
            self.plan_agent_is_free(&task)?;
            plan.problem = None;
            let doing = if from_code { Doing::Draw } else { Doing::Draft };
            let job = Job { doing, message: message.clone(), made: turn_number(), started_at: now_ms() };
            self.plans.jobs.lock().unwrap().insert(task_id.to_string(), job);
            Ok(())
        })?;
        self.send_plan_message(task_id, message, |_| {}).await
    }

    /// Called at the end of every turn of a task, with the message the turn
    /// answered, the number the turn took when it started and whether it ended
    /// by itself. This is where a block that was being built becomes built, a
    /// draft arrives and proposals are read.
    pub(crate) async fn plan_turn_ended(self: &Arc<Self>, task_id: &str, message: &str, turn: u64, ended_well: bool) {
        let own = |job: &Job| job.message == message && job.made < turn;
        let job = self.plan_job(task_id).filter(own);
        // Any other turn matters to the plan only when its answer may propose a block.
        if job.is_none() && !ended_well {
            return;
        }
        let (manager, id, waited_for) = (self.clone(), task_id.to_string(), job.is_some());
        // It reads the store and may ask git, so it runs off the event loop.
        match tokio::task::spawn_blocking(move || manager.close_plan_turn(&id, job, ended_well)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::debug!("Could not bring the plan of {task_id} up to date: {error:#}"),
            Err(error) => tracing::debug!("Could not bring the plan of {task_id} up to date: {error}"),
        }
        // Whatever went wrong above, that message's turn is over: nothing may wait for it for ever.
        let left_over = waited_for && {
            let mut jobs = self.plans.jobs.lock().unwrap();
            let stuck = jobs.get(task_id).is_some_and(own);
            if stuck {
                jobs.remove(task_id);
            }
            stuck
        };
        if left_over {
            let _ = self.change_plan(task_id, |_| Ok(()));
        }
    }

    fn close_plan_turn(&self, task_id: &str, job: Option<Job>, ended_well: bool) -> Result<()> {
        let task = self.store().task(task_id)?;
        let author = self.plan_author(&task);
        let Some(job) = job else {
            // An ordinary turn. If it was told the plan, the agent may have proposed blocks.
            let plan = self.stored_plan(task_id);
            if !plan.only || !plan.has_blocks() {
                return Ok(());
            }
            let events = self.store().events(task_id)?;
            let proposed = proposals_in(&answer_of(newest_turn(&events)));
            if proposed.is_empty() {
                return Ok(());
            }
            return self.change_plan(task_id, |plan| {
                for said in &proposed {
                    plan.propose(said, None, &author);
                }
                Ok(())
            });
        };

        let events = self.store().events(task_id)?;
        let turn = newest_turn(&events);
        let answer = answer_of(turn);
        match &job.doing {
            Doing::Build(id) => {
                let files = if ended_well { self.files_of_turn(&task, turn, job.started_at) } else { Vec::new() };
                let proved = if ended_well { proofs_in(&answer) } else { Vec::new() };
                let mut built = None;
                self.change_plan(task_id, |plan| {
                    self.plans.jobs.lock().unwrap().remove(task_id);
                    plan.build_ended(id, ended_well, files, proved, &author);
                    if ended_well {
                        built = plan.blocks.iter().find(|block| block.id == *id).map(|block| block.text.clone());
                        for said in proposals_in(&answer) {
                            plan.propose(&said, Some(id), &author);
                        }
                    }
                    Ok(())
                })?;
                // The conversation keeps what was built and by whom, next to everything else that happened.
                if let Some(text) = built {
                    self.emit(
                        task_id,
                        json!({ "type": "plan_block", "action": "built", "blockId": id, "text": text, "by": author.id, "byName": author.name }),
                    );
                }
                Ok(())
            }
            Doing::Draft | Doing::Draw => self.change_plan(task_id, |plan| {
                self.plans.jobs.lock().unwrap().remove(task_id);
                // A turn that was stopped has no answer to read; the conversation says why.
                if ended_well {
                    plan.draft_arrived(&answer, job.doing == Doing::Draw, &author);
                }
                Ok(())
            }),
        }
    }

    /// The files a turn changed: those its finished edits name, and in a git
    /// project everything that differs from the code as it was when the
    /// turn's message was sent. That covers what a command of the agent changed.
    fn files_of_turn(&self, task: &TaskRow, turn: &[Value], started_at: u64) -> Vec<Place> {
        let root = Path::new(&task.cwd);
        let mut files = edited_in(turn, root);
        // In a git project the turn's own lines are known too: the stretches it put in.
        let mut lines: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        if task.branch.is_some()
            && let Ok(checkpoints) = self.store().checkpoints(&task.id)
            && let Some(before) = checkpoints
                .iter()
                .rev()
                // The checkpoint of this turn's own message, not one an earlier turn left.
                .find(|row| (row.kind == "message" || row.kind == "start") && row.at >= started_at)
            && let Ok(now) = gitops::snapshot_tree(root)
            && now != before.tree
            && let Ok(changed) = gitops::changes_between(root, &before.tree, &now)
        {
            files.extend(changed.into_iter().map(|file| file.path));
            if let Ok(stretches) = gitops::stretches_between(root, &before.tree, &now) {
                for (path, list) in stretches {
                    lines.insert(path, stretches_of_lines(&list));
                }
            }
        }
        let mut places = Vec::new();
        for path in files {
            match lines.get(&path).filter(|list| !list.is_empty()) {
                Some(list) => {
                    for (first, last) in list.iter().take(MAX_STRETCHES) {
                        places.push(Place { path: path.clone(), line: Some(*first), to: Some(*last) });
                    }
                }
                None => places.push(Place { path, line: None, to: None }),
            }
        }
        places
    }

    /// What an agent is told about the task's plan with a message, if anything.
    pub(crate) fn plan_note(&self, task_id: &str) -> Option<String> {
        // The message that builds a block holds the plan and says what to keep to: no need to say it twice.
        if matches!(self.plan_job(task_id), Some(Job { doing: Doing::Build(_), .. })) {
            return None;
        }
        let plan = self.stored_plan(task_id);
        if !plan.only || !plan.has_blocks() {
            return None;
        }
        Some(format!(
            "Note from the editor: this task is built from a plan, shown here in the editor's flow format (`*` marks a block that is built already). \
             Build nothing that is not a block of this plan; if something is missing, do not build it, and end your answer with a line `PROPOSE: <what the block should do>` instead.\n{}",
            plan.written()
        ))
    }

    /// Every command of the Plan view. `plan.build` and `plan.draft` send the
    /// agent a message; the others only change the plan. Each answers with the
    /// plan as it stands after.
    pub async fn plan_command(self: &Arc<Self>, method: &str, params: &Value) -> Result<Value> {
        let text = |name: &str| params.get(name).and_then(Value::as_str);
        let need = |name: &str| text(name).ok_or_else(|| anyhow!("Missing parameter: {name}"));
        let flag = |name: &str| params.get(name).and_then(Value::as_bool).unwrap_or(true);
        let task_id = need("taskId")?;
        match method {
            "plan.get" => return self.plan_of(task_id),
            "plan.build" => return self.plan_build(task_id, text("blockId")).await,
            "plan.draft" => return self.plan_draft(task_id, text("from") == Some("code")).await,
            _ => {}
        }
        let mut added = None;
        // A block made in the interface starts under a stand-in name; the name it gets next is its first real one.
        let mut named = None;
        self.change_plan(task_id, |plan| {
            match method {
                "plan.add" => {
                    let kind = serde_json::from_value(params.get("kind").cloned().unwrap_or(json!("step")))
                        .map_err(|_| anyhow!("A block is a step, a question (\"ask\") or a note"))?;
                    added = Some(plan.add(kind, text("text").unwrap_or_default(), text("after"), text("label"))?);
                }
                "plan.edit" => {
                    let id = need("blockId")?;
                    let before = plan.block(id)?.text.clone();
                    plan.edit(id, text("text"), text("details"))?;
                    let block = plan.block(id)?;
                    if STAND_IN_NAMES.contains(&before.as_str()) && block.text != before && block.by.is_none() {
                        named = Some(id.to_string());
                    }
                }
                "plan.connect" => plan.connect(need("from")?, need("to")?, text("label"))?,
                "plan.disconnect" => plan.disconnect(need("from")?, need("to")?)?,
                "plan.remove" => plan.remove(need("blockId")?)?,
                "plan.take" => plan.take(need("blockId")?, flag("mine"))?,
                "plan.accept" => drop(plan.accept(text("blockId"))?),
                "plan.decline" => drop(plan.decline(text("blockId"))?),
                "plan.only" => plan.only = flag("on"),
                "plan.dismiss" => plan.problem = None,
                other => bail!("Unknown method: {other}"),
            }
            Ok(())
        })?;
        let mut view = self.plan_of(task_id)?;
        // The conversation keeps who added what, so the agent's lines and the user's read as one story.
        // A block is told once it has a name of its own: at once when it came with one, else when it is first renamed.
        let told = added.clone().or(named);
        let block = told.as_deref().and_then(|id| {
            view["blocks"].as_array().and_then(|blocks| blocks.iter().find(|block| block["id"] == id)).cloned()
        });
        if let Some(block) = block
            && let (Some(text), Some(kind)) = (block["text"].as_str(), block["kind"].as_str())
            && !STAND_IN_NAMES.contains(&text)
        {
            self.emit(
                task_id,
                json!({ "type": "plan_block", "action": "added", "blockId": block["id"], "kind": kind, "text": text, "by": "you" }),
            );
        }
        if let Some(id) = added {
            view["blockId"] = json!(id);
        }
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(path: &str) -> Place {
        Place { path: path.into(), line: None, to: None }
    }

    #[test]
    fn the_lines_a_turn_put_in_are_told_as_stretches() {
        let change = |new_start: usize, added: usize| gitops::Stretch {
            old_start: new_start,
            old_len: 0,
            new_start,
            added: vec!["x".to_string(); added],
            removed: vec![],
        };
        // Lines 4 to 9, a line taken out (it adds nothing), 10 to 11 touching the first, and 20 alone.
        let found = stretches_of_lines(&[change(4, 6), change(9, 0), change(10, 2), change(20, 1)]);
        assert_eq!(found, [(4, 11), (20, 20)]);
        assert!(stretches_of_lines(&[change(3, 0)]).is_empty());
    }

    #[test]
    fn the_tests_an_agent_names_as_proof_are_read_out_of_its_answer() {
        let answer = "Done.\n\n- PROVED BY: `answers 429 on the sixth attempt`\nPROVED BY: <resets after a minute>\nPROPOSE: Log it";
        assert_eq!(proofs_in(answer), ["answers 429 on the sixth attempt", "resets after a minute"]);
        assert_eq!(proposals_in(answer), ["Log it"]);
        assert!(proofs_in("It is proved by nothing.").is_empty());
    }

    fn author() -> Author {
        Author { id: "mock".into(), name: "Practice agent".into(), at: 7 }
    }

    /// The plan of the design: a request, a question with two answers.
    fn login_plan() -> (Plan, [String; 4]) {
        let mut plan = Plan::default();
        let request = plan.add(Kind::Step, "Request to POST /login", None, None).unwrap();
        let over = plan.add(Kind::Ask, "Over the limit?", Some(&request), None).unwrap();
        let refuse = plan.add(Kind::Step, "Respond 429", Some(&over), Some("yes")).unwrap();
        let check = plan.add(Kind::Step, "Check the password", Some(&over), Some("no")).unwrap();
        (plan, [request, over, refuse, check])
    }

    #[test]
    fn a_plan_is_written_in_the_flow_format_and_read_back_the_same() {
        let (mut plan, [request, _, refuse, _]) = login_plan();
        plan.block_mut(&request).unwrap().state = State::Before;
        plan.block_mut(&request).unwrap().files = vec![Place { path: "src/app.ts".into(), line: Some(4), to: None }];
        plan.block_mut(&refuse).unwrap().state = State::Built;
        let written = plan.written();
        assert_eq!(
            written,
            "```flow\nstep b1: Request to POST /login @ src/app.ts:4\nask b2: Over the limit?\nstep* b3: Respond 429\nstep b4: Check the password\nb1 -> b2\nb2 -> b3: yes\nb2 -> b4: no\n```"
        );
        // Read by the same rules an agent's answer is read by, it is the same flow again.
        let read = parse_flow(&format!("Here is the plan.\n\n{written}\n\nThat is all.")).unwrap();
        assert_eq!(read, plan.to_flow());
        assert_eq!(write_flow(&read), written);
        assert_eq!(
            read.nodes[2],
            FlowNode { id: "b3".into(), ask: false, text: "Respond 429".into(), changed: true, place: None }
        );
    }

    #[test]
    fn what_is_only_proposed_and_what_is_a_note_stays_out_of_the_flow() {
        let (mut plan, [_, _, refuse, _]) = login_plan();
        plan.add(Kind::Note, "Never hardcode the limit", None, None).unwrap();
        plan.propose("Reset the count after a correct password", Some(&refuse), &author()).unwrap();
        plan.take(&refuse, true).unwrap();
        let written = plan.written();
        assert!(!written.contains("Reset the count"), "{written}");
        // What the user took for themselves is in the flow like any block, and named under it.
        assert!(
            written.ends_with(
                "```\nThe user builds these blocks themselves, so leave them alone: `b3`\nNotes on the plan:\n- Never hardcode the limit"
            ),
            "{written}"
        );
        assert!(!written.contains("\n\n"), "one piece, with no empty line: {written}");
        assert_eq!(plan.to_flow().nodes.len(), 4);
        // The block nothing leads to starts the flow, wherever it stands in the list.
        let mut turned = Plan::default();
        let last = turned.add(Kind::Step, "Second", None, None).unwrap();
        let first = turned.add(Kind::Step, "First", None, None).unwrap();
        turned.connect(&first, &last, None).unwrap();
        assert_eq!(turned.to_flow().nodes[0].text, "First");
    }

    #[test]
    fn a_flow_is_read_by_the_rules_of_the_map_view() {
        let answer = "Sure.\r\n```flow\r\nstep a: Mail it to: ops@example.com\r\n\r\nask b: Sent? @ src/mail.ts:12\r\nstep* c: Done @ a b:3\r\nstep d: Try again\r\na -> b\r\nb->c:yes\r\nb -> d: no\r\n```\r\n";
        let flow = parse_flow(answer).unwrap();
        assert_eq!(flow.nodes[0].text, "Mail it to: ops@example.com");
        assert_eq!(flow.nodes[0].place, None);
        assert_eq!(flow.nodes[1].place, Some(Place { path: "src/mail.ts".into(), line: Some(12), to: None }));
        assert!(flow.nodes[1].ask && !flow.nodes[1].changed);
        // "a b:3" is not a place: a path has no space in it.
        assert_eq!((flow.nodes[2].text.as_str(), flow.nodes[2].changed), ("Done @ a b:3", true));
        assert_eq!(flow.arrows[1], Arrow { from: "b".into(), to: "c".into(), label: Some("yes".into()) });

        let problem = |lines: &[&str]| parse_flow(&format!("```flow\n{}\n```", lines.join("\n"))).unwrap_err();
        assert_eq!(parse_flow("The limiter allows five attempts.").unwrap_err(), "The answer has no flow block.");
        assert_eq!(parse_flow("```flow\nstep a: One").unwrap_err(), "The flow block is not closed.");
        assert_eq!(problem(&["step a: One"]), "A flow needs at least two steps.");
        assert_eq!(
            problem(&["step a: One", "First, the request comes in."]),
            "Line 2 of the flow is not a step, a question or an arrow: \"First, the request comes in.\"."
        );
        assert_eq!(problem(&["step a: One", "step a: Two"]), "Line 2 of the flow uses the id \"a\" a second time.");
        assert_eq!(
            problem(&[&format!("step a: {}", "x".repeat(61)), "step b: Two"]),
            "Line 1 of the flow has more than 60 characters of text."
        );
        assert_eq!(
            problem(&["step a: One", "step b: Two", "a -> b: a very long label"]),
            "Line 3 of the flow has a label of more than 12 characters."
        );
        assert_eq!(
            problem(&["step a: One", "step b: Two", "a -> c"]),
            "An arrow names \"c\", which is not a step of the flow."
        );
        assert_eq!(problem(&["step a: One", "step b: Two", "a -> a"]), "The step \"a\" has an arrow to itself.");
        assert_eq!(
            problem(&["step a: One", "step b: Two", "step c: Three", "a -> b", "a -> c"]),
            "The step \"a\" leads to 2 places. Only a question may do that."
        );
        assert_eq!(
            problem(&["ask a: One?", "step b: Two", "a -> b: yes"]),
            "The question \"a\" needs at least two answers."
        );
        assert_eq!(
            problem(&["ask a: One?", "step b: Two", "step c: Three", "a -> b: yes", "a -> c"]),
            "An answer to the question \"a\" has no label."
        );
        assert_eq!(problem(&["step a: One", "step b: Two"]), "No arrow leads to the step \"b\".");
        assert_eq!(
            problem(&["stepa: One", "step b: Two"]).split(':').next(),
            Some("Line 1 of the flow is not a step, a question or an arrow")
        );
        let many: Vec<String> = (0..31).map(|n| format!("step s{n}: Step {n}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(problem(&many), "The flow has more than 30 steps.");
    }

    #[test]
    fn the_next_block_is_the_first_planned_one_that_what_comes_before_it_allows() {
        let (mut plan, [request, over, refuse, check]) = login_plan();
        let next = |plan: &Plan| plan.next().map(|block| block.id.clone());
        assert_eq!(next(&plan), Some(request.clone()), "nothing comes before the first block");
        assert!(!plan.allows(&over) && !plan.allows(&refuse));

        plan.block_mut(&request).unwrap().state = State::Built;
        assert_eq!(next(&plan), Some(over.clone()));
        // A block the user took, or one that was in the code before, lets the blocks after it be built.
        plan.take(&over, true).unwrap();
        assert_eq!(next(&plan), Some(refuse.clone()), "the first in the order of the list");
        plan.block_mut(&refuse).unwrap().state = State::Building;
        assert_eq!(next(&plan), Some(check.clone()));
        plan.block_mut(&check).unwrap().state = State::Before;
        assert_eq!(next(&plan), None);
        assert_eq!(plan.why_no_next(), "The block that is left is being built right now.");
        plan.block_mut(&refuse).unwrap().state = State::Built;
        assert_eq!(plan.why_no_next(), "Nothing is left to build: every block of the plan is built or yours.");

        // An arrow back up a loop holds nothing up, and a block that is only proposed neither.
        let mut looped = Plan::default();
        let a = looped.add(Kind::Step, "Ask for the password", None, None).unwrap();
        let b = looped.add(Kind::Ask, "Right?", Some(&a), None).unwrap();
        looped.connect(&b, &a, Some("no")).unwrap();
        assert_eq!(next(&looped), Some(a.clone()));
        let waiting = looped.propose("Count the attempts", None, &author()).unwrap();
        looped.connect(&waiting, &a, None).unwrap();
        assert_eq!(next(&looped), Some(a));
        assert!(!looped.allows(&b));

        // A note is never built, and a plan of notes has nothing to build.
        let mut notes = Plan::default();
        notes.add(Kind::Note, "Keep it small", None, None).unwrap();
        assert_eq!(next(&notes), None);
        assert_eq!(notes.why_no_next(), "The plan has no blocks yet. Add a step first.");
        let mut stuck = Plan::default();
        let x = stuck.add(Kind::Step, "One", None, None).unwrap();
        let y = stuck.add(Kind::Step, "Two", Some(&x), None).unwrap();
        stuck.block_mut(&x).unwrap().state = State::Building;
        assert_eq!(next(&stuck), None);
        assert!(stuck.why_no_next().starts_with("No block can be built next"));
        stuck.remove(&y).unwrap();
        assert_eq!(stuck.why_no_next(), "The block that is left is being built right now.");
        let y = stuck.add(Kind::Step, "Two", Some(&x), None).unwrap();
        stuck.block_mut(&x).unwrap().state = State::Built;
        stuck.block_mut(&y).unwrap().state = State::Proposed;
        assert!(stuck.why_no_next().starts_with("The blocks that are left wait for your yes"));
    }

    #[test]
    fn blocks_change_state_as_the_user_and_the_agent_act_on_them() {
        let (mut plan, [request, over, refuse, check]) = login_plan();
        assert_eq!(plan.counts(), Counts { built: 0, total: 4, building: 0, planned: 4, proposed: 0 });

        // Built at the end of a good turn, with the files of that turn.
        plan.block_mut(&refuse).unwrap().state = State::Building;
        assert!(plan.edit(&refuse, Some("Respond 418"), None).unwrap_err().to_string().contains("being built"));
        assert!(plan.remove(&refuse).is_err() && plan.take(&refuse, true).is_err());
        plan.build_ended(
            &refuse,
            true,
            vec![place("src/middleware/rateLimit.ts")],
            vec!["answers 429".into()],
            &author(),
        );
        let built = plan.block(&refuse).unwrap().clone();
        assert_eq!((built.state, built.by.as_deref(), built.at), (State::Built, Some("mock"), Some(7)));
        assert_eq!(built.files, vec![Place { path: "src/middleware/rateLimit.ts".into(), line: None, to: None }]);
        assert_eq!(built.built_from.as_deref(), Some("Respond 429"));
        // The end of a turn that built nothing of the plan changes nothing.
        plan.build_ended(&check, true, vec![], vec![], &author());
        assert_eq!(plan.block(&check).unwrap().state, State::Planned);

        // The same text again is no change. Another text, or more to do, and it is planned again.
        plan.edit(&refuse, Some("  Respond   429 "), Some("")).unwrap();
        assert_eq!(plan.block(&refuse).unwrap().state, State::Built);
        plan.edit(&refuse, None, Some("Say in Retry-After how long to wait.")).unwrap();
        let again = plan.block(&refuse).unwrap().clone();
        assert_eq!((again.state, &again.by, again.files.len()), (State::Planned, &None, 0));
        assert!(build_message(&plan, &again).contains("It was built before, when it read: Respond 429\n"));

        // A stopped or failed turn puts the block back.
        plan.block_mut(&refuse).unwrap().state = State::Building;
        plan.build_ended(&refuse, false, vec![place("x")], vec!["a test".into()], &author());
        assert_eq!(plan.block(&refuse).unwrap().state, State::Planned);

        // Taking a block and giving it back.
        plan.take(&over, true).unwrap();
        assert_eq!(plan.block(&over).unwrap().state, State::Yours);
        assert!(plan.take(&request, false).unwrap_err().to_string().contains("not yours"));
        plan.take(&over, false).unwrap();
        assert_eq!(plan.block(&over).unwrap().state, State::Planned);

        // A proposal waits; yes makes it planned, no takes it away with its arrow.
        let first = plan.propose("Reset the count after a correct password", Some(&check), &author()).unwrap();
        assert_eq!(plan.propose("reset the count after a correct password", None, &author()), None, "not twice");
        let second = plan.propose("Log the blocked attempt", None, &author()).unwrap();
        assert_eq!(plan.counts(), Counts { built: 0, total: 6, building: 0, planned: 4, proposed: 2 });
        assert_eq!(plan.block(&first).unwrap().from.as_deref(), Some("build"));
        assert!(plan.arrows.contains(&Arrow { from: check.clone(), to: first.clone(), label: None }));
        assert!(
            plan.take(&first, true).is_err()
                && plan.accept(Some(&check)).is_err()
                && plan.decline(Some(&check)).is_err()
        );
        assert_eq!(plan.accept(Some(&first)).unwrap(), 1);
        let accepted = plan.block(&first).unwrap();
        assert_eq!((accepted.state, &accepted.by, &accepted.from), (State::Planned, &None, &None));
        assert_eq!(plan.decline(None).unwrap(), 1);
        assert!(plan.block(&second).is_err());
        assert_eq!(plan.accept(None).unwrap(), 0);

        // After a question, or a step that already leads somewhere, a proposal stands on its own.
        let loose = plan.propose("Warn before the last attempt", Some(&over), &author()).unwrap();
        assert!(!plan.arrows.iter().any(|arrow| arrow.to == loose));
        assert_eq!(plan.blocks.last().unwrap().id, loose);
        plan.decline(Some(&loose)).unwrap();

        // A long proposal becomes a short name with the whole sentence under it.
        let long = "Tell the user how many attempts are left before the account is locked for the rest of the minute";
        let id = plan.propose(long, None, &author()).unwrap();
        let block = plan.block(&id).unwrap();
        assert_eq!(block.text, "Tell the user how many attempts are left before the account…");
        assert_eq!(block.details, long);
        assert_eq!(plan.propose(long, None, &author()), None);
    }

    #[test]
    fn blocks_are_added_joined_and_removed() {
        let mut plan = Plan::default();
        assert!(plan.only, "agents are held to the plan unless the user says otherwise");
        let a = plan.add(Kind::Step, "", None, None).unwrap();
        assert_eq!((a.as_str(), plan.block(&a).unwrap().text.as_str()), ("b1", "New step"));
        let b = plan.add(Kind::Ask, " Over the\n limit? ", Some(&a), None).unwrap();
        assert_eq!(plan.block(&b).unwrap().text, "Over the limit?");
        let c = plan.add(Kind::Step, "Respond 429", Some(&b), Some("yes")).unwrap();
        let note = plan.add(Kind::Note, &"n".repeat(200), None, None).unwrap();
        // A second block after the same one goes behind everything that already follows it.
        let side = plan.add(Kind::Step, "Count the attempt", Some(&a), None).unwrap();
        let order: Vec<&str> = plan.blocks.iter().map(|block| block.id.as_str()).collect();
        assert_eq!(order, vec![a.as_str(), b.as_str(), c.as_str(), side.as_str(), note.as_str()]);

        // What cannot be.
        assert!(plan.add(Kind::Step, &"x".repeat(61), None, None).unwrap_err().to_string().contains("at most 60"));
        assert!(plan.add(Kind::Note, &"x".repeat(201), None, None).is_err());
        assert!(plan.add(Kind::Step, "After a note", Some(&note), None).unwrap_err().to_string().contains("note"));
        assert!(plan.add(Kind::Note, "A note after a step", Some(&a), None).is_err());
        assert!(plan.add(Kind::Step, "Nowhere", Some("b99"), None).is_err());
        assert!(plan.edit(&a, Some("   "), None).unwrap_err().to_string().contains("name"));
        assert!(plan.connect(&a, &a, None).is_err() && plan.connect(&a, &note, None).is_err());
        assert!(plan.connect(&a, &c, Some("a label that is too long")).is_err());
        assert!(plan.disconnect(&c, &a).is_err());

        // Joining twice changes the label; it does not draw a second arrow.
        plan.connect(&b, &c, Some("no")).unwrap();
        assert_eq!(plan.arrows.iter().filter(|arrow| arrow.from == b && arrow.to == c).count(), 1);
        assert_eq!(plan.arrows.iter().find(|arrow| arrow.to == c).unwrap().label.as_deref(), Some("no"));
        plan.disconnect(&b, &c).unwrap();
        plan.connect(&b, &c, Some("yes")).unwrap();

        // Removing a block from the middle of a line leaves the line whole, with the label it was reached by.
        plan.remove(&side).unwrap();
        plan.connect(&a, &b, Some("then")).unwrap();
        plan.remove(&b).unwrap();
        assert_eq!(plan.arrows, vec![Arrow { from: a.clone(), to: c.clone(), label: Some("then".into()) }]);
        // A block at the end of a line takes its arrow with it.
        plan.remove(&c).unwrap();
        assert!(plan.arrows.is_empty());
        assert!(plan.remove(&c).is_err());
        // An id is never given twice, also after its block is gone.
        assert_eq!(plan.add(Kind::Step, "Again", None, None).unwrap(), "b6");

        let mut full = Plan::default();
        for n in 0..MAX_BLOCKS {
            full.add(Kind::Step, &format!("Step {n}"), None, None).unwrap();
        }
        assert!(
            full.add(Kind::Step, "One too many", None, None).unwrap_err().to_string().contains("at most 60 blocks")
        );
        assert_eq!(full.propose("One too many", None, &author()), None);
    }

    #[test]
    fn propose_lines_are_read_out_of_an_answer() {
        let answer = "Done. One file changed.\n\nPROPOSE: Reset the count after a correct password\n- PROPOSE: <Log the blocked attempt>\n**PROPOSE:** Tell the form how long to wait\n`PROPOSE: Reset the count after a correct password`\nPROPOSE:\nI would also propose: nothing.\npropose: not this";
        assert_eq!(
            proposals_in(answer),
            vec![
                "Reset the count after a correct password",
                "Log the blocked attempt",
                "Tell the form how long to wait"
            ]
        );
        assert!(proposals_in("The limiter allows five attempts a minute.").is_empty());
        let many: String = (0..9).map(|n| format!("PROPOSE: Block {n}\n")).collect();
        assert_eq!(proposals_in(&many).len(), MAX_PROPOSALS);
    }

    #[test]
    fn a_draft_arrives_as_proposed_blocks_and_a_drawing_as_what_was_there_before() {
        let answer = "Here is the flow.\n\n```flow\nstep a: Request to POST /login\nask b: Over the limit? @ src/middleware/rateLimit.ts:3\nstep* c: Respond 429 @ src/middleware/rateLimit.ts:3\nstep d: Check the password\na -> b\nb -> c: yes\nb -> d: no\n```";
        let mut plan = Plan::default();
        let own = plan.add(Kind::Step, "My own block", None, None).unwrap();
        plan.problem = Some("An older problem".into());
        plan.draft_arrived(answer, false, &author());
        assert_eq!(plan.problem, None);
        let drafted: Vec<(&str, Kind, State)> =
            plan.blocks.iter().skip(1).map(|block| (block.text.as_str(), block.kind, block.state)).collect();
        assert_eq!(
            drafted,
            vec![
                ("Request to POST /login", Kind::Step, State::Proposed),
                ("Over the limit?", Kind::Ask, State::Proposed),
                ("Respond 429", Kind::Step, State::Proposed),
                ("Check the password", Kind::Step, State::Proposed),
            ]
        );
        assert_eq!(plan.blocks[1].from.as_deref(), Some("draft"));
        assert_eq!(plan.arrows[1], Arrow { from: "b3".into(), to: "b4".into(), label: Some("yes".into()) });

        // A second draft replaces what is left of the first; what the user said yes to stays.
        plan.accept(Some("b2")).unwrap();
        plan.draft_arrived(answer, false, &author());
        let texts: Vec<&str> = plan.blocks.iter().map(|block| block.text.as_str()).collect();
        assert_eq!(texts.iter().filter(|text| **text == "Request to POST /login").count(), 2);
        assert_eq!(plan.blocks.len(), 6);
        assert!(plan.block(&own).is_ok());

        // Drawn from the code: there before, each with its place.
        plan.draft_arrived(answer, true, &author());
        let drawn: Vec<&Block> = plan.blocks.iter().filter(|block| block.from.as_deref() == Some("code")).collect();
        assert_eq!(drawn.len(), 4);
        assert!(drawn.iter().all(|block| block.state == State::Before));
        assert_eq!(drawn[1].files, vec![Place { path: "src/middleware/rateLimit.ts".into(), line: Some(3), to: None }]);
        assert_eq!(plan.counts().proposed, 4);

        // An answer in another shape is called that, and adds nothing.
        let before = plan.blocks.len();
        plan.draft_arrived("The limiter allows five attempts a minute.", false, &author());
        assert_eq!(
            plan.problem.as_deref(),
            Some(
                "Practice agent answered, but not in the flow format, so there is nothing to add to the plan. The answer has no flow block."
            )
        );
        assert_eq!(plan.blocks.len(), before);
    }

    #[test]
    fn the_messages_to_the_agent_say_what_to_do_and_nothing_else() {
        let (mut plan, [_, _, refuse, _]) = login_plan();
        plan.edit(&refuse, None, Some("Say in Retry-After how long to wait.")).unwrap();
        let message = build_message(&plan, plan.block(&refuse).unwrap());
        assert_eq!(
            message,
            "Build one block of this task's plan, and nothing else. The plan, in the editor's flow format (`*` marks a block that is built already):\n\n```flow\nstep b1: Request to POST /login\nask b2: Over the limit?\nstep b3: Respond 429\nstep b4: Check the password\nb1 -> b2\nb2 -> b3: yes\nb2 -> b4: no\n```\n\nBuild only this block: `b3`, “Respond 429”.\nWhat it should do: Say in Retry-After how long to wait.\n\nIf you think a block is missing from the plan, do not build it. End your answer with one line `PROPOSE: <what the block should do>` for each such block.\n\nIf a test shows that this block works, name it in one more line: `PROVED BY: <the name of the test>`."
        );
        let draft = draft_message("Add rate limiting to /login\n\nFive attempts a minute.", false);
        assert!(draft.starts_with("Draft a plan for this task as a flow chart"), "{draft}");
        assert!(draft.contains(
            "The task, as it was first described:\n> Add rate limiting to /login\n> \n> Five attempts a minute.\n\n"
        ));
        assert!(draft.contains(
            "in exactly this format, so the editor can draw it:\n\n```flow\nstep a: The first thing that happens\n"
        ));
        assert!(!draft.contains("@ path:line"));
        // The pattern an agent is shown is itself a flow the editor can read.
        assert!(parse_flow(&draft).is_ok() && parse_flow(&draft_message("", true)).is_ok());
        let draw = draft_message("", true);
        assert!(draw.starts_with("Draw the code this task is about as a flow chart") && draw.contains("@ path:line"));
        assert!(!draw.contains("as it was first described"));
    }

    #[test]
    fn the_files_of_a_turn_are_those_its_finished_edits_name() {
        let root = Path::new("/work/shop");
        let call = |id: &str, kind: &str, status: &str, path: &str| json!({ "type": "update", "update": { "sessionUpdate": "tool_call", "toolCallId": id, "kind": kind, "status": status, "locations": [{ "path": path }] } });
        let update = |id: &str, status: &str| json!({ "type": "update", "update": { "sessionUpdate": "tool_call_update", "toolCallId": id, "status": status } });
        let events = vec![
            json!({ "type": "user_message", "text": "An earlier message" }),
            call("old", "edit", "completed", "/work/shop/src/old.ts"),
            json!({ "type": "user_message", "text": "Build only this block" }),
            call("read", "read", "in_progress", "/work/shop/src/read.ts"),
            update("read", "completed"),
            call("edit", "edit", "pending", "/work/shop/src/b.ts"),
            update("edit", "completed"),
            call("refused", "edit", "pending", "/work/shop/src/refused.ts"),
            update("refused", "failed"),
            call("outside", "edit", "completed", "/etc/hosts"),
            // A call that says only later what it is: its diff makes it an edit.
            call("late", "other", "pending", "/work/shop/src/a.ts"),
            json!({ "type": "update", "update": { "sessionUpdate": "tool_call_update", "toolCallId": "late", "status": "completed", "content": [{ "type": "diff", "path": "/work/shop/src/a.ts" }] } }),
            json!({ "type": "update", "update": { "sessionUpdate": "agent_message_chunk", "content": { "text": "Done. " } } }),
            json!({ "type": "update", "update": { "sessionUpdate": "agent_message_chunk", "content": { "text": "One file changed." } } }),
            json!({ "type": "turn_end", "stopReason": "end_turn" }),
        ];
        let turn = newest_turn(&events);
        assert_eq!(turn.len(), events.len() - 3);
        assert_eq!(answer_of(turn), "Done. One file changed.");
        assert_eq!(edited_in(turn, root).into_iter().collect::<Vec<_>>(), vec!["src/a.ts", "src/b.ts"]);
    }

    #[test]
    fn a_plan_is_kept_as_json_and_an_older_one_still_reads() {
        let (mut plan, [_, _, refuse, _]) = login_plan();
        plan.block_mut(&refuse).unwrap().state = State::Building;
        plan.build_ended(&refuse, true, vec![place("src/a.ts")], vec![], &author());
        let kept = serde_json::to_string(&plan).unwrap();
        assert_eq!(serde_json::from_str::<Plan>(&kept).unwrap(), plan);
        let value: Value = serde_json::from_str(&kept).unwrap();
        assert_eq!(value["blocks"][2]["state"], "built");
        assert_eq!(value["blocks"][2]["byName"], "Practice agent");
        assert_eq!(value["blocks"][2]["files"], json!([{ "path": "src/a.ts", "line": null }]));
        assert_eq!(value["blocks"][1]["kind"], "ask");
        // What a later version adds is filled in when it is missing.
        let old: Plan =
            serde_json::from_str(r#"{"blocks":[{"id":"b1","kind":"step","text":"One","state":"planned"}]}"#).unwrap();
        assert!(old.only && old.arrows.is_empty() && old.blocks[0].details.is_empty());
    }
}
