//! Pen's text buffer: a `ropey` rope with inverse-edit undo.
//!
//! Indexes are Unicode scalar (char) indexes, matching `ropey`. The buffer
//! knows nothing about files, themes or the UI; `crates/project` does disk,
//! `crates/app` does drawing.

mod ime;
pub use ime::*;

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

/// One visible line, for the editor's viewport. Only the lines on screen are
/// ever turned into `String`s; the rest of the file stays in the rope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSlice {
    pub line_idx: usize,
    pub text: String,
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
        Buffer {
            rope: ropey::Rope::from_str(text),
            undo: VecDeque::new(),
            redo: Vec::new(),
            dirty: false,
        }
    }

    /// Read a whole file. Clean (not dirty), empty undo. UTF-8 only: bytes
    /// that aren't valid UTF-8 are an error, as are read failures.
    pub fn from_reader(reader: impl std::io::Read) -> Result<Self, std::io::Error> {
        let rope = ropey::Rope::from_reader(reader)?;
        Ok(Buffer {
            rope,
            undo: VecDeque::new(),
            redo: Vec::new(),
            dirty: false,
        })
    }

    /// At most `count` lines starting at `first_line`, fewer at end of file.
    /// Walks only the requested range; it never shapes or copies the file.
    pub fn viewport(&self, first_line: usize, count: usize) -> Vec<LineSlice> {
        let end = (first_line + count).min(self.len_lines());
        (first_line..end)
            .map(|line_idx| LineSlice {
                line_idx,
                text: self.line(line_idx),
            })
            .collect()
    }

    /// Insert `text` at a char index. Pushes one undo entry and clears redo.
    pub fn insert(&mut self, char_idx: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        self.rope.insert(char_idx, text);
        self.push_undo(Edit::Delete {
            at: char_idx,
            len: text.chars().count(),
        });
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
        self.push_undo(Edit::Insert {
            at: range.start,
            text: removed,
        });
        self.redo.clear();
        self.dirty = true;
    }

    /// Apply the newest undo entry. False when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo.pop_back() else {
            return false;
        };
        let inverse = self.apply(edit);
        self.redo.push(inverse);
        self.dirty = true;
        true
    }

    /// Re-apply the newest undone edit. False when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo.pop() else {
            return false;
        };
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

    /// Which line `char_idx` sits on, in `O(log N)` without copying the rope.
    pub fn char_to_line(&self, char_idx: usize) -> usize {
        self.rope.char_to_line(char_idx.min(self.rope.len_chars()))
    }

    /// First char index of `line_idx`, in `O(log N)`.
    pub fn line_to_char(&self, line_idx: usize) -> usize {
        self.rope.line_to_char(line_idx.min(self.rope.len_lines()))
    }

    /// Char count of `line_idx` without its trailing `\n` or `\r\n`, in `O(log N)`.
    pub fn line_len_chars(&self, line_idx: usize) -> usize {
        let slice = self.rope.line(line_idx);
        let n = slice.len_chars();
        if n > 0 && slice.char(n - 1) == '\n' {
            if n > 1 && slice.char(n - 2) == '\r' {
                n - 2
            } else {
                n - 1
            }
        } else {
            n
        }
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

    /// Literal case-sensitive scalar search starting at `from_char`, wrapping
    /// once around the top of the buffer. No regex.
    pub fn find_next(&self, from_char: usize, query: &str) -> Option<std::ops::Range<usize>> {
        if query.is_empty() {
            return None;
        }
        let qlen = query.chars().count();
        let len = self.rope.len_chars();
        let from = from_char.min(len);

        let tail: String = self.rope.slice(from..len).into();
        if let Some(byte_idx) = tail.find(query) {
            let start = from + tail[..byte_idx].chars().count();
            return Some(start..start + qlen);
        }
        if from > 0 {
            let head_end = (from + qlen.saturating_sub(1)).min(len);
            let head: String = self.rope.slice(0..head_end).into();
            if let Some(byte_idx) = head.find(query) {
                let start = head[..byte_idx].chars().count();
                return Some(start..start + qlen);
            }
        }
        None
    }

    /// Applies an edit to the rope and returns its inverse.
    fn apply(&mut self, edit: Edit) -> Edit {
        match edit {
            Edit::Insert { at, text } => {
                self.rope.insert(at, &text);
                Edit::Delete {
                    at,
                    len: text.chars().count(),
                }
            }
            Edit::Delete { at, len } => {
                let removed: String = self.rope.slice(at..at + len).into();
                self.rope.remove(at..at + len);
                Edit::Insert { at, text: removed }
            }
        }
    }

    /// Replace a char range without touching undo, redo or dirty. For IME
    /// preedit text only (`ime::apply_ime`), which shows in the rope but was
    /// never an edit.
    pub(crate) fn replace_silent(&mut self, range: std::ops::Range<usize>, text: &str) {
        if !range.is_empty() {
            self.rope.remove(range.clone());
        }
        if !text.is_empty() {
            self.rope.insert(range.start, text);
        }
    }

    /// For `ime::apply_ime`: a preedit must leave dirty as it found it.
    pub(crate) fn set_dirty(&mut self, dirty: bool) {
        self.dirty = dirty;
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
    fn viewport_does_not_return_the_whole_file() {
        let mut raw = String::new();
        for i in 0..10_000 {
            raw.push_str(&format!("line {i}\n"));
        }
        let b = Buffer::from_str(&raw);
        let window = b.viewport(100, 40);
        assert_eq!(window.len(), 40);
        assert_eq!(window[0].line_idx, 100);
        assert_eq!(window[0].text, "line 100");
    }

    #[test]
    fn from_reader_matches_from_str() {
        let b = Buffer::from_reader("a\nb".as_bytes()).unwrap();
        assert_eq!(b.text(), "a\nb");
        assert!(!b.is_dirty());
    }

    /// The large-file budget from the plan: a 10 MB open finishes under
    /// 100 ms. Run in release: `cargo test -p buffer --release -- --ignored --nocapture`.
    #[test]
    #[ignore = "release-only budget test"]
    fn ten_megabyte_open_is_under_100ms() {
        let line = "let value = compute(input, options); // a plausible line of code\n";
        let mut raw = String::with_capacity(10 * 1024 * 1024 + line.len());
        while raw.len() < 10 * 1024 * 1024 {
            raw.push_str(line);
        }
        let start = std::time::Instant::now();
        let b = Buffer::from_reader(raw.as_bytes()).unwrap();
        let took = start.elapsed();
        println!("10 MB from_reader: {took:?} ({} lines)", b.len_lines());
        assert!(
            took.as_millis() < 100,
            "10 MB open took {took:?}, budget is 100 ms"
        );
    }

    /// Task 11 keystroke budget: inserting one char into the middle of a 10 MB
    /// rope, slicing a 40-line viewport, and syntax-highlighting those 40 lines
    /// finishes in under 2 ms in release.
    #[test]
    #[ignore = "release-only budget test"]
    fn keystroke_viewport_and_highlight_on_10mb_is_under_2ms() {
        let line = "pub fn compute(input: usize, factor: usize) -> usize { input * factor + 1 }\n";
        let mut raw = String::with_capacity(10 * 1024 * 1024 + line.len());
        while raw.len() < 10 * 1024 * 1024 {
            raw.push_str(line);
        }
        let mut b = Buffer::from_reader(raw.as_bytes()).unwrap();
        let theme = highlight::ink();
        // Warm the static lexer lookup table once (as opening the file already does).
        let _ = highlight::highlight_lines(Some("rust"), &["fn warm() {}".to_owned()], &theme);

        let mid_line = b.len_lines() / 2;
        let mid_char = b.line_to_char(mid_line) + 12;

        let start = std::time::Instant::now();
        b.insert(mid_char, "x");
        let vp = b.viewport(mid_line, 40);
        let lines: Vec<String> = vp.into_iter().map(|s| s.text).collect();
        let spans = highlight::highlight_lines(Some("rust"), &lines, &theme);
        let took = start.elapsed();

        println!(
            "10 MB keystroke + 40-line viewport + highlight: {took:?} ({} spans on edited line)",
            spans[0].len()
        );
        assert!(
            took.as_micros() < 2_000,
            "keystroke + viewport + highlight took {took:?}, budget is 2 ms"
        );
    }

    #[test]
    fn find_next_is_char_indexed() {
        let b = Buffer::from_str("éé needle");
        assert_eq!(b.find_next(0, "needle"), Some(3..9));
    }

    #[test]
    fn line_split_is_char_based() {
        let b = Buffer::from_str("é\nß");
        assert_eq!(b.len_lines(), 2);
        assert_eq!(b.line(0), "é");
        assert_eq!(b.len_chars(), 3);
    }
}
