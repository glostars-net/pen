//! Git panel, status labels, and monochrome diff surface (`crates/app/src/git_panel.rs`).
//!
//! All data here is read-only on the UI thread: a Tokio worker refreshes
//! [`GitSnapshot`] and diff patches off-thread and publishes them back.

use std::path::PathBuf;

use highlight::Theme;
use rune::{
    Cursor, El, Key, KeyEvent, ScrollSpec, TextEdit, TextStyle, Transition, Tx, col, row, text,
    text_input,
};
use rune_kit::Kit;
use rune_kit::diff::{DiffMode, DiffState, DiffView};

use crate::theme_kit::{MONO_UI_FONT, UI_FONT, UI_MEDIUM_FONT, rgb_color};

/// Which dock tab is active inside Pen's left sidebar (Zed-style Files / Git).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarMode {
    #[default]
    Files,
    Git,
}

/// Destructive action awaiting explicit confirmation from the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingConfirm {
    Discard(PathBuf),
    RemoveWorktree(PathBuf),
}

impl PendingConfirm {
    pub fn prompt(&self) -> String {
        match self {
            PendingConfirm::Discard(rel) => {
                let name = rel
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| rel_display(rel));
                format!("Discard changes to {name}?")
            }
            PendingConfirm::RemoveWorktree(path) => {
                format!("Remove worktree {}?", path.display())
            }
        }
    }
}

/// UI state owned by the Git panel.
#[derive(Debug, Default)]
pub struct GitPanelState {
    pub mode: SidebarMode,
    pub selected_change: Option<PathBuf>,
    pub selected_commit: Option<String>,
    pub commit_open: bool,
    pub commit_message: TextEdit,
    pub add_worktree_open: bool,
    pub worktree_path: TextEdit,
    pub worktree_branch: TextEdit,
    pub confirm: Option<PendingConfirm>,
}

/// The latest known git state, or default/empty when the project isn't a git
/// repository (or none is open yet).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitSnapshot {
    pub is_repo: bool,
    pub current_branch: Option<String>,
    pub branches: Vec<String>,
    pub entries: Vec<pen_git::StatusEntry>,
    pub history: Vec<pen_git::CommitSummary>,
    pub worktrees: Vec<pen_git::WorktreeInfo>,
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

