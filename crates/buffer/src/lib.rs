//! Pen's text buffer: a `ropey` rope with inverse-edit undo.
//!
//! Indexes are Unicode scalar (char) indexes, matching `ropey`. The buffer
//! knows nothing about files, themes or the UI; `crates/project` does disk,
//! `crates/app` does drawing.

use std::collections::VecDeque;

/// Undo entries kept; the oldest drops past this.
const UNDO_CAP: usize = 256;

/// One stored edit: the inverse of what the user did, applied to go back.
#[derive(Debug)]
enum Edit {
    /// Put `text` back at `at` (the inverse of a delete).
    Insert { at: usize, text: String },
    /// Remove `len` chars at `at` (the inverse of an insert).
    Delete { at: usize, len: usize },
}

/// A text buffer on a rope, with inverse-edit undo. No grouping: one edit,
/// one entry.
pub struct Buffer {
    rope: ropey::Rope,
    undo: VecDeque<Edit>,
    redo: Vec<Edit>,
    dirty: bool,
}

impl Buffer {
    #[allow(clippy::should_implement_trait)] // the plan's constructor name; not `FromStr`, which parses
    pub fn from_str(text: &str) -> Self {
        Buffer { rope: ropey::Rope::from_str(text), undo: VecDeque::new(), redo: Vec::new(), dirty: false }
    }

    /// Insert `text` at a char index. Pushes one undo entry and clears redo.
    pub fn insert(&mut self, char_idx: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        self.rope.insert(char_idx, text);
        self.push_undo(Edit::Delete { at: char_idx, len: text.chars().count() });
        self.redo.clear();
        self.dirty = true;
    }

    /// Delete a char range. Pushes one undo entry and clears redo.
    pub fn delete(&mut self, range: std::ops::Range<usize>) {
        if range.is_empty() {
            return;
        }
        let removed: String = self.rope.slice(range.clone()).into();
        self.rope.remove(range.clone());
        self.push_undo(Edit::Insert { at: range.start, text: removed });
        self.redo.clear();
        self.dirty = true;
    }

    /// Apply the newest undo entry. False when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo.pop_back() else { return false };
        let inverse = self.apply(edit);
        self.redo.push(inverse);
        self.dirty = true;
        true
    }

    /// Re-apply the newest undone edit. False when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo.pop() else { return false };
        let inverse = self.apply(edit);
        self.undo.push_back(inverse);
        self.dirty = true;
        true
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// One line's text, without its line break.
    pub fn line(&self, line_idx: usize) -> String {
        let line = self.rope.line(line_idx);
        let mut s: String = line.into();
        if s.ends_with('\n') {
            s.pop();
            if s.ends_with('\r') {
                s.pop();
            }
        }
        s
    }

    /// Whether the text changed since [`Buffer::mark_saved`] (or opening).
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clears dirty. Undo history stays.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Applies an edit to the rope and returns its inverse.
    fn apply(&mut self, edit: Edit) -> Edit {
        match edit {
            Edit::Insert { at, text } => {
                self.rope.insert(at, &text);
                Edit::Delete { at, len: text.chars().count() }
            }
            Edit::Delete { at, len } => {
                let removed: String = self.rope.slice(at..at + len).into();
                self.rope.remove(at..at + len);
                Edit::Insert { at, text: removed }
            }
        }
    }

    fn push_undo(&mut self, edit: Edit) {
        if self.undo.len() == UNDO_CAP {
            self.undo.pop_front();
        }
        self.undo.push_back(edit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_delete_undo_redo_round_trip() {
        let mut b = Buffer::from_str("ac");
        b.insert(1, "b");
        assert_eq!(b.text(), "abc");
        assert!(b.is_dirty());
        b.delete(1..2);
        assert_eq!(b.text(), "ac");
        assert!(b.undo());
        assert_eq!(b.text(), "abc");
        assert!(b.redo());
        assert_eq!(b.text(), "ac");
    }

    #[test]
    fn mark_saved_clears_dirty_only() {
        let mut b = Buffer::from_str("a");
        b.insert(1, "b");
        b.mark_saved();
        assert!(!b.is_dirty());
        assert!(b.undo());
        assert_eq!(b.text(), "a");
        assert!(b.is_dirty());
    }

    #[test]
    fn line_split_is_char_based() {
        let b = Buffer::from_str("é\nß");
        assert_eq!(b.len_lines(), 2);
        assert_eq!(b.line(0), "é");
        assert_eq!(b.len_chars(), 3);
    }
}
