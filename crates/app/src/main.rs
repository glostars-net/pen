//! Pen: a code editor that feels like a pen on paper, drawn by Rune.
//!
//! Phase 1 (plan/2026-11-10-pen.md): one window, a folder tree, a rope-backed
//! editor page, save, find, and a command palette. The UI thread never does
//! disk or watcher work; a Tokio worker handles all filesystem calls and wakes
//! the window via `Waker`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use buffer::Buffer;
use highlight::{Theme, lang_for_path, load_theme};
use pen::disk::{DiskEvent, on_disk_change};
use pen::editor::EditorState;
use pen::theme_kit::{UI_FONT, build_kit, rgb_color};
use project::watch::{ProjectWatcher, WatchEvent, watch_project_with};
use project::{FileEntry, Project, ReadFile};
use rune::dirs::AppDirs;
use rune::{
    AppSpec, Cursor, Cx, Drag, El, Ime, Key, KeyEvent, Spring, Transition, Tx, Waker, WheelEvent,
    col, text,
};
use rune_kit::shell::{self, PaneMotion, PaneState, SplitView, Toolbar};
use rune_kit::{FileNode, FileTree, Tree, TreeItem, TreeModel};
use tokio::sync::{mpsc, watch};

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
                if system_dark { highlight::ink() } else { highlight::paper() }
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
    ReadFile { project: Project, rel: PathBuf },
    ReloadCleanFile { project: Project, rel: PathBuf },
    SaveFile { project: Project, rel: PathBuf, bytes: Vec<u8> },
    SaveThemePref(&'static str),
}

/// Answers sent back from the Tokio disk worker to `poll`.
enum IoMsg {
    PrefsLoaded { theme_pref: ThemePref },
    ProjectListed {
        project: Project,
        entries: Vec<FileEntry>,
        watch_rx: Option<watch::Receiver<Option<WatchEvent>>>,
    },
    TreeRefreshed { entries: Vec<FileEntry> },
    FileRead { rel: PathBuf, result: ReadFile },
    FileReloaded { rel: PathBuf, result: ReadFile },
    FileSaved { rel: PathBuf },
    Error(String),
}

enum OpenPage {
    Empty,
    Editor(EditorState),
    Notice(String),
}

struct Pen {
    /// Folder tree pane. Width per the plan: 248 pt, resizable 160–480.
    sidebar: PaneState,
    project: Option<Project>,
    files: TreeModel<FileNode<PathBuf>>,
    active_path: Option<PathBuf>,
    page: OpenPage,
    /// Unsaved buffers kept in memory when switching files so clicking another
    /// file in the tree never discards work.
    unsaved: BTreeMap<PathBuf, EditorState>,
    theme_pref: ThemePref,
    status: Option<String>,
    watch_rx: Option<watch::Receiver<Option<WatchEvent>>>,
    io_tx: Option<mpsc::Sender<IoCmd>>,
    io_rx: Option<mpsc::Receiver<IoMsg>>,
}

fn init() -> Pen {
    Pen {
        sidebar: PaneState::new(248.0).range(160.0, 480.0),
        project: None,
        files: TreeModel::new().key("pen/tree"),
        active_path: None,
        page: OpenPage::Empty,
        unsaved: BTreeMap::new(),
        theme_pref: ThemePref::System,
        status: None,
        watch_rx: None,
        io_tx: None,
        io_rx: None,
    }
}

