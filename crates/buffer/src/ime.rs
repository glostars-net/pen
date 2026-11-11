//! The IME commit boundary: preedit text shows in the rope while composing,
//! but only a commit is a real edit. Undo never replays a preedit, and a
//! cancelled composition leaves no trace.
//!
//! Platform events (UTF-16 decode, candidate windows) are the view's job in
//! `crates/app`; this module only edits the rope.

use crate::Buffer;

/// An active composition: where it starts and what it currently shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preedit {
    /// Char index in the rope where the preedit begins (not including it).
    pub start: usize,
    pub text: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ImeError {
    #[error("preedit start {start} is past the end of the buffer ({len} chars)")]
    OutOfBounds { start: usize, len: usize },
}

/// Replace or commit the composition.
///
/// `commit: false` shows `replacement` as the preedit without touching undo
/// or dirty; an empty `replacement` cancels. `commit: true` removes the
/// preedit and inserts `replacement` as one normal edit (one undo entry).
pub fn apply_ime(
    buffer: &mut Buffer,
    preedit: &mut Option<Preedit>,
    replacement: &str,
    commit: bool,
) -> Result<(), ImeError> {
    let was_dirty = buffer.is_dirty();
    // Take the current preedit out of the rope, silently: it was never an edit.
    let start = match preedit.take() {
        Some(p) => {
            buffer.replace_silent(p.start..p.start + p.text.chars().count(), "");
            p.start
        }
        None => buffer.len_chars(),
    };
    if start > buffer.len_chars() {
        return Err(ImeError::OutOfBounds { start, len: buffer.len_chars() });
    }
    if commit {
        // One normal edit: dirty, one undo entry.
        buffer.insert(start, replacement);
    } else if !replacement.is_empty() {
        buffer.replace_silent(start..start, replacement);
        *preedit = Some(Preedit { start, text: replacement.to_owned() });
        buffer.set_dirty(was_dirty);
    } else {
        // Cancelled: the rope is back to its pre-composition text.
        buffer.set_dirty(was_dirty);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preedit_is_not_in_the_undo_stack() {
        let mut b = Buffer::from_str("a");
        let mut pre = None;
        apply_ime(&mut b, &mut pre, "ん", false).unwrap();
        assert_eq!(b.text(), "aん");
        assert!(!b.undo());
        apply_ime(&mut b, &mut pre, "", false).unwrap();
        assert_eq!(b.text(), "a");
    }

    #[test]
    fn commit_is_one_undo_entry() {
        let mut b = Buffer::from_str("");
        let mut pre = None;
        apply_ime(&mut b, &mut pre, "に", false).unwrap();
        apply_ime(&mut b, &mut pre, "日", true).unwrap();
        assert_eq!(b.text(), "日");
        assert!(pre.is_none());
        assert!(b.undo());
        assert_eq!(b.text(), "");
    }

    #[test]
    fn preedit_keeps_dirty_as_it_was() {
        let mut b = Buffer::from_str("a");
        let mut pre = None;
        apply_ime(&mut b, &mut pre, "か", false).unwrap();
        assert!(!b.is_dirty(), "a preedit is not an edit");
        apply_ime(&mut b, &mut pre, "家", true).unwrap();
        assert!(b.is_dirty(), "a commit is");
    }
}
