//! Pen: a code editor that feels like a pen on paper, drawn by Rune.
//!
//! Phase 1 (plan/2026-11-10-pen.md): one window, a folder tree, a rope-backed
//! editor page, save, find, and a command palette. The UI thread never does
//! disk or watcher work; a Tokio worker handles all filesystem calls and wakes
//! the window via `Waker`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use buffer::Buffer;
use highlight::{Theme, lang_for_path, load_theme};
use pen::disk::{DiskEvent, on_disk_change};
use pen::editor::{CODE_SIZE, EditorState, LINE_H, editor_view_themed};
use pen::git_guard::{CheckoutDecision, decide_checkout};
use pen::git_panel::{
    GitPanelState, GitSnapshot, PendingConfirm, SidebarMode, branches_section, changes_section,
    commit_bar, confirm_bar, diff_page_view, git_dock_container, history_section, status_label,
};
use pen::palette::{BUILTIN_COMMANDS, filter_command_list, shortcut_hint};
use pen::theme_kit::{UI_FONT, UI_MEDIUM_FONT, build_kit, diff_highlighter, rgb_color};
use project::watch::{ProjectWatcher, WatchEvent, watch_project_with};
use project::{FileEntry, Project, ReadFile};
use rune::dirs::AppDirs;
use rune::{
    AppSpec, Chrome, Cursor, Cx, Drag, El, Family, FileDialogOptions, Ime, Key, KeyEvent,
    OpticalSize, Spring, TextEdit, TextStyle, Transition, Tx, Waker, WheelEvent, col, el, row,
    text, text_input,
};
use rune_kit::diff::{DiffMode, DiffState, parse as parse_diff};
use rune_kit::shell::{self, PaneMotion, PaneState, SplitView, Toolbar};
use rune_kit::{FileNode, FileTree, Tree, TreeItem, TreeModel};
use tokio::sync::{mpsc, watch};

/// Git status refreshes at most this often (Task 2): a burst of triggers
/// (opening a project, several quick saves) collapses into one refresh.
const GIT_STATUS_THROTTLE: Duration = Duration::from_millis(200);

const FIND_FONT: TextStyle = TextStyle {
    size: CODE_SIZE,
    weight: 400.0,
    family: Family::Mono,
    line_height: LINE_H,
    tracking: 0.0,
    variations: "",
    features: "",
    optical_size: OpticalSize::Auto,
    italic: false,
};

#[derive(Clone, Debug)]
enum ThemePref {
    System,
    Ink,
    Paper,
    Custom(Box<Theme>),
}

impl ThemePref {
    fn resolve(&self, system_dark: bool) -> Theme {
        match self {
            ThemePref::System => {
                if system_dark {
                    highlight::ink()
                } else {
                    highlight::paper()
                }
            }
            ThemePref::Ink => highlight::ink(),
            ThemePref::Paper => highlight::paper(),
            ThemePref::Custom(t) => (**t).clone(),
        }
    }
}