fn setup(s: &mut Pen, waker: Waker) {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<IoCmd>(32);
    let (msg_tx, msg_rx) = mpsc::channel::<IoMsg>(32);
    if let Some(arg) = rune::extra_args().into_iter().find(|a| !a.starts_with('-')) {
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
                // Load stored theme preference off the UI thread.
                let theme_pref = tokio::task::spawn_blocking(read_theme_pref_blocking)
                    .await
                    .unwrap_or(ThemePref::System);
                let _ = msg_tx.send(IoMsg::PrefsLoaded { theme_pref }).await;
                waker();

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
                                    let watch_rx = match watch_project_with(&project, move || w()) {
                                        Ok((guard, rx)) => {
                                            active_watcher = Some(guard);
                                            Some(rx)
                                        }
                                        Err(_) => None,
                                    };
                                    IoMsg::ProjectListed { project, entries, watch_rx }
                                }
                                Err(e) => IoMsg::Error(e),
                            }
                        }
                        IoCmd::RefreshTree(project) => {
                            tokio::task::spawn_blocking(move || project.list())
                                .await
                                .map_err(|e| e.to_string())
                                .and_then(|r| r.map_err(|e| e.to_string()))
                                .map_or_else(IoMsg::Error, |entries| IoMsg::TreeRefreshed { entries })
                        }
                        IoCmd::ReadFile { project, rel } => {
                            tokio::task::spawn_blocking(move || {
                                let result = project.read(&rel)?;
                                Ok::<_, project::ProjectError>((rel, result))
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()))
                            .map_or_else(IoMsg::Error, |(rel, result)| {
                                IoMsg::FileRead { rel, result }
                            })
                        }
                        IoCmd::ReloadCleanFile { project, rel } => {
                            tokio::task::spawn_blocking(move || {
                                let result = project.read(&rel)?;
                                Ok::<_, project::ProjectError>((rel, result))
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()))
                            .map_or_else(IoMsg::Error, |(rel, result)| {
                                IoMsg::FileReloaded { rel, result }
                            })
                        }
                        IoCmd::SaveFile { project, rel, bytes } => {
                            tokio::task::spawn_blocking(move || {
                                project.write_atomic(&rel, &bytes)?;
                                Ok::<_, project::ProjectError>(rel)
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()))
                            .map_or_else(IoMsg::Error, |rel| IoMsg::FileSaved { rel })
                        }
                        IoCmd::SaveThemePref(choice) => {
                            let _ = tokio::task::spawn_blocking(move || {
                                write_theme_pref_blocking(choice)
                            })
                            .await;
                            continue;
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
}

fn poll(s: &mut Pen, _now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;

    // Non-blocking read of the latest watcher event (`tokio::sync::watch`).
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
                        let _ = io_tx.try_send(IoCmd::ReloadCleanFile { project, rel: rel.clone() });
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
            IoMsg::PrefsLoaded { theme_pref } => {
                s.theme_pref = theme_pref;
                tx.instant();
            }
            IoMsg::ProjectListed { project, entries, watch_rx } => {
                s.files = build_file_tree(entries);
                s.project = Some(project);
                s.watch_rx = watch_rx;
                s.active_path = None;
                s.page = OpenPage::Empty;
                s.unsaved.clear();
                s.status = None;
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
                        OpenPage::Editor(EditorState::from_buffer(buf).with_language(lang))
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
            IoMsg::Error(err) => {
                s.status = Some(err);
                tx.instant();
            }
        }
    }
    true
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

impl Pen {
    fn stash_current_if_dirty(&mut self) {
        let Some(prev) = self.active_path.take() else { return };
        if let OpenPage::Editor(ed) = std::mem::replace(&mut self.page, OpenPage::Empty)
            && ed.buffer.is_dirty()
        {
            self.unsaved.insert(prev, ed);
        }
    }

    fn request_open_file(&mut self, id: &str, tx: &mut Tx) {
        let Some(rel) = self.files.get(id).and_then(|n| n.data.clone()) else { return };
        if self.active_path.as_ref() == Some(&rel) {
            return;
        }
        if let Some(stashed) = self.unsaved.remove(&rel) {
            self.stash_current_if_dirty();
            self.active_path = Some(rel);
            self.page = OpenPage::Editor(stashed);
            self.status = None;
            tx.instant();
            return;
        }
        let (Some(project), Some(io_tx)) = (self.project.clone(), &self.io_tx) else { return };
        let _ = io_tx.try_send(IoCmd::ReadFile { project, rel });
    }