fn rel_display(rel: &std::path::Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Render the `"Changes"` section at the top of the Git dock (Zed-style):
/// clicking a row opens its unstaged diff; `+` stages, `↺` prompts discard.
pub fn changes_section<S: 'static>(
    entries: &[pen_git::StatusEntry],
    selected: Option<&std::path::Path>,
    theme: &Theme,
    on_select: impl Fn(&mut S, PathBuf, &mut Tx) + Clone + 'static,
    on_stage: impl Fn(&mut S, PathBuf, &mut Tx) + Clone + 'static,
    on_discard: impl Fn(&mut S, PathBuf, &mut Tx) + Clone + 'static,
) -> El {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let count_font = TextStyle {
        size: 11.0,
        line_height: 16.0,
        features: "'tnum' 1",
        ..UI_FONT
    };

    let mut head = row()
        .key("pen/git-changes-head")
        .w_full()
        .h(24.0)
        .px(8.0)
        .center_y()
        .justify_between()
        .child(text("Changes").font(UI_MEDIUM_FONT).color(faint));
    if !entries.is_empty() {
        head = head.child(
            text(entries.len().to_string())
                .font(count_font)
                .color(faint),
        );
    }

    let section = col().key("pen/git-changes").w_full().gap(2.0).child(head);
    if entries.is_empty() {
        return section.child(
            row()
                .key("pen/git-changes-empty")
                .w_full()
                .h(28.0)
                .px(8.0)
                .center_y()
                .child(text("Working tree clean").font(UI_FONT).color(faint)),
        );
    }

    let mut list = col().key("pen/git-changes-rows").w_full().gap(1.0);
    for entry in entries {
        let active = selected == Some(entry.rel.as_path());
        let slash = rel_display(&entry.rel);
        let lbl = status_label(entry);
        let rel_owned = entry.rel.clone();
        let cb = on_select.clone();
        let mut item = row()
            .key(format!("pen/change-{slash}"))
            .role("option")
            .label(if lbl.is_empty() {
                slash.clone()
            } else {
                format!("{slash} {lbl}")
            })
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
            .on_press_tx(move |s: &mut S, tx: &mut Tx| cb(s, rel_owned.clone(), tx))
            .child(
                text(slash.clone())
                    .font(UI_FONT)
                    .color(fg)
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .truncate(),
            );
        if !lbl.is_empty() {
            item = item.child(text(lbl).font(count_font).color(faint));
        }
        let discard_rel = entry.rel.clone();
        let discard_cb = on_discard.clone();
        let discard_btn = row()
            .key(format!("pen/discard-{slash}"))
            .role("button")
            .label(format!("Discard {slash}"))
            .focusable(true)
            .h(20.0)
            .px(6.0)
            .radius(4.0)
            .center_y()
            .cursor(Cursor::Pointer)
            .hover(fg.alpha(0.10))
            .transition(Transition::none())
            .on_press_tx(move |s: &mut S, tx: &mut Tx| discard_cb(s, discard_rel.clone(), tx))
            .child(text("↺").font(UI_FONT).color(faint));
        let stage_rel = entry.rel.clone();
        let stage_cb = on_stage.clone();
        let stage_btn = row()
            .key(format!("pen/stage-{slash}"))
            .role("button")
            .label(format!("Stage {slash}"))
            .focusable(true)
            .h(20.0)
            .px(6.0)
            .radius(4.0)
            .center_y()
            .cursor(Cursor::Pointer)
            .hover(fg.alpha(0.10))
            .transition(Transition::none())
            .on_press_tx(move |s: &mut S, tx: &mut Tx| stage_cb(s, stage_rel.clone(), tx))
            .child(text("+").font(UI_MEDIUM_FONT).color(faint));
        item = item.child(discard_btn).child(stage_btn);
        list = list.child(item);
    }
    section.child(list)
}

/// Render the `"Branches"` section inside the Git dock: clicking an inactive
/// branch checks it out (guarded by `decide_checkout` in the app).
pub fn branches_section<S: 'static>(
    branches: &[String],
    current: Option<&str>,
    theme: &Theme,
    on_checkout: impl Fn(&mut S, String, &mut Tx) + Clone + 'static,
) -> El {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);

    let mut section = col().key("pen/git-branches").w_full().gap(2.0);
    section = section.child(
        row()
            .key("pen/git-branches-head")
            .w_full()
            .h(24.0)
            .px(8.0)
            .center_y()
            .child(text("Branches").font(UI_MEDIUM_FONT).color(faint)),
    );

    if branches.is_empty() {
        return section.child(
            row()
                .key("pen/git-branches-empty")
                .w_full()
                .h(28.0)
                .px(8.0)
                .center_y()
                .child(text("No branches yet").font(UI_FONT).color(faint)),
        );
    }

    let mut list = col().key("pen/git-branches-rows").w_full().gap(1.0);
    for branch in branches {
        let is_current = current == Some(branch.as_str());
        let b_owned = branch.clone();
        let cb = on_checkout.clone();
        let mut item = row()
            .key(format!("pen/branch-{branch}"))
            .role("option")
            .label(branch.clone())
            .focusable(true)
            .w_full()
            .h(28.0)
            .px(8.0)
            .gap(8.0)
            .radius(6.0)
            .center_y()
            .cursor(Cursor::Pointer)
            .fill(if is_current { fg.alpha(0.08) } else { bg })
            .hover(fg.alpha(0.06))
            .transition(Transition::none())
            .on_press_tx(move |s: &mut S, tx: &mut Tx| cb(s, b_owned.clone(), tx))
            .child(
                text(branch.clone())
                    .font(if is_current { UI_MEDIUM_FONT } else { UI_FONT })
                    .color(fg)
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .truncate(),
            );
        if is_current {
            item = item.child(text("current").font(UI_FONT).color(faint));
        }
        list = list.child(item);
    }
    section.child(list)
}

