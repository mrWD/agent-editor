//! Who wrote which line since the last review.
//!
//! For every file that changed in a task the core keeps three things: the text
//! the user last accepted (the base), the text it last saw on disk, and one
//! author per line of that text. Each time the file changes, the lines that
//! stayed keep their author and the new lines get the author of the change.
//! This file holds the arithmetic; reading and storing is done elsewhere.

use serde::Serialize;
use similar::{DiffOp, TextDiff};
use std::time::Duration;

/// Comparing two versions of a file may not take longer than this. Past the
/// limit the comparison gets rougher, never wrong.
const DIFF_TIMEOUT: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    /// The line is as it was at the last review.
    Base,
    /// The agent wrote it and the user has not reviewed it.
    Agent,
    /// The user wrote it.
    You,
}

impl Author {
    fn letter(self) -> char {
        match self {
            Author::Base => 'b',
            Author::Agent => 'a',
            Author::You => 'y',
        }
    }

    fn from_letter(letter: char) -> Author {
        match letter {
            'a' => Author::Agent,
            'y' => Author::You,
            _ => Author::Base,
        }
    }
}

/// Authors as stored: one letter per line.
pub fn encode(authors: &[Author]) -> String {
    authors.iter().map(|a| a.letter()).collect()
}

pub fn decode(letters: &str) -> Vec<Author> {
    letters.chars().map(Author::from_letter).collect()
}

/// Splits text into lines, each with its line ending, so joining gives the text back.
pub fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn ops(old: &[&str], new: &[&str]) -> Vec<DiffOp> {
    TextDiff::configure().timeout(DIFF_TIMEOUT).diff_slices(old, new).ops().to_vec()
}

/// The authors of `new`, given the authors of `old` and who made the change.
pub fn carry(old: &str, old_authors: &[Author], new: &str, who: Author) -> Vec<Author> {
    let old_lines = lines(old);
    let new_lines = lines(new);
    let author_of = |index: usize| old_authors.get(index).copied().unwrap_or(Author::Base);
    let mut authors = Vec::with_capacity(new_lines.len());
    for op in ops(&old_lines, &new_lines) {
        match op {
            DiffOp::Equal { old_index, len, .. } => {
                authors.extend((old_index..old_index + len).map(author_of));
            }
            DiffOp::Insert { new_len, .. } | DiffOp::Replace { new_len, .. } => {
                authors.extend(std::iter::repeat_n(who, new_len));
            }
            DiffOp::Delete { .. } => {}
        }
    }
    authors
}

/// Clears the marks of lines that read as they did at the last review.
/// A line changed and then changed back is nobody's change.
pub fn settle(base: &str, current: &str, authors: Vec<Author>) -> Vec<Author> {
    let base_lines = lines(base);
    let current_lines = lines(current);
    let mut settled = authors;
    settled.resize(current_lines.len(), Author::Base);
    for op in ops(&base_lines, &current_lines) {
        if let DiffOp::Equal { new_index, len, .. } = op {
            for author in &mut settled[new_index..new_index + len] {
                *author = Author::Base;
            }
        }
    }
    settled
}

/// Puts back what the agent changed and keeps what the user changed.
///
/// Every changed block between `base` and `seen` is looked at on its own. A
/// block without a line of the user's goes back to the base. A block with one
/// goes back line by line where its lines can be paired with the base, and
/// stays as it is where they cannot. Returns the new text and its authors.
pub fn undo_agent(base: &str, seen: &str, authors: &[Author]) -> (String, Vec<Author>) {
    let base_lines = lines(base);
    let seen_lines = lines(seen);
    let author_of = |index: usize| authors.get(index).copied().unwrap_or(Author::Base);
    let mut text = String::with_capacity(base.len().max(seen.len()));
    let mut result = Vec::new();

    let keep_seen = |text: &mut String, result: &mut Vec<Author>, from: usize, len: usize| {
        for (offset, line) in seen_lines[from..from + len].iter().enumerate() {
            text.push_str(line);
            result.push(author_of(from + offset));
        }
    };
    let take_base = |text: &mut String, result: &mut Vec<Author>, from: usize, len: usize| {
        for line in &base_lines[from..from + len] {
            text.push_str(line);
            result.push(Author::Base);
        }
    };

    for op in ops(&base_lines, &seen_lines) {
        match op {
            DiffOp::Equal { new_index, len, .. } => keep_seen(&mut text, &mut result, new_index, len),
            DiffOp::Delete { old_index, old_len, .. } => take_base(&mut text, &mut result, old_index, old_len),
            DiffOp::Insert { new_index, new_len, .. } => {
                let yours = (new_index..new_index + new_len).any(|i| author_of(i) == Author::You);
                if yours {
                    keep_seen(&mut text, &mut result, new_index, new_len);
                }
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                let yours = (new_index..new_index + new_len).any(|i| author_of(i) == Author::You);
                if !yours {
                    take_base(&mut text, &mut result, old_index, old_len);
                } else if old_len == new_len {
                    // Same number of lines before and after: each line has its own
                    // counterpart, so the agent's lines go back one by one.
                    for offset in 0..new_len {
                        if author_of(new_index + offset) == Author::You {
                            keep_seen(&mut text, &mut result, new_index + offset, 1);
                        } else {
                            take_base(&mut text, &mut result, old_index + offset, 1);
                        }
                    }
                } else {
                    keep_seen(&mut text, &mut result, new_index, new_len);
                }
            }
        }
    }
    (text, result)
}

