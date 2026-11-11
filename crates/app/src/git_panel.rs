//! Git panel & status helpers (`crates/app/src/git_panel.rs`).
//!
//! All data here is read-only on the UI thread: a Tokio worker refreshes
//! [`GitSnapshot`] off-thread and publishes it over a `watch` channel.

use highlight::Theme;
use rune::{Cursor, El, ScrollSpec, TextStyle, Transition, Tx, col, row, text};

use crate::theme_kit::{UI_FONT, UI_MEDIUM_FONT, rgb_color};

/// Which dock tab is active inside Pen's left sidebar (Zed-style Files / Git).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarMode {
    #[default]
    Files,
    Git,
}

/// UI state owned by the Git panel.
#[derive(Debug, Default)]
pub struct GitPanelState {
    pub mode: SidebarMode,
    pub selected_commit: Option<String>,
}

/// The latest known git state, or default/empty when the project isn't a git
/// repository (or none is open yet).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitSnapshot {
    pub is_repo: bool,
    pub entries: Vec<pen_git::StatusEntry>,
    pub history: Vec<pen_git::CommitSummary>,
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

/// Render the History list inside the Git panel. Each row shows the commit's
/// first-line summary and its 7-character id in faint tabular text. Selection
/// has no animation.
pub fn history_section<S: 'static>(
    history: &[pen_git::CommitSummary],
    selected_id: Option<&str>,
    theme: &Theme,
    on_select: impl Fn(&mut S, String, &mut Tx) + Clone + 'static,
) -> El {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let id_font = TextStyle {
        size: 11.0,
        line_height: 16.0,
        features: "'tnum' 1",
        ..UI_FONT
    };

    let mut section = col().key("pen/git-history").w_full().gap(2.0);
    section = section.child(
        row()
            .key("pen/git-history-head")
            .w_full()
            .h(24.0)
            .px(8.0)
            .center_y()
            .child(text("History").font(UI_MEDIUM_FONT).color(faint)),
    );

    if history.is_empty() {
        return section.child(
            row()
                .key("pen/git-history-empty")
                .w_full()
                .h(28.0)
                .px(8.0)
                .center_y()
                .child(text("No commits yet").font(UI_FONT).color(faint)),
        );
    }

    let mut list = col().key("pen/git-history-rows").w_full().gap(1.0);
    for commit in history {
        let active = selected_id == Some(commit.id.as_str());
        let id_owned = commit.id.clone();
        let cb = on_select.clone();
        let item = row()
            .key(format!("pen/commit-{}", commit.id))
            .role("option")
            .label(format!("{} {}", commit.summary, commit.id))
            .focusable(true)
            .w_full()
            .h(28.0)
            .px(8.0)
            .gap(8.0)
            .radius(6.0)
            .center_y()
            .cursor(Cursor::Pointer)
            .fill(if active { fg.alpha(0.10) } else { bg })
            .hover(fg.alpha(0.06))
            .transition(Transition::none())
            .on_press_tx(move |s: &mut S, tx: &mut Tx| cb(s, id_owned.clone(), tx))
            .child(
                text(commit.summary.clone())
                    .font(UI_FONT)
                    .color(fg)
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .truncate(),
            )
            .child(text(commit.id.clone()).font(id_font).color(faint));
        list = list.child(item);
    }
    section.child(list)
}

/// Full Git dock body: scrollable workbench stack.
pub fn git_dock_container(children: Vec<El>) -> El {
    col()
        .key("pen/git-panel")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(8.0)
        .gap(12.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .transition(Transition::none())
        .children(children)
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