/// Single-line inline confirmation bar pinned above the page (never a modal
/// dialog stack). Declining leaves worktree and buffers untouched.
pub fn confirm_bar<S: 'static>(
    prompt: &str,
    theme: &Theme,
    on_confirm: impl Fn(&mut S, &mut Tx) + 'static,
    on_cancel: impl Fn(&mut S, &mut Tx) + 'static,
) -> El {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let rule = fg.alpha(0.20);

    row()
        .key("pen/confirm-bar")
        .w_full()
        .h(36.0)
        .px(16.0)
        .gap(10.0)
        .center_y()
        .fill(bg)
        .border_sides([0.0, 0.0, 1.0, 0.0], rule)
        .transition(Transition::none())
        .child(
            text(prompt.to_owned())
                .font(UI_FONT)
                .color(fg)
                .grow(1.0)
                .truncate(),
        )
        .child(
            row()
                .key("pen/confirm-yes")
                .role("button")
                .label("Confirm")
                .focusable(true)
                .h(24.0)
                .px(10.0)
                .radius(6.0)
                .center()
                .cursor(Cursor::Pointer)
                .border(1.0, rule)
                .hover(fg.alpha(0.08))
                .transition(Transition::none())
                .on_press_tx(on_confirm)
                .child(text("Confirm").font(UI_MEDIUM_FONT).color(fg)),
        )
        .child(
            row()
                .key("pen/confirm-no")
                .role("button")
                .label("Cancel")
                .focusable(true)
                .h(24.0)
                .px(10.0)
                .radius(6.0)
                .center()
                .cursor(Cursor::Pointer)
                .hover(fg.alpha(0.06))
                .transition(Transition::none())
                .on_press_tx(on_cancel)
                .child(text("Cancel").font(UI_FONT).color(faint)),
        )
}

/// Single-line commit bar pinned directly above the page (same pattern as Find
/// in phase 1 — no dialog stack). `Enter` submits, `Escape` closes.
pub fn commit_bar<S: 'static>(
    message: &TextEdit,
    theme: &Theme,
    edit_field: impl Fn(&mut S) -> &mut TextEdit + 'static,
    on_submit: impl Fn(&mut S, &mut Tx) + Clone + 'static,
    on_close: impl Fn(&mut S, &mut Tx) + Clone + 'static,
) -> El {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let rule = fg.alpha(0.20);
    let submit_key = on_submit.clone();
    let close_key = on_close.clone();

    let input = text_input(message, MONO_UI_FONT)
        .key("pen/commit-input")
        .label("Commit message")
        .single_line()
        .placeholder("Commit message", faint)
        .color(fg)
        .caret(fg, fg.alpha(0.12))
        .grow(1.0)
        .h(20.0)
        .on_edit(edit_field)
        .on_input_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| match ev.key {
            Key::Escape => {
                close_key(s, tx);
                true
            }
            Key::Enter => {
                submit_key(s, tx);
                true
            }
            _ => false,
        });

    row()
        .key("pen/commit-bar")
        .w_full()
        .h(36.0)
        .px(16.0)
        .gap(12.0)
        .center_y()
        .fill(bg)
        .border_sides([0.0, 0.0, 1.0, 0.0], rule)
        .transition(Transition::none())
        .child(input)
        .child(
            row()
                .key("pen/commit-submit")
                .role("button")
                .label("Commit")
                .focusable(true)
                .h(24.0)
                .px(10.0)
                .radius(6.0)
                .center()
                .cursor(Cursor::Pointer)
                .border(1.0, rule)
                .hover(fg.alpha(0.06))
                .transition(Transition::none())
                .on_press_tx(on_submit)
                .child(text("Commit").font(UI_FONT).color(fg)),
        )
        .child(
            row()
                .key("pen/commit-close")
                .role("button")
                .label("Close commit")
                .focusable(true)
                .size(24.0, 24.0)
                .radius(6.0)
                .center()
                .cursor(Cursor::Pointer)
                .hover(fg.alpha(0.06))
                .transition(Transition::none())
                .on_press_tx(on_close)
                .child(text("×").font(UI_FONT).color(faint)),
        )
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