/// A stretch of lines by one author. Lines count from 1; `to` is the last line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Run {
    pub from: usize,
    pub to: usize,
    pub who: Author,
}

/// The stretches of lines that are not as they were at the last review.
pub fn runs(authors: &[Author]) -> Vec<Run> {
    let mut list: Vec<Run> = Vec::new();
    for (index, author) in authors.iter().enumerate() {
        if *author == Author::Base {
            continue;
        }
        let line = index + 1;
        match list.last_mut() {
            Some(last) if last.who == *author && last.to + 1 == line => last.to = line,
            _ => list.push(Run { from: line, to: line, who: *author }),
        }
    }
    list
}

pub fn count(authors: &[Author], who: Author) -> usize {
    authors.iter().filter(|a| **a == who).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use Author::{Agent, Base, You};

    #[test]
    fn new_lines_get_the_author_of_the_change_and_old_lines_keep_theirs() {
        let base = "one\ntwo\nthree\n";
        let after_agent = "one\ntwo and a half\nthree\nfour\n";
        let authors = carry(base, &[Base, Base, Base], after_agent, Agent);
        assert_eq!(authors, vec![Base, Agent, Base, Agent]);

        // The user then changes one of the agent's lines and adds one at the top.
        let after_you = "zero\none\ntwo and a half\nthree\nFOUR\n";
        let authors = carry(after_agent, &authors, after_you, You);
        assert_eq!(authors, vec![You, Base, Agent, Base, You]);
        assert_eq!(
            runs(&authors),
            vec![
                Run { from: 1, to: 1, who: You },
                Run { from: 3, to: 3, who: Agent },
                Run { from: 5, to: 5, who: You },
            ]
        );
    }

    #[test]
    fn a_line_changed_back_is_nobodys_change() {
        let base = "one\ntwo\nthree\n";
        // The agent changed "two", then someone put it back and changed "three".
        let now = "one\ntwo\nTHREE\n";
        assert_eq!(settle(base, now, vec![Base, You, You]), vec![Base, Base, You]);
        // Authors that are too few or too many are brought to the number of lines.
        assert_eq!(settle("a\n", "a\nb\n", vec![]), vec![Base, Base]);
    }

    #[test]
    fn a_file_without_a_last_newline_is_handled() {
        let authors = carry("a\nb", &[Base, Base], "a\nb\nc", Agent);
        // "b" gained a line ending, so it counts as changed along with the new line.
        assert_eq!(authors, vec![Base, Agent, Agent]);
        assert_eq!(lines("a\nb\nc").concat(), "a\nb\nc");
    }

    #[test]
    fn undo_puts_back_the_agents_blocks_and_keeps_yours() {
        let base = "one\ntwo\nthree\nfour\nfive\n";
        // The agent changed "two" and removed "four"; the user changed "five".
        let seen = "one\nTWO\nthree\nfive!\n";
        let authors = vec![Base, Agent, Base, You];
        let (text, authors) = undo_agent(base, seen, &authors);
        assert_eq!(text, "one\ntwo\nthree\nfive!\n");
        assert_eq!(authors, vec![Base, Base, Base, You]);
    }

    #[test]
    fn undo_separates_your_line_from_the_agents_line_right_next_to_it() {
        let base = "a\n  windowMs: 60,\n  limit: 5,\nz\n";
        let seen = "a\n  windowMs: 60, // one minute\n  limit: limits.login,\nz\n";
        let (text, authors) = undo_agent(base, seen, &[Base, You, Agent, Base]);
        assert_eq!(text, "a\n  windowMs: 60, // one minute\n  limit: 5,\nz\n");
        assert_eq!(authors, vec![Base, You, Base, Base]);
    }

    #[test]
    fn undo_leaves_a_block_alone_when_your_lines_cannot_be_told_from_the_agents() {
        let base = "a\nold\nz\n";
        let seen = "a\nmine\ntheirs\nmore of theirs\nz\n";
        let before = vec![Base, You, Agent, Agent, Base];
        let (text, authors) = undo_agent(base, seen, &before);
        assert_eq!(text, seen);
        assert_eq!(authors, before);
    }

    #[test]
    fn undo_of_a_file_the_agent_made_leaves_only_your_lines() {
        let seen = "a\nb\nc\n";
        let (text, authors) = undo_agent("", seen, &[Agent, Agent, Agent]);
        assert_eq!(text, "");
        assert!(authors.is_empty());
    }

    #[test]
    fn authors_survive_storing() {
        let authors = vec![Base, Agent, You, Base];
        assert_eq!(encode(&authors), "bayb");
        assert_eq!(decode("bayb"), authors);
    }
}