    fn request_save(&mut self) -> bool {
        let (Some(project), Some(rel), Some(io_tx)) =
            (self.project.clone(), self.active_path.clone(), &self.io_tx)
        else {
            return false;
        };
        let OpenPage::Editor(ed) = &self.page else { return false };
        let _ = io_tx.try_send(IoCmd::SaveFile {
            project,
            rel,
            bytes: ed.buffer.text().into_bytes(),
        });
        true
    }
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

fn read_theme_pref_blocking() -> ThemePref {
    let Some(dirs) = AppDirs::new("pen") else { return ThemePref::System };
    let custom_json = dirs.config().join("theme.json");
    if let Ok(raw) = std::fs::read_to_string(custom_json)
        && let Ok(theme) = load_theme(&raw)
    {
        return ThemePref::Custom(Box::new(theme));
    }
    let choice_path = dirs.config().join("theme");
    match std::fs::read_to_string(choice_path).ok().as_deref().map(str::trim) {
        Some("ink") => ThemePref::Ink,
        Some("paper") => ThemePref::Paper,
        _ => ThemePref::System,
    }
}

fn write_theme_pref_blocking(choice: &str) {
    let Some(dirs) = AppDirs::new("pen") else { return };
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
    cx.focus_ring(1.0, 1.0, fg);
    let k = build_kit(&theme);

    let page = match &s.page {
        OpenPage::Editor(ed) => editor_page_container(ed, &theme),
        OpenPage::Notice(msg) => col()
            .key("pen/notice")
            .w_full()
            .h_full()
            .fill(bg)
            .center()
            .transition(Transition::none())
            .child(text(msg.clone()).font(UI_FONT).color(faint)),
        OpenPage::Empty => col()
            .key("pen/page-empty")
            .w_full()
            .h_full()
            .fill(bg)
            .transition(Transition::none()),
    };

    let tree = Tree::new(|s: &Pen| &s.files, |s: &mut Pen| &mut s.files, tree_label)
        .name("Files")
        .row(|n, r| TreeItem::file(n, r))
        .on_select(|s: &mut Pen, id: &str, tx: &mut Tx| s.request_open_file(id, tx))
        .on_open(|s: &mut Pen, id: &str, tx: &mut Tx| s.request_open_file(id, tx))
        .build(s, &k)
        .grow(1.0)
        .min_h(0.0);
    let sidebar = col().key("pen/sidebar").w_full().h_full().padding(8.0).fill(bg).child(tree);

    SplitView::new(page)
        .sidebar(&s.sidebar, |s: &mut Pen| &mut s.sidebar, sidebar)
        .toolbar(Toolbar::new().title("Pen"))
        .titlebar_inset(cx.titlebar_inset())
        .pane_motion(PaneMotion::new().spring(Spring::new(0.35, 0.0)))
        .reduced_motion(cx.is_reduced_motion())
        .build(&k)
}

fn editor_page_container(ed: &EditorState, theme: &Theme) -> El {
    col()
        .key("pen/page")
        .w_full()
        .h_full()
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
        .child(pen::editor::editor_view_themed(ed, theme))
}

fn on_key(s: &mut Pen, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if shell::is_sidebar_toggle(ev) {
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
    if let OpenPage::Editor(ed) = &mut s.page
        && ed.focused
        && ed.on_ime(ime)
    {
        tx.instant();
        return true;
    }
    false
}

fn main() {
    rune::prewarm_gpu();
    if let Err(e) = rune::set_app_icon(include_bytes!("../assets/icon.png").as_slice()) {
        eprintln!("pen: app icon not set: {e}");
    }
    let spec = AppSpec {
        fonts: pen::FONTS,
        families: pen::FAMILIES,
        min_size: Some((640.0, 420.0)),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Pen", (1100.0, 720.0), init, view)
    };
    rune::App::new(spec).on_ime(on_ime).run();
}