/// Render the `"Worktrees"` section inside the Git dock: two single-line
/// inputs for adding a worktree (path + branch, never a wizard), and a `×`
/// remove action on linked worktrees that triggers confirmation in the app.
#[allow(clippy::too_many_arguments)]
pub fn worktrees_section<S: 'static>(
    worktrees: &[pen_git::WorktreeInfo],
    add_open: bool,
    path_edit: &TextEdit,
    branch_edit: &TextEdit,
    theme: &Theme,
    edit_path: impl Fn(&mut S) -> &mut TextEdit + 'static,
    edit_branch: impl Fn(&mut S) -> &mut TextEdit + 'static,
    on_toggle_add: impl Fn(&mut S, &mut Tx) + Clone + 'static,
    on_submit_add: impl Fn(&mut S, &mut Tx) + Clone + 'static,
    on_remove: impl Fn(&mut S, PathBuf, &mut Tx) + Clone + 'static,
) -> El {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let rule = fg.alpha(0.20);

    let toggle_head = on_toggle_add.clone();
    let head = row()
        .key("pen/git-worktrees-head")
        .w_full()
        .h(24.0)
        .px(8.0)
        .center_y()
        .justify_between()
        .child(text("Worktrees").font(UI_MEDIUM_FONT).color(faint))
        .child(
            row()
                .key("pen/git-worktree-add-btn")
                .role("button")
                .label("Add worktree")
                .focusable(true)
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .cursor(Cursor::Pointer)
                .hover(fg.alpha(0.08))
                .transition(Transition::none())
                .on_press_tx(toggle_head)
                .child(text("+").font(UI_MEDIUM_FONT).color(faint)),
        );

    let mut section = col().key("pen/git-worktrees").w_full().gap(4.0).child(head);

    if add_open {
        let cancel_key = on_toggle_add.clone();
        let cancel_key2 = on_toggle_add.clone();
        let submit_key = on_submit_add.clone();
        let path_input = text_input(path_edit, MONO_UI_FONT)
            .key("pen/wt-path-input")
            .label("Worktree path")
            .single_line()
            .placeholder("../worktree-dir", faint)
            .color(fg)
            .caret(fg, fg.alpha(0.12))
            .w_full()
            .h(22.0)
            .on_edit(edit_path)
            .on_input_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| match ev.key {
                Key::Escape => {
                    cancel_key(s, tx);
                    true
                }
                _ => false,
            });
        let branch_input = text_input(branch_edit, MONO_UI_FONT)
            .key("pen/wt-branch-input")
            .label("Worktree branch")
            .single_line()
            .placeholder("branch-name", faint)
            .color(fg)
            .caret(fg, fg.alpha(0.12))
            .w_full()
            .h(22.0)
            .on_edit(edit_branch)
            .on_input_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| match ev.key {
                Key::Escape => {
                    cancel_key2(s, tx);
                    true
                }
                Key::Enter => {
                    submit_key(s, tx);
                    true
                }
                _ => false,
            });

        let form = col()
            .key("pen/git-worktree-form")
            .w_full()
            .padding(6.0)
            .gap(6.0)
            .radius(6.0)
            .border(1.0, rule)
            .child(
                row()
                    .w_full()
                    .px(6.0)
                    .h(26.0)
                    .center_y()
                    .border_sides([0.0, 0.0, 1.0, 0.0], rule)
                    .child(path_input),
            )
            .child(
                row()
                    .w_full()
                    .px(6.0)
                    .h(26.0)
                    .center_y()
                    .child(branch_input),
            )
            .child(
                row()
                    .w_full()
                    .justify_end()
                    .gap(6.0)
                    .child(
                        row()
                            .key("pen/wt-cancel")
                            .role("button")
                            .label("Cancel")
                            .focusable(true)
                            .h(22.0)
                            .px(8.0)
                            .radius(4.0)
                            .center()
                            .cursor(Cursor::Pointer)
                            .hover(fg.alpha(0.06))
                            .transition(Transition::none())
                            .on_press_tx(on_toggle_add)
                            .child(text("Cancel").font(UI_FONT).color(faint)),
                    )
                    .child(
                        row()
                            .key("pen/wt-submit")
                            .role("button")
                            .label("Add")
                            .focusable(true)
                            .h(22.0)
                            .px(8.0)
                            .radius(4.0)
                            .center()
                            .cursor(Cursor::Pointer)
                            .border(1.0, rule)
                            .hover(fg.alpha(0.08))
                            .transition(Transition::none())
                            .on_press_tx(on_submit_add)
                            .child(text("Add").font(UI_MEDIUM_FONT).color(fg)),
                    ),
            );
        section = section.child(form);
    }

    let mut list = col().key("pen/git-worktrees-rows").w_full().gap(1.0);
    for (idx, wt) in worktrees.iter().enumerate() {
        let label_text = wt
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| wt.path.display().to_string());
        let mut item = row()
            .key(format!("pen/wt-{}", wt.path.display()))
            .w_full()
            .h(28.0)
            .px(8.0)
            .gap(8.0)
            .radius(6.0)
            .center_y()
            .fill(bg)
            .transition(Transition::none())
            .child(
                text(label_text)
                    .font(UI_FONT)
                    .color(fg)
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .truncate(),
            )
            .child(text(wt.branch.clone()).font(UI_FONT).color(faint));

        if idx > 0 {
            let rm_path = wt.path.clone();
            let rm_cb = on_remove.clone();
            item = item.child(
                row()
                    .key(format!("pen/wt-rm-{}", wt.path.display()))
                    .role("button")
                    .label(format!("Remove worktree {}", wt.path.display()))
                    .focusable(true)
                    .size(20.0, 20.0)
                    .radius(4.0)
                    .center()
                    .cursor(Cursor::Pointer)
                    .hover(fg.alpha(0.10))
                    .transition(Transition::none())
                    .on_press_tx(move |s: &mut S, tx: &mut Tx| rm_cb(s, rm_path.clone(), tx))
                    .child(text("×").font(UI_FONT).color(faint)),
            );
        }
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