/// Requests sent from the UI thread to the Tokio disk worker.
enum IoCmd {
    OpenProject(PathBuf),
    RefreshTree(Project),
    ReadFile {
        project: Project,
        rel: PathBuf,
    },
    ReloadCleanFile {
        project: Project,
        rel: PathBuf,
    },
    SaveFile {
        project: Project,
        rel: PathBuf,
        bytes: Vec<u8>,
    },
    SaveThemePref(&'static str),
    DiffWorktree {
        root: PathBuf,
        rel: PathBuf,
    },
    DiffCommit {
        root: PathBuf,
        id: String,
    },
    StageFile {
        root: PathBuf,
        rel: PathBuf,
    },
    Commit {
        root: PathBuf,
        message: String,
    },
    CheckoutBranch {
        project: Project,
        branch: String,
    },
    DiscardFile {
        project: Project,
        rel: PathBuf,
    },
}

/// Answers sent back from the Tokio disk worker to `poll`.
enum IoMsg {
    PrefsLoaded {
        theme_pref: ThemePref,
        first_launch: bool,
    },
    ProjectListed {
        project: Project,
        entries: Vec<FileEntry>,
        watch_rx: Option<watch::Receiver<Option<WatchEvent>>>,
    },
    TreeRefreshed {
        entries: Vec<FileEntry>,
    },
    FileRead {
        rel: PathBuf,
        result: ReadFile,
    },
    FileReloaded {
        rel: PathBuf,
        result: ReadFile,
    },
    FileSaved {
        rel: PathBuf,
    },
    DiffLoaded {
        title: String,
        patch: String,
    },
    WorktreeUpdated {
        note: String,
        entries: Vec<FileEntry>,
    },
    StatusNote(String),
    Error(String),
}

enum OpenPage {
    Empty,
    Editor(EditorState),
    Diff { title: String },
    Notice(String),
}

struct Pen {
    /// Folder tree pane. Width per the plan: 248 pt, resizable 160–480.
    sidebar: PaneState,
    project: Option<Project>,
    files: TreeModel<FileNode<PathBuf>>,
    active_path: Option<PathBuf>,
    page: OpenPage,
    unsaved: BTreeMap<PathBuf, EditorState>,
    theme_pref: ThemePref,
    status: Option<String>,
    palette_open: bool,
    palette_query: TextEdit,
    palette_index: usize,
    find_open: bool,
    find_query: TextEdit,
    page_opacity: f32,
    reveal_first_launch: bool,
    watch_rx: Option<watch::Receiver<Option<WatchEvent>>>,
    git_status: GitSnapshot,
    git_panel: GitPanelState,
    diff_state: DiffState,
    diff_theme_bg: highlight::Rgb,
    git_rx: Option<watch::Receiver<GitSnapshot>>,
    io_tx: Option<mpsc::Sender<IoCmd>>,
    io_rx: Option<mpsc::Receiver<IoMsg>>,
}

fn init() -> Pen {
    Pen {
        sidebar: PaneState::new(248.0).range(160.0, 480.0).collapsed(),
        project: None,
        files: TreeModel::new().key("pen/tree"),
        active_path: None,
        page: OpenPage::Empty,
        unsaved: BTreeMap::new(),
        theme_pref: ThemePref::System,
        status: None,
        palette_open: false,
        palette_query: TextEdit::new(""),
        palette_index: 0,
        find_open: false,
        find_query: TextEdit::new(""),
        page_opacity: 1.0,
        reveal_first_launch: false,
        watch_rx: None,
        git_status: GitSnapshot::default(),
        git_panel: GitPanelState::default(),
        diff_state: {
            let paper = highlight::paper();
            let mut ds = DiffState::default();
            ds.set_highlighter(diff_highlighter(&paper));
            ds
        },
        diff_theme_bg: highlight::paper().bg,
        git_rx: None,
        io_tx: None,
        io_rx: None,
    }
}

fn setup(s: &mut Pen, waker: Waker) {
    let headless = rune::is_headless();
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<IoCmd>(32);
    let (msg_tx, msg_rx) = mpsc::channel::<IoMsg>(32);
    let (git_trigger_tx, mut git_trigger_rx) = mpsc::channel::<PathBuf>(8);
    let (git_tx, git_rx) = watch::channel(GitSnapshot::default());
    if !headless && let Some(arg) = rune::extra_args().iter().find(|a| !a.starts_with('-')) {
        let _ = cmd_tx.try_send(IoCmd::OpenProject(PathBuf::from(arg)));
    }
    std::thread::Builder::new()
        .name("pen-io".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                // Git status: collapses a burst of triggers (project open, several
                // quick saves) into one refresh at most every `GIT_STATUS_THROTTLE`.
                tokio::spawn({
                    let waker = waker.clone();
                    async move {
                        let mut last_refresh = Instant::now() - GIT_STATUS_THROTTLE;
                        while let Some(root) = git_trigger_rx.recv().await {
                            while git_trigger_rx.try_recv().is_ok() {}
                            let elapsed = last_refresh.elapsed();
                            if elapsed < GIT_STATUS_THROTTLE {
                                tokio::time::sleep(GIT_STATUS_THROTTLE - elapsed).await;
                                while git_trigger_rx.try_recv().is_ok() {}
                            }
                            let snap = tokio::task::spawn_blocking(move || {
                                let Some(repo) = pen_git::Repo::discover(&root).ok().flatten()
                                else {
                                    return GitSnapshot::default();
                                };
                                GitSnapshot {
                                    is_repo: true,
                                    current_branch: repo.current_branch().ok().flatten(),
                                    branches: repo.branches().unwrap_or_default(),
                                    entries: repo.status().unwrap_or_default(),
                                    history: repo.log(50).unwrap_or_default(),
                                }
                            })
                            .await
                            .unwrap_or_default();
                            last_refresh = Instant::now();
                            if git_tx.send(snap).is_err() {
                                break;
                            }
                            waker();
                        }
                    }
                });

                if !headless {
                    let (theme_pref, first_launch) =
                        tokio::task::spawn_blocking(read_startup_prefs_blocking)
                            .await
                            .unwrap_or((ThemePref::System, false));
                    let _ = msg_tx
                        .send(IoMsg::PrefsLoaded {
                            theme_pref,
                            first_launch,
                        })
                        .await;
                    waker();
                }

                let mut active_watcher: Option<ProjectWatcher> = None;
                while let Some(cmd) = cmd_rx.recv().await {
                    let reply = match cmd {
                        IoCmd::OpenProject(path) => {
                            active_watcher = None;
                            let res = tokio::task::spawn_blocking(move || {
                                let project = Project::open(path)?;
                                let entries = project.list()?;
                                Ok::<_, project::ProjectError>((project, entries))
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()));
                            match res {
                                Ok((project, entries)) => {
                                    let w = waker.clone();
                                    let watcher_git_tx = git_trigger_tx.clone();
                                    let watcher_root = project.root.clone();
                                    let watch_rx = match watch_project_with(&project, move || {
                                        let _ = watcher_git_tx.try_send(watcher_root.clone());
                                        w();
                                    }) {
                                        Ok((guard, rx)) => {
                                            active_watcher = Some(guard);
                                            Some(rx)
                                        }
                                        Err(_) => None,
                                    };
                                    let _ = git_trigger_tx.try_send(project.root.clone());
                                    IoMsg::ProjectListed {
                                        project,
                                        entries,
                                        watch_rx,
                                    }
                                }
                                Err(e) => IoMsg::Error(e),
                            }
                        }
                        IoCmd::RefreshTree(project) => {
                            tokio::task::spawn_blocking(move || project.list())
                                .await
                                .map_err(|e| e.to_string())
                                .and_then(|r| r.map_err(|e| e.to_string()))
                                .map_or_else(IoMsg::Error, |entries| IoMsg::TreeRefreshed {
                                    entries,
                                })
                        }
                        IoCmd::ReadFile { project, rel } => {
                            tokio::task::spawn_blocking(move || {
                                let result = project.read(&rel)?;
                                Ok::<_, project::ProjectError>((rel, result))
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()))
                            .map_or_else(
                                IoMsg::Error,
                                |(rel, result)| IoMsg::FileRead { rel, result },
                            )
                        }
                        IoCmd::ReloadCleanFile { project, rel } => {
                            tokio::task::spawn_blocking(move || {
                                let result = project.read(&rel)?;
                                Ok::<_, project::ProjectError>((rel, result))
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()))
                            .map_or_else(
                                IoMsg::Error,
                                |(rel, result)| IoMsg::FileReloaded { rel, result },
                            )
                        }
                        IoCmd::SaveFile {
                            project,
                            rel,
                            bytes,
                        } => {
                            let root = project.root.clone();
                            let reply = tokio::task::spawn_blocking(move || {
                                project.write_atomic(&rel, &bytes)?;
                                Ok::<_, project::ProjectError>(rel)
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()))
                            .map_or_else(IoMsg::Error, |rel| IoMsg::FileSaved { rel });
                            let _ = git_trigger_tx.try_send(root);
                            reply
                        }
                        IoCmd::SaveThemePref(choice) => {
                            let _ = tokio::task::spawn_blocking(move || {
                                write_theme_pref_blocking(choice)
                            })
                            .await;
                            continue;
                        }
                        IoCmd::DiffWorktree { root, rel } => {
                            tokio::task::spawn_blocking(move || {
                                let repo = pen_git::Repo::discover(&root)
                                    .map_err(|e| e.to_string())?
                                    .ok_or_else(|| "Not a git repository.".to_string())?;
                                let patch = repo.diff_worktree(&rel).map_err(|e| e.to_string())?;
                                Ok::<_, String>((
                                    format!("Changes — {}", path_to_slash(&rel)),
                                    patch,
                                ))
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()))
                            .map_or_else(
                                IoMsg::Error,
                                |(title, patch)| IoMsg::DiffLoaded { title, patch },
                            )
                        }
                        IoCmd::DiffCommit { root, id } => tokio::task::spawn_blocking(move || {
                            let repo = pen_git::Repo::discover(&root)
                                .map_err(|e| e.to_string())?
                                .ok_or_else(|| "Not a git repository.".to_string())?;
                            let patch = repo.diff_commit(&id).map_err(|e| e.to_string())?;
                            Ok::<_, String>((format!("Changes — {id}"), patch))
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| e.to_string()))
                        .map_or_else(IoMsg::Error, |(title, patch)| IoMsg::DiffLoaded {
                            title,
                            patch,
                        }),
                        IoCmd::StageFile { root, rel } => {
                            let refresh_root = root.clone();
                            let reply = tokio::task::spawn_blocking(move || {
                                let repo = pen_git::Repo::discover(&root)
                                    .map_err(|e| e.to_string())?
                                    .ok_or_else(|| "Not a git repository.".to_string())?;
                                repo.stage(&rel).map_err(|e| e.to_string())?;
                                Ok::<_, String>(format!("Staged {}.", path_to_slash(&rel)))
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r)
                            .map_or_else(IoMsg::Error, IoMsg::StatusNote);
                            let _ = git_trigger_tx.try_send(refresh_root);
                            reply
                        }
                        IoCmd::Commit { root, message } => {
                            let refresh_root = root.clone();
                            let reply = tokio::task::spawn_blocking(move || {
                                let repo = pen_git::Repo::discover(&root)
                                    .map_err(|e| e.to_string())?
                                    .ok_or_else(|| "Not a git repository.".to_string())?;
                                let oid = repo.commit(&message).map_err(|e| e.to_string())?;
                                let short: String = oid.chars().take(7).collect();
                                Ok::<_, String>(format!("Committed {short}."))
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r)
                            .map_or_else(IoMsg::Error, IoMsg::StatusNote);
                            let _ = git_trigger_tx.try_send(refresh_root);
                            reply
                        }
                        IoCmd::CheckoutBranch { project, branch } => {
                            let refresh_root = project.root.clone();
                            let reply = tokio::task::spawn_blocking(move || {
                                let repo = pen_git::Repo::discover(&project.root)
                                    .map_err(|e| e.to_string())?
                                    .ok_or_else(|| "Not a git repository.".to_string())?;
                                repo.checkout(&branch).map_err(|e| e.to_string())?;
                                let entries = project.list().map_err(|e| e.to_string())?;
                                Ok::<_, String>((format!("Checked out {branch}."), entries))
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r)
                            .map_or_else(
                                IoMsg::Error,
                                |(note, entries)| IoMsg::WorktreeUpdated { note, entries },
                            );
                            let _ = git_trigger_tx.try_send(refresh_root);
                            reply
                        }
                        IoCmd::DiscardFile { project, rel } => {
                            let refresh_root = project.root.clone();
                            let reply = tokio::task::spawn_blocking(move || {
                                let repo = pen_git::Repo::discover(&project.root)
                                    .map_err(|e| e.to_string())?
                                    .ok_or_else(|| "Not a git repository.".to_string())?;
                                repo.discard(&rel).map_err(|e| e.to_string())?;
                                let entries = project.list().map_err(|e| e.to_string())?;
                                Ok::<_, String>((
                                    format!("Discarded changes to {}.", path_to_slash(&rel)),
                                    entries,
                                ))
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r)
                            .map_or_else(
                                IoMsg::Error,
                                |(note, entries)| IoMsg::WorktreeUpdated { note, entries },
                            );
                            let _ = git_trigger_tx.try_send(refresh_root);
                            reply
                        }
                    };
                    let _ = &active_watcher;
                    if msg_tx.send(reply).await.is_err() {
                        break;
                    }
                    waker();
                }
            });
        })
        .ok();
    s.io_tx = Some(cmd_tx);
    s.io_rx = Some(msg_rx);
    s.git_rx = Some(git_rx);
}

fn poll(s: &mut Pen, _now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;

    if s.reveal_first_launch {
        s.reveal_first_launch = false;
        s.page_opacity = 1.0;
        changed = true;
    }

    let watch_ev = s
        .watch_rx
        .as_mut()
        .filter(|rx| rx.has_changed().unwrap_or(false))
        .and_then(|rx| rx.borrow_and_update().clone());
    if let Some(WatchEvent::Changed(rel)) = watch_ev {
        if s.active_path.as_ref() == Some(&rel) {
            let dirty = matches!(&s.page, OpenPage::Editor(ed) if ed.buffer.is_dirty());
            match on_disk_change(dirty) {
                DiskEvent::Keep => {
                    s.status = Some("Changed on disk. Your copy is kept.".into());
                    changed = true;
                    tx.instant();
                }
                DiskEvent::Reload => {
                    if let (Some(project), Some(io_tx)) = (s.project.clone(), &s.io_tx) {
                        let _ = io_tx.try_send(IoCmd::ReloadCleanFile {
                            project,
                            rel: rel.clone(),
                        });
                    }
                }
            }
        }
        let slash = path_to_slash(&rel);
        if !s.files.contains(&slash)
            && let (Some(project), Some(io_tx)) = (s.project.clone(), &s.io_tx)
        {
            let _ = io_tx.try_send(IoCmd::RefreshTree(project));
        }
    }

    let git_snapshot = s
        .git_rx
        .as_mut()
        .filter(|rx| rx.has_changed().unwrap_or(false))
        .map(|rx| rx.borrow_and_update().clone());
    if let Some(snapshot) = git_snapshot {
        s.git_status = snapshot;
        changed = true;
    }

    let mut batch = Vec::new();
    if let Some(rx) = &mut s.io_rx {
        while let Ok(msg) = rx.try_recv() {
            batch.push(msg);
        }
    }
    if batch.is_empty() {
        return changed;
    }
    for msg in batch {
        match msg {
            IoMsg::PrefsLoaded {
                theme_pref,
                first_launch,
            } => {
                s.theme_pref = theme_pref;
                if first_launch && s.project.is_none() {
                    s.page_opacity = 0.0;
                    s.reveal_first_launch = true;
                    tx.instant();
                    tx.wake_after(Duration::from_millis(16));
                } else {
                    tx.instant();
                }
            }
            IoMsg::ProjectListed {
                project,
                entries,
                watch_rx,
            } => {
                s.files = build_file_tree(entries);
                s.project = Some(project);
                s.watch_rx = watch_rx;
                s.active_path = None;
                s.page = OpenPage::Empty;
                s.unsaved.clear();
                s.git_status = GitSnapshot::default();
                s.git_panel = GitPanelState::default();
                s.status = None;
                if !s.sidebar.is_open() {
                    s.sidebar.toggle();
                }
                tx.instant();
            }
            IoMsg::TreeRefreshed { entries } => {
                let selected = s.files.cursor().map(str::to_owned);
                s.files = build_file_tree(entries);
                if let Some(id) = selected
                    && s.files.contains(&id)
                {
                    s.files.select(&id);
                }
                tx.instant();
            }
            IoMsg::FileRead { rel, result } => {
                s.stash_current_if_dirty();
                let lang = lang_for_path(rel.to_string_lossy().as_ref());
                s.active_path = Some(rel);
                s.page = match result {
                    ReadFile::Utf8(src) => {
                        let mut buf = Buffer::from_str(&src);
                        buf.mark_saved();
                        let mut ed = EditorState::from_buffer(buf).with_language(lang);
                        ed.focused = !s.palette_open && !s.find_open;
                        OpenPage::Editor(ed)
                    }
                    ReadFile::NotUtf8 => OpenPage::Notice("This file is not UTF-8.".into()),
                };
                s.status = None;
                tx.instant();
            }
            IoMsg::FileReloaded { rel, result } => {
                if s.active_path.as_ref() == Some(&rel) {
                    match (&mut s.page, result) {
                        (OpenPage::Editor(ed), _) if ed.buffer.is_dirty() => {
                            s.status = Some("Changed on disk. Your copy is kept.".into());
                        }
                        (OpenPage::Editor(ed), ReadFile::Utf8(src)) => {
                            if ed.buffer.text() != src {
                                let caret = ed.caret;
                                let scroll_y = ed.scroll_y;
                                let mut buf = Buffer::from_str(&src);
                                buf.mark_saved();
                                ed.caret = caret.min(buf.len_chars());
                                ed.anchor = ed.caret;
                                ed.scroll_y = scroll_y;
                                ed.buffer = buf;
                            }
                        }
                        (_, ReadFile::NotUtf8) => {
                            s.page = OpenPage::Notice("This file is not UTF-8.".into());
                        }
                        _ => {}
                    }
                    tx.instant();
                }
            }
            IoMsg::FileSaved { rel } => {
                if s.active_path.as_ref() == Some(&rel)
                    && let OpenPage::Editor(ed) = &mut s.page
                {
                    ed.buffer.mark_saved();
                }
                s.unsaved.remove(&rel);
                s.status = None;
                tx.instant();
            }
            IoMsg::DiffLoaded { title, patch } => {
                s.stash_current_if_dirty();
                s.diff_state.set_files(parse_diff(&patch));
                s.page = OpenPage::Diff { title };
                s.status = None;
                tx.instant();
            }
            IoMsg::WorktreeUpdated { note, entries } => {
                let prev_sel = s.files.selected().next().map(str::to_owned);
                let mut tree = build_file_tree(entries);
                if let Some(sel) = prev_sel
                    && tree.contains(&sel)
                {
                    tree.select(&sel);
                }
                s.files = tree;
                if let (Some(project), Some(rel), OpenPage::Editor(ed)) =
                    (s.project.clone(), s.active_path.clone(), &s.page)
                    && !ed.buffer.is_dirty()
                    && let Some(io_tx) = &s.io_tx
                {
                    let _ = io_tx.try_send(IoCmd::ReloadCleanFile { project, rel });
                }
                s.status = Some(note);
                tx.instant();
            }
            IoMsg::StatusNote(note) => {
                s.status = Some(note);
                tx.instant();
            }
            IoMsg::Error(err) => {
                s.status = Some(err);
                tx.instant();
            }
        }
    }
    true
}

impl Pen {
    fn any_buffer_dirty(&self) -> bool {
        let active_dirty = matches!(&self.page, OpenPage::Editor(ed) if ed.buffer.is_dirty());
        active_dirty || !self.unsaved.is_empty()
    }

    fn request_checkout(&mut self, branch: String, tx: &mut Tx) {
        if matches!(
            decide_checkout(self.any_buffer_dirty()),
            CheckoutDecision::KeepBuffer
        ) {
            self.status = Some("Changed on disk. Your copy is kept.".into());
            tx.instant();
            return;
        }
        if let (Some(project), Some(io_tx)) = (self.project.clone(), &self.io_tx) {
            let _ = io_tx.try_send(IoCmd::CheckoutBranch { project, branch });
        }
    }

    fn request_discard(&mut self, rel: PathBuf, tx: &mut Tx) {
        if matches!(
            decide_checkout(self.any_buffer_dirty()),
            CheckoutDecision::KeepBuffer
        ) {
            self.status = Some("Changed on disk. Your copy is kept.".into());
            tx.instant();
            return;
        }
        self.git_panel.confirm = Some(PendingConfirm::Discard(rel));
        tx.instant();
    }

    fn confirm_pending(&mut self, tx: &mut Tx) {
        let Some(confirm) = self.git_panel.confirm.take() else {
            return;
        };
        if matches!(
            decide_checkout(self.any_buffer_dirty()),
            CheckoutDecision::KeepBuffer
        ) {
            self.status = Some("Changed on disk. Your copy is kept.".into());
            tx.instant();
            return;
        }
        match confirm {
            PendingConfirm::Discard(rel) => {
                if let (Some(project), Some(io_tx)) = (self.project.clone(), &self.io_tx) {
                    let _ = io_tx.try_send(IoCmd::DiscardFile { project, rel });
                }
            }
        }
        tx.instant();
    }

    fn palette_commands(&self) -> Vec<String> {
        let mut cmds: Vec<String> = BUILTIN_COMMANDS.iter().map(|c| (*c).to_owned()).collect();
        for b in &self.git_status.branches {
            cmds.push(format!("Checkout {b}"));
        }
        cmds
    }

    fn stash_current_if_dirty(&mut self) {
        let Some(prev) = self.active_path.take() else {
            return;
        };
        if let OpenPage::Editor(ed) = std::mem::replace(&mut self.page, OpenPage::Empty)
            && ed.buffer.is_dirty()
        {
            self.unsaved.insert(prev, ed);
        }
    }

    fn request_open_file(&mut self, id: &str, tx: &mut Tx) {
        let Some(rel) = self.files.get(id).and_then(|n| n.data.clone()) else {
            return;
        };
        if self.active_path.as_ref() == Some(&rel) {
            return;
        }
        if let Some(mut stashed) = self.unsaved.remove(&rel) {
            self.stash_current_if_dirty();
            self.active_path = Some(rel);
            stashed.focused = !self.palette_open && !self.find_open;
            self.page = OpenPage::Editor(stashed);
            self.status = None;
            tx.instant();
            return;
        }
        let (Some(project), Some(io_tx)) = (self.project.clone(), &self.io_tx) else {
            return;
        };
        let _ = io_tx.try_send(IoCmd::ReadFile { project, rel });
    }

    fn request_save(&mut self) -> bool {
        let (Some(project), Some(rel), Some(io_tx)) =
            (self.project.clone(), self.active_path.clone(), &self.io_tx)
        else {
            return false;
        };
        let OpenPage::Editor(ed) = &self.page else {
            return false;
        };
        let _ = io_tx.try_send(IoCmd::SaveFile {
            project,
            rel,
            bytes: ed.buffer.text().into_bytes(),
        });
        true
    }

    fn prompt_open_folder(&mut self, tx: &mut Tx) {
        let options = FileDialogOptions {
            title: "Open a folder".into(),
            directory: self.project.as_ref().map(|p| p.root.clone()),
            file_name: None,
            filters: Vec::new(),
        };
        tx.open_folder(options, |s: &mut Pen, result, _tx| {
            if let Ok(Some(path)) = result
                && let Some(io_tx) = &s.io_tx
            {
                let _ = io_tx.try_send(IoCmd::OpenProject(path));
            }
        });
    }

    fn toggle_palette(&mut self, tx: &mut Tx) {
        if self.palette_open {
            self.close_palette(tx);
        } else {
            self.palette_open = true;
            self.palette_index = 0;
            self.palette_query.clear();
            self.palette_query.focused = true;
            if let OpenPage::Editor(ed) = &mut self.page {
                ed.focused = false;
            }
            tx.instant();
        }
    }

    fn close_palette(&mut self, tx: &mut Tx) {
        self.palette_open = false;
        self.palette_query.focused = false;
        tx.blur();
        if let OpenPage::Editor(ed) = &mut self.page {
            ed.focused = !self.find_open;
        }
        tx.instant();
    }

    fn open_find(&mut self, tx: &mut Tx) {
        self.palette_open = false;
        self.palette_query.focused = false;
        self.find_open = true;
        self.find_query.focused = true;
        if let OpenPage::Editor(ed) = &mut self.page {
            ed.focused = false;
        }
        tx.instant();
    }

    fn close_find(&mut self, tx: &mut Tx) {
        self.find_open = false;
        self.find_query.focused = false;
        tx.blur();
        if let OpenPage::Editor(ed) = &mut self.page {
            ed.focused = !self.git_panel.commit_open;
        }
        tx.instant();
    }

    fn open_commit_bar(&mut self, tx: &mut Tx) {
        self.palette_open = false;
        self.palette_query.focused = false;
        self.git_panel.commit_open = true;
        self.git_panel.commit_message.focused = true;
        if let OpenPage::Editor(ed) = &mut self.page {
            ed.focused = false;
        }
        tx.instant();
    }

    fn close_commit_bar(&mut self, tx: &mut Tx) {
        self.git_panel.commit_open = false;
        self.git_panel.commit_message.focused = false;
        tx.blur();
        if let OpenPage::Editor(ed) = &mut self.page {
            ed.focused = !self.find_open;
        }
        tx.instant();
    }

    fn submit_commit(&mut self, tx: &mut Tx) {
        let message = self.git_panel.commit_message.text.clone();
        if message.trim().is_empty() {
            self.status = Some("Commit message cannot be empty.".into());
            tx.instant();
            return;
        }
        let Some(proj) = &self.project else {
            self.status = Some("Open a folder first.".into());
            tx.instant();
            return;
        };
        if let Some(io_tx) = &self.io_tx {
            let _ = io_tx.try_send(IoCmd::Commit {
                root: proj.root.clone(),
                message,
            });
        }
        self.git_panel.commit_message.clear();
        self.close_commit_bar(tx);
    }

    fn run_command(&mut self, name: &str, tx: &mut Tx) {
        self.close_palette(tx);
        match name {
            "Open folder" => self.prompt_open_folder(tx),
            "Save" => {
                self.request_save();
            }
            "Find" => self.open_find(tx),
            "Stage file" => {
                let target = self
                    .active_path
                    .clone()
                    .or_else(|| self.git_panel.selected_change.clone());
                match (&self.project, target, &self.io_tx) {
                    (Some(proj), Some(rel), Some(io_tx)) => {
                        let _ = io_tx.try_send(IoCmd::StageFile {
                            root: proj.root.clone(),
                            rel,
                        });
                    }
                    _ => {
                        self.status = Some("Select a file to stage.".into());
                        tx.instant();
                    }
                }
            }
            "Commit" => self.open_commit_bar(tx),
            "Discard file" => {
                let target = self
                    .active_path
                    .clone()
                    .or_else(|| self.git_panel.selected_change.clone());
                match target {
                    Some(rel) => self.request_discard(rel, tx),
                    None => {
                        self.status = Some("Select a file to discard.".into());
                        tx.instant();
                    }
                }
            }
            "Split diff" => {
                let next = match self.diff_state.mode() {
                    DiffMode::Unified => DiffMode::Split,
                    DiffMode::Split => DiffMode::Unified,
                };
                self.diff_state.set_mode(next);
                tx.instant();
            }
            "Use Ink" => {
                let ink = highlight::ink();
                self.theme_pref = ThemePref::Ink;
                self.diff_theme_bg = ink.bg;
                self.diff_state.set_highlighter(diff_highlighter(&ink));
                if let Some(io_tx) = &self.io_tx {
                    let _ = io_tx.try_send(IoCmd::SaveThemePref("ink"));
                }
                tx.instant();
            }
            "Use Paper" => {
                let paper = highlight::paper();
                self.theme_pref = ThemePref::Paper;
                self.diff_theme_bg = paper.bg;
                self.diff_state.set_highlighter(diff_highlighter(&paper));
                if let Some(io_tx) = &self.io_tx {
                    let _ = io_tx.try_send(IoCmd::SaveThemePref("paper"));
                }
                tx.instant();
            }
            _ => {
                if let Some(branch) = name.strip_prefix("Checkout ") {
                    self.request_checkout(branch.to_owned(), tx);
                }
            }
        }
    }
}

fn build_file_tree(entries: Vec<FileEntry>) -> TreeModel<FileNode<PathBuf>> {
    let pairs = entries.into_iter().filter(|e| !e.is_dir).map(|e| {
        let slash = path_to_slash(&e.rel);
        (slash, e.rel)
    });
    let mut tree = FileTree::new().key("pen/tree").build(pairs);
    tree.expand_all();
    tree
}

fn path_to_slash(p: &std::path::Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn tree_label(n: &FileNode<PathBuf>) -> &str {
    n.label()
}

/// Startup preferences read on the Tokio worker: theme choice (`theme` or
/// `theme.json`) and the one-shot `launched` marker for the 200ms first-launch
/// page fade.
fn read_startup_prefs_blocking() -> (ThemePref, bool) {
    let Some(dirs) = AppDirs::new("pen") else {
        return (ThemePref::System, false);
    };
    let cfg = dirs.config();
    let launched_file = cfg.join("launched");
    let first_launch = !launched_file.exists();
    if first_launch && std::fs::create_dir_all(cfg).is_ok() {
        let _ = std::fs::write(&launched_file, b"1\n");
    }

    let custom_json = cfg.join("theme.json");
    if let Ok(raw) = std::fs::read_to_string(custom_json)
        && let Ok(theme) = load_theme(&raw)
    {
        return (ThemePref::Custom(Box::new(theme)), first_launch);
    }
    let choice_path = cfg.join("theme");
    let pref = match std::fs::read_to_string(choice_path)
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("ink") => ThemePref::Ink,
        Some("paper") => ThemePref::Paper,
        _ => ThemePref::System,
    };
    (pref, first_launch)
}

fn write_theme_pref_blocking(choice: &str) {
    let Some(dirs) = AppDirs::new("pen") else {
        return;
    };
    if std::fs::create_dir_all(dirs.config()).is_err() {
        return;
    }
    let tmp = dirs.config().join(".theme.pen-tmp");
    let dst = dirs.config().join("theme");
    if std::fs::write(&tmp, choice.as_bytes()).is_ok() {
        let _ = std::fs::rename(&tmp, &dst);
    }
}

fn view(s: &Pen, cx: &mut Cx<Pen>) -> El {
    let theme = s.theme_pref.resolve(cx.system_dark());
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let rule = fg.alpha(0.20);
    cx.focus_ring(1.0, 1.0, fg);
    let k = build_kit(&theme);

    let main_surface = match (&s.project, &s.page) {
        (None, OpenPage::Empty) => empty_folder_screen(s, &theme),
        (_, OpenPage::Empty) => col()
            .key("pen/page-blank")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(bg)
            .transition(Transition::none()),
        (_, OpenPage::Notice(msg)) => col()
            .key("pen/notice")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(bg)
            .center()
            .transition(Transition::none())
            .child(text(msg.clone()).font(UI_FONT).color(faint)),
        (_, OpenPage::Diff { title }) => {
            let resolved_theme = theme.clone();
            diff_page_view(
                s,
                title,
                &theme,
                &k,
                |s: &Pen| &s.diff_state,
                |s: &mut Pen| &mut s.diff_state,
                |s: &mut Pen, tx: &mut Tx| {
                    let next = match s.diff_state.mode() {
                        DiffMode::Unified => DiffMode::Split,
                        DiffMode::Split => DiffMode::Unified,
                    };
                    s.diff_state.set_mode(next);
                    tx.instant();
                },
            )
            .on_layout_tx(move |s: &mut Pen, _r: [f32; 4], tx: &mut Tx| {
                if s.diff_theme_bg != resolved_theme.bg {
                    s.diff_theme_bg = resolved_theme.bg;
                    s.diff_state
                        .set_highlighter(diff_highlighter(&resolved_theme));
                    tx.instant();
                } else {
                    tx.live_only();
                }
            })
        }
        (_, OpenPage::Editor(ed)) => editor_page_container(ed, &theme),
    };

    let mut page_stack = col()
        .key("pen/main")
        .w_full()
        .h_full()
        .fill(bg)
        .border_sides([1.0, 0.0, 0.0, 0.0], rule);
    if s.find_open {
        page_stack = page_stack.child(find_bar(s, &theme));
    }
    if s.git_panel.commit_open {
        page_stack = page_stack.child(commit_bar(
            &s.git_panel.commit_message,
            &theme,
            |s: &mut Pen| &mut s.git_panel.commit_message,
            |s: &mut Pen, tx: &mut Tx| s.submit_commit(tx),
            |s: &mut Pen, tx: &mut Tx| s.close_commit_bar(tx),
        ));
    }
    if let Some(confirm) = &s.git_panel.confirm {
        page_stack = page_stack.child(confirm_bar(
            &confirm.prompt(),
            &theme,
            |s: &mut Pen, tx: &mut Tx| s.confirm_pending(tx),
            |s: &mut Pen, tx: &mut Tx| {
                s.git_panel.confirm = None;
                tx.instant();
            },
        ));
    }
    page_stack = page_stack.child(main_surface);
    if let Some(status) = &s.status {
        page_stack = page_stack.child(
            row()
                .key("pen/status")
                .w_full()
                .h(32.0)
                .px(16.0)
                .center_y()
                .fill(bg)
                .border_sides([1.0, 0.0, 0.0, 0.0], rule)
                .transition(Transition::none())
                .child(text(status.clone()).font(UI_FONT).color(fg)),
        );
    }

    let mut toolbar = Toolbar::new().title("Pen");
    if let Some(rel) = &s.active_path {
        let dirty = matches!(&s.page, OpenPage::Editor(ed) if ed.buffer.is_dirty());
        let label = if dirty {
            format!("{} · edited", path_to_slash(rel))
        } else {
            path_to_slash(rel)
        };
        toolbar = toolbar.trailing(
            text(label)
                .key("pen/active-file")
                .font(UI_FONT)
                .color(faint)
                .transition(Transition::none()),
        );
    }

    let sidebar_body = match s.git_panel.mode {
        SidebarMode::Files => {
            let git_labels: Rc<HashMap<PathBuf, String>> = Rc::new(
                s.git_status
                    .entries
                    .iter()
                    .filter_map(|e| {
                        let lbl = status_label(e);
                        (!lbl.is_empty()).then(|| (e.rel.clone(), lbl))
                    })
                    .collect(),
            );
            let tree = Tree::new(|s: &Pen| &s.files, |s: &mut Pen| &mut s.files, tree_label)
                .name("Files")
                .row(move |node, r| {
                    let item = TreeItem::file(node, r);
                    match node.data.as_ref().and_then(|rel| git_labels.get(rel)) {
                        Some(lbl) => item.badge(lbl.clone()),
                        None => item,
                    }
                })
                .on_select(|s: &mut Pen, id: &str, tx: &mut Tx| s.request_open_file(id, tx))
                .on_open(|s: &mut Pen, id: &str, tx: &mut Tx| s.request_open_file(id, tx))
                .build(s, &k)
                .grow(1.0)
                .min_h(0.0);
            col()
                .key("pen/sidebar-files")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(8.0)
                .child(tree)
        }
        SidebarMode::Git => {
            let changes = changes_section(
                &s.git_status.entries,
                s.git_panel.selected_change.as_deref(),
                &theme,
                |s: &mut Pen, rel: PathBuf, tx: &mut Tx| {
                    s.git_panel.selected_change = Some(rel.clone());
                    s.git_panel.selected_commit = None;
                    if let (Some(proj), Some(io_tx)) = (&s.project, &s.io_tx) {
                        let _ = io_tx.try_send(IoCmd::DiffWorktree {
                            root: proj.root.clone(),
                            rel,
                        });
                    }
                    tx.instant();
                },
                |s: &mut Pen, rel: PathBuf, _tx: &mut Tx| {
                    if let (Some(proj), Some(io_tx)) = (&s.project, &s.io_tx) {
                        let _ = io_tx.try_send(IoCmd::StageFile {
                            root: proj.root.clone(),
                            rel,
                        });
                    }
                },
                |s: &mut Pen, rel: PathBuf, tx: &mut Tx| s.request_discard(rel, tx),
            );
            let branches = branches_section(
                &s.git_status.branches,
                s.git_status.current_branch.as_deref(),
                &theme,
                |s: &mut Pen, branch: String, tx: &mut Tx| s.request_checkout(branch, tx),
            );
            let history = history_section(
                &s.git_status.history,
                s.git_panel.selected_commit.as_deref(),
                &theme,
                |s: &mut Pen, id: String, tx: &mut Tx| {
                    s.git_panel.selected_commit = Some(id.clone());
                    s.git_panel.selected_change = None;
                    if let (Some(proj), Some(io_tx)) = (&s.project, &s.io_tx) {
                        let _ = io_tx.try_send(IoCmd::DiffCommit {
                            root: proj.root.clone(),
                            id,
                        });
                    }
                    tx.instant();
                },
            );
            git_dock_container(vec![changes, branches, history])
        }
    };
    let mut sidebar = col()
        .key("pen/sidebar")
        .w_full()
        .h_full()
        .fill(bg)
        .border_sides([1.0, 1.0, 0.0, 0.0], rule);
    if s.git_status.is_repo {
        let files_active = s.git_panel.mode == SidebarMode::Files;
        let git_active = s.git_panel.mode == SidebarMode::Git;
        let tab = |key: &str, label: &str, active: bool, target: SidebarMode| {
            row()
                .key(key)
                .role("tab")
                .label(label)
                .focusable(true)
                .h(26.0)
                .px(8.0)
                .radius(6.0)
                .center_y()
                .cursor(Cursor::Pointer)
                .fill(if active { fg.alpha(0.08) } else { bg })
                .hover(fg.alpha(0.06))
                .transition(Transition::none())
                .on_press_tx(move |s: &mut Pen, tx: &mut Tx| {
                    s.git_panel.mode = target;
                    tx.instant();
                })
                .child(
                    text(label.to_owned())
                        .font(if active { UI_MEDIUM_FONT } else { UI_FONT })
                        .color(if active { fg } else { faint }),
                )
        };
        sidebar = sidebar.child(
            row()
                .key("pen/sidebar-tabs")
                .w_full()
                .h(34.0)
                .px(8.0)
                .gap(4.0)
                .center_y()
                .border_sides([0.0, 0.0, 1.0, 0.0], rule)
                .child(tab(
                    "pen/tab-files",
                    "Files",
                    files_active,
                    SidebarMode::Files,
                ))
                .child(tab("pen/tab-git", "Git", git_active, SidebarMode::Git)),
        );
    }
    sidebar = sidebar.child(sidebar_body);

    let mut split = SplitView::new(page_stack)
        .sidebar(&s.sidebar, |s: &mut Pen| &mut s.sidebar, sidebar)
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .pane_motion(PaneMotion::new().spring(Spring::new(0.35, 0.0)))
        .reduced_motion(cx.is_reduced_motion());
    if s.project.is_none() && matches!(s.page, OpenPage::Empty) {
        // Design: on the empty screen, "Open a folder" is the only control.
        split = split.hide_sidebar_toggle();
    }
    let shell_el = split.build(&k);
    rune_kit::root(shell_el, s.palette_open.then(|| palette_overlay(s, &theme)))
}

/// Empty state: "Open a folder" centered alone on the page. First launch only
/// fades opacity over 200 ms; every later launch is instant.
fn empty_folder_screen(s: &Pen, theme: &Theme) -> El {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let open_btn = row()
        .key("pen/open-folder")
        .role("button")
        .label("Open a folder")
        .focusable(true)
        .cursor(Cursor::Pointer)
        .h(40.0)
        .px(20.0)
        .center()
        .radius(6.0)
        .fill(bg)
        .border(1.0, fg.alpha(0.20))
        .hover(fg.alpha(0.06))
        .press(fg.alpha(0.12))
        .transition(Transition::none())
        .on_press_tx(|s: &mut Pen, tx: &mut Tx| s.prompt_open_folder(tx))
        .child(text("Open a folder").font(UI_MEDIUM_FONT).color(fg));

    col()
        .key("pen/empty")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(bg)
        .center()
        .opacity(s.page_opacity)
        .opacity_motion(Spring::new(0.20, 0.0))
        .transition(Transition::none())
        .child(open_btn)
}

/// Single-line Find bar pinned directly above the writing surface (never a dialog).
fn find_bar(s: &Pen, theme: &Theme) -> El {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let rule = fg.alpha(0.20);

    let input = text_input(&s.find_query, FIND_FONT)
        .key("pen/find-input")
        .label("Find")
        .single_line()
        .placeholder("Find", faint)
        .color(fg)
        .caret(fg, fg.alpha(0.12))
        .grow(1.0)
        .h(20.0)
        .on_edit(|s: &mut Pen| &mut s.find_query)
        .on_input_change(|s: &mut Pen, tx: &mut Tx| {
            let q = s.find_query.text.clone();
            if let OpenPage::Editor(ed) = &mut s.page {
                ed.find_and_select(&q, false);
            }
            tx.instant();
        })
        .on_input_key(|s: &mut Pen, ev: &KeyEvent, tx: &mut Tx| match ev.key {
            Key::Escape => {
                s.close_find(tx);
                true
            }
            Key::Enter => {
                let q = s.find_query.text.clone();
                if let OpenPage::Editor(ed) = &mut s.page {
                    ed.find_and_select(&q, true);
                }
                tx.instant();
                true
            }
            _ => false,
        });

    row()
        .key("pen/find-bar")
        .w_full()
        .h(38.0)
        .pl(48.0)
        .pr(20.0)
        .gap(12.0)
        .center_y()
        .fill(bg)
        .border_sides([0.0, 0.0, 1.0, 0.0], rule)
        .instant_changes()
        .transition(Transition::none())
        .child(text("Find").font(UI_FONT).color(faint))
        .child(input)
}

/// Instant command palette (`⌘K` on macOS, `Ctrl+K` elsewhere): solid surface,
/// 6px chrome radius, 1px hairline border, zero animation.
fn palette_overlay(s: &Pen, theme: &Theme) -> El {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let rule = fg.alpha(0.20);

    let all_cmds = s.palette_commands();
    let hits = filter_command_list(&s.palette_query.text, &all_cmds);
    let selected = if hits.is_empty() {
        0
    } else {
        s.palette_index.min(hits.len() - 1)
    };

    let input = text_input(&s.palette_query, UI_FONT)
        .key("pen/palette-input")
        .label("Command")
        .single_line()
        .placeholder("Type a command", faint)
        .color(fg)
        .caret(fg, fg.alpha(0.12))
        .w_full()
        .h(28.0)
        .on_edit(|s: &mut Pen| &mut s.palette_query)
        .on_input_change(|s: &mut Pen, tx: &mut Tx| {
            s.palette_index = 0;
            tx.instant();
        })
        .on_input_key(|s: &mut Pen, ev: &KeyEvent, tx: &mut Tx| {
            let all_cmds = s.palette_commands();
            let hits = filter_command_list(&s.palette_query.text, &all_cmds);
            match ev.key {
                Key::Escape => {
                    s.close_palette(tx);
                    true
                }
                Key::Down => {
                    if !hits.is_empty() {
                        s.palette_index = (s.palette_index + 1) % hits.len();
                        tx.instant();
                    }
                    true
                }
                Key::Up => {
                    if !hits.is_empty() {
                        s.palette_index = s.palette_index.checked_sub(1).unwrap_or(hits.len() - 1);
                        tx.instant();
                    }
                    true
                }
                Key::Enter => {
                    if let Some(cmd) = hits.get(s.palette_index.min(hits.len().saturating_sub(1))) {
                        let cmd = cmd.clone();
                        s.run_command(&cmd, tx);
                    }
                    true
                }
                _ => false,
            }
        });

    let mut list = col()
        .key("pen/palette-list")
        .w_full()
        .px(6.0)
        .py(6.0)
        .gap(2.0);
    for (idx, cmd) in hits.iter().enumerate() {
        let active = idx == selected;
        let cmd_owned = cmd.clone();
        let mut r = row()
            .key(format!("pen/cmd-{cmd}"))
            .role("option")
            .label(cmd.clone())
            .w_full()
            .h(34.0)
            .px(10.0)
            .radius(4.0)
            .center_y()
            .justify_between()
            .cursor(Cursor::Pointer)
            .fill(if active { fg.alpha(0.10) } else { bg })
            .hover(fg.alpha(0.08))
            .transition(Transition::none())
            .on_press_tx(move |s: &mut Pen, tx: &mut Tx| s.run_command(&cmd_owned, tx))
            .child(
                text(cmd.clone())
                    .font(if active { UI_MEDIUM_FONT } else { UI_FONT })
                    .color(fg),
            );
        if let Some(hint) = shortcut_hint(cmd) {
            r = r.child(text(hint).font(UI_FONT).color(faint));
        }
        list = list.child(r);
    }

    let card = col()
        .key("pen/palette")
        .w(440.0)
        .mt(76.0)
        .radius(6.0)
        .fill(bg)
        .border(1.0, fg.alpha(0.24))
        .clip_rounded(6.0)
        .instant_changes()
        .transition(Transition::none())
        .modal_tx(true, |s: &mut Pen, tx: &mut Tx| s.close_palette(tx))
        .child(
            row()
                .w_full()
                .h(42.0)
                .px(16.0)
                .center_y()
                .border_sides([0.0, 0.0, 1.0, 0.0], rule)
                .child(input),
        )
        .child(list);

    let scrim = if bg.luminance() < 0.2 {
        rune::Color::BLACK.alpha(0.42)
    } else {
        fg.alpha(0.10)
    };
    el().key("pen/palette-backdrop")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(scrim)
        .transition(Transition::none())
        .child(col().w_full().items_center().child(card))
}

fn editor_page_container(ed: &EditorState, theme: &Theme) -> El {
    col()
        .key("pen/page")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .cursor(Cursor::Text)
        .transition(Transition::none())
        .on_layout_tx(|s: &mut Pen, r: [f32; 4], tx: &mut Tx| {
            if let OpenPage::Editor(ed) = &mut s.page
                && (ed.viewport_height - r[3]).abs() > 0.5
            {
                ed.viewport_height = r[3].max(60.0);
            } else {
                tx.live_only();
            }
        })
        .on_wheel(|s: &mut Pen, ev: WheelEvent, tx: &mut Tx| {
            if let OpenPage::Editor(ed) = &mut s.page {
                ed.scroll_by(ev.dy);
                tx.instant();
                return true;
            }
            false
        })
        .on_drag(|s: &mut Pen, drag: &mut Drag, tx: &mut Tx| {
            let shift = tx.mods().shift;
            if let OpenPage::Editor(ed) = &mut s.page {
                ed.on_drag(drag, shift);
                tx.instant();
            }
        })
        .on_paste(|s: &mut Pen, clip: &str, tx: &mut Tx| {
            if let OpenPage::Editor(ed) = &mut s.page {
                ed.insert_text(clip);
                tx.instant();
            }
        })
        .child(editor_view_themed(ed, theme))
}

fn on_key(s: &mut Pen, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.is_command('k') {
        s.toggle_palette(tx);
        return true;
    }
    if s.palette_open {
        if ev.key == Key::Escape {
            s.close_palette(tx);
            return true;
        }
        return false;
    }
    if ev.key == Key::Escape && s.git_panel.confirm.is_some() {
        s.git_panel.confirm = None;
        tx.instant();
        return true;
    }
    if ev.is_command('o') {
        s.prompt_open_folder(tx);
        return true;
    }
    if ev.is_command('f') {
        s.open_find(tx);
        return true;
    }
    if shell::is_sidebar_toggle(ev) && s.project.is_some() {
        s.sidebar.toggle();
        return true;
    }
    if ev.mods.cmd {
        match ev.key {
            Key::Command('s') => return s.request_save(),
            Key::Command('c') => {
                if let OpenPage::Editor(ed) = &s.page
                    && let Some(sel) = ed.selected_text()
                {
                    tx.copy(sel);
                    return true;
                }
            }
            Key::Command('x') => {
                if let OpenPage::Editor(ed) = &mut s.page
                    && let Some(cut) = ed.cut_selection()
                {
                    tx.copy(cut);
                    tx.instant();
                    return true;
                }
            }
            _ => {}
        }
    }
    if let OpenPage::Diff { .. } = &mut s.page
        && s.diff_state.key(ev, tx)
    {
        return true;
    }
    if let OpenPage::Editor(ed) = &mut s.page
        && ed.focused
        && ed.on_key(ev)
    {
        tx.instant();
        return true;
    }
    false
}

fn on_ime(s: &mut Pen, ime: &Ime, tx: &mut Tx) -> bool {
    if s.palette_open || s.find_open {
        return false;
    }
    if let OpenPage::Editor(ed) = &mut s.page
        && ed.focused
        && ed.on_ime(ime)
    {
        tx.instant();
        return true;
    }
    false
}

type SceneFn = fn(&mut Pen);

/// Named scenes for headless design inspection (`--snap ... --scene <name>`).
const SCENES: &[(&str, SceneFn)] = &[
    ("empty-paper", |s| {
        s.theme_pref = ThemePref::Paper;
    }),
    ("empty-ink", |s| {
        s.theme_pref = ThemePref::Ink;
    }),
    ("editor-paper", |s| {
        s.theme_pref = ThemePref::Paper;
        populate_demo_editor(s);
    }),
    ("editor-ink", |s| {
        s.theme_pref = ThemePref::Ink;
        populate_demo_editor(s);
    }),
    ("palette-ink", |s| {
        s.theme_pref = ThemePref::Ink;
        populate_demo_editor(s);
        s.palette_open = true;
    }),
    ("palette-paper", |s| {
        s.theme_pref = ThemePref::Paper;
        populate_demo_editor(s);
        s.palette_open = true;
    }),
    ("find-paper", |s| {
        s.theme_pref = ThemePref::Paper;
        populate_demo_editor(s);
        s.find_open = true;
        s.find_query = TextEdit::new("Document");
        if let OpenPage::Editor(ed) = &mut s.page {
            ed.focused = false;
            ed.find_and_select("Document", false);
        }
    }),
    ("diff-paper", |s| {
        s.theme_pref = ThemePref::Paper;
        populate_demo_diff(s, &highlight::paper());
    }),
    ("diff-ink", |s| {
        s.theme_pref = ThemePref::Ink;
        populate_demo_diff(s, &highlight::ink());
    }),
];

fn populate_demo_editor(s: &mut Pen) {
    let sample = "\
//! Pen: a code editor that feels like a pen on paper.

use std::path::Path;

pub struct Document {
    pub title: String,
    pub saved: bool,
}

impl Document {
    pub fn open(path: &Path) -> Option<Self> {
        let title = path.file_name()?.to_str()?.to_owned();
        Some(Document { title, saved: true })
    }
}
";
    let mut buf = Buffer::from_str(sample);
    buf.mark_saved();
    let mut ed = EditorState::from_buffer(buf).with_language(Some("rust"));
    ed.caret = 92;
    ed.anchor = 92;
    s.project = Some(Project {
        root: PathBuf::from("."),
    });
    s.files = FileTree::new().key("pen/tree").build([
        ("Cargo.toml", PathBuf::from("Cargo.toml")),
        ("README.md", PathBuf::from("README.md")),
        (
            "crates/app/src/main.rs",
            PathBuf::from("crates/app/src/main.rs"),
        ),
        (
            "crates/app/src/editor.rs",
            PathBuf::from("crates/app/src/editor.rs"),
        ),
        (
            "crates/buffer/src/lib.rs",
            PathBuf::from("crates/buffer/src/lib.rs"),
        ),
        (
            "crates/highlight/src/lib.rs",
            PathBuf::from("crates/highlight/src/lib.rs"),
        ),
        (
            "crates/project/src/lib.rs",
            PathBuf::from("crates/project/src/lib.rs"),
        ),
    ]);
    s.files.expand_all();
    s.files.select("crates/app/src/editor.rs");
    s.active_path = Some(PathBuf::from("crates/app/src/editor.rs"));
    s.page = OpenPage::Editor(ed);
    if !s.sidebar.is_open() {
        s.sidebar.toggle();
    }
}

fn populate_demo_diff(s: &mut Pen, theme: &Theme) {
    populate_demo_editor(s);
    s.active_path = None;
    s.git_status = GitSnapshot {
        is_repo: true,
        current_branch: Some("main".into()),
        branches: vec!["main".into(), "phase-2-git".into()],
        entries: vec![
            pen_git::StatusEntry {
                rel: PathBuf::from("crates/app/src/editor.rs"),
                added: 3,
                removed: 1,
                tracked: true,
            },
            pen_git::StatusEntry {
                rel: PathBuf::from("crates/git/src/lib.rs"),
                added: 0,
                removed: 0,
                tracked: false,
            },
        ],
        history: vec![
            pen_git::CommitSummary {
                id: "974428d".into(),
                summary: "feat: show git status in the file tree".into(),
            },
            pen_git::CommitSummary {
                id: "cc1381a".into(),
                summary: "feat: read git status through gix".into(),
            },
        ],
    };
    let patch = "\
diff --git a/crates/app/src/editor.rs b/crates/app/src/editor.rs
--- a/crates/app/src/editor.rs
+++ b/crates/app/src/editor.rs
@@ -9,5 +9,7 @@ impl Document {
     pub fn open(path: &Path) -> Option<Self> {
         let title = path.file_name()?.to_str()?.to_owned();
-        Some(Document { title, saved: true })
+        let saved = path.exists();
+        let doc = Document { title, saved };
+        Some(doc)
     }
 }
";
    s.diff_theme_bg = theme.bg;
    s.diff_state.set_highlighter(diff_highlighter(theme));
    s.diff_state.set_files(parse_diff(patch));
    s.page = OpenPage::Diff {
        title: "Changes — crates/app/src/editor.rs".into(),
    };
}

fn main() {
    rune::prewarm_gpu();
    if let Err(e) = rune::set_app_icon(include_bytes!("../assets/icon.png").as_slice()) {
        eprintln!("pen: app icon not set: {e}");
    }
    let chrome = Chrome {
        full_size_content: true,
        traffic_lights: Some((16.0, 17.0)),
        drag_regions: true,
    };
    let spec = AppSpec {
        fonts: pen::FONTS,
        families: pen::FAMILIES,
        min_size: Some((640.0, 420.0)),
        scenes: SCENES,
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Pen", (1100.0, 720.0), init, view)
    };
    rune::App::new(spec).chrome(chrome).on_ime(on_ime).run();
}
