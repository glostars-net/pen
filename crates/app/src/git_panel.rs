//! Git status surfaced in the file tree: a snapshot published by a Tokio
//! task (`main.rs`), read by the UI thread. No color — the label uses the
//! theme's faint token, and an untracked file reads "new".

/// The latest known git status, or empty when the project isn't a git
/// repository (or none is open yet).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitSnapshot {
    pub entries: Vec<pen_git::StatusEntry>,
}

/// `"+2 −1"` for a modification, `"new"` for an untracked file, `""` when
/// there's nothing to show. Uses a minus sign (U+2212), not a hyphen.
pub fn status_label(entry: &pen_git::StatusEntry) -> String {
    if !entry.tracked {
        return "new".to_string();
    }
    match (entry.added, entry.removed) {
        (0, 0) => String::new(),
        (added, removed) => format!("+{added} \u{2212}{removed}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_label_uses_the_ink_counts() {
        let e = pen_git::StatusEntry {
            rel: "a.rs".into(),
            added: 2,
            removed: 1,
            tracked: true,
        };
        assert_eq!(status_label(&e), "+2 \u{2212}1");
    }

    #[test]
    fn status_label_marks_untracked_files_as_new() {
        let e = pen_git::StatusEntry {
            rel: "b.rs".into(),
            added: 0,
            removed: 0,
            tracked: false,
        };
        assert_eq!(status_label(&e), "new");
    }
}