/// Monochrome diff page (Zed-inspired header + `rune_kit::diff::DiffView`).
pub fn diff_page_view<S: 'static>(
    state: &S,
    title: &str,
    theme: &Theme,
    kit: &Kit,
    get: impl Fn(&S) -> &DiffState + 'static,
    get_mut: impl Fn(&mut S) -> &mut DiffState + 'static,
    on_toggle_split: impl Fn(&mut S, &mut Tx) + 'static,
) -> El {
    let ds = get(state);
    let mode = ds.mode();
    let empty = ds.files().is_empty();
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let rule = fg.alpha(0.20);
    let toggle_label = match mode {
        DiffMode::Unified => "Split diff",
        DiffMode::Split => "Unified diff",
    };

    let header = row()
        .key("pen/diff-header")
        .w_full()
        .h(36.0)
        .px(16.0)
        .center_y()
        .justify_between()
        .fill(bg)
        .border_sides([0.0, 0.0, 1.0, 0.0], rule)
        .transition(Transition::none())
        .child(
            text(title.to_owned())
                .font(UI_MEDIUM_FONT)
                .color(fg)
                .truncate(),
        )
        .child(
            row()
                .key("pen/diff-mode-toggle")
                .role("button")
                .label(toggle_label)
                .focusable(true)
                .h(26.0)
                .px(10.0)
                .radius(6.0)
                .center_y()
                .cursor(Cursor::Pointer)
                .border(1.0, rule)
                .hover(fg.alpha(0.06))
                .transition(Transition::none())
                .on_press_tx(on_toggle_split)
                .child(text(toggle_label).font(UI_FONT).color(faint)),
        );

    let body = if empty {
        col()
            .key("pen/diff-empty")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(bg)
            .center()
            .transition(Transition::none())
            .child(text("No changes").font(UI_FONT).color(faint))
    } else {
        DiffView::new(get, get_mut)
            .key("pen/diff-view")
            .build(state, kit)
            .grow(1.0)
            .min_h(0.0)
    };

    col()
        .key("pen/diff-page")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(bg)
        .instant_changes()
        .transition(Transition::none())
        .child(header)
        .child(body)
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
