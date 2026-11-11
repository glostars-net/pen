//! Pen: a code editor that feels like a pen on paper, drawn by Rune.
//!
//! Phase 1 (plan/2026-11-10-pen.md): one window, a folder tree, a rope-backed
//! editor page, save, find, and a command palette. The UI thread never does
//! disk or watcher work; a Tokio worker handles all filesystem calls and wakes
//! the window via `Waker`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use buffer::Buffer;
use pen::editor::{EditorState, PagePalette, editor_view};
use project::{FileEntry, Project, ReadFile};
use rune::{
    AppSpec, Cursor, Cx, Drag, El, Family, Ime, Key, KeyEvent, OpticalSize, Spring, TextStyle,
    Transition, Tx, Waker, WheelEvent, col, text,
};
use rune_kit::shell::{self, PaneMotion, PaneState, SplitView, Toolbar};
use rune_kit::{FileNode, FileTree, Kit, Tree, TreeItem, TreeModel};
use tokio::sync::mpsc;

const UI_FONT: TextStyle = TextStyle {
    size: 13.0,
    weight: 400.0,
    family: Family::Sans,
    line_height: 20.0,
    tracking: 0.0,
    variations: "",
    features: "",
    optical_size: OpticalSize::Auto,
    italic: false,
};

/// Requests sent from the UI thread to the Tokio disk worker.
enum IoCmd {
    OpenProject(PathBuf),
    ReadFile { project: Project, rel: PathBuf },
    SaveFile { project: Project, rel: PathBuf, bytes: Vec<u8> },
}

/// Answers sent back from the Tokio disk worker to `poll`.
enum IoMsg {
    ProjectListed { project: Project, entries: Vec<FileEntry> },
    FileRead { rel: PathBuf, result: ReadFile },
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
    status: Option<String>,
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
        status: None,
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
                while let Some(cmd) = cmd_rx.recv().await {
                    let reply = match cmd {
                        IoCmd::OpenProject(path) => {
                            tokio::task::spawn_blocking(move || {
                                let project = Project::open(path)?;
                                let entries = project.list()?;
                                Ok::<_, project::ProjectError>((project, entries))
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()))
                            .map_or_else(IoMsg::Error, |(project, entries)| {
                                IoMsg::ProjectListed { project, entries }
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
                            .map_or_else(IoMsg::Error, |(rel, result)| {
                                IoMsg::FileRead { rel, result }
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
                    };
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
    let Some(rx) = &mut s.io_rx else { return false };
    let mut batch = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        batch.push(msg);
    }
    if batch.is_empty() {
        return false;
    }
    for msg in batch {
        match msg {
            IoMsg::ProjectListed { project, entries } => {
                let pairs = entries.into_iter().filter(|e| !e.is_dir).map(|e| {
                    let slash = path_to_slash(&e.rel);
                    (slash, e.rel)
                });
                let mut tree = FileTree::new().key("pen/tree").build(pairs);
                tree.expand_all();
                s.files = tree;
                s.project = Some(project);
                s.active_path = None;
                s.page = OpenPage::Empty;
                s.unsaved.clear();
                s.status = None;
                tx.instant();
            }
            IoMsg::FileRead { rel, result } => {
                // Stash the outgoing buffer if it has unsaved edits.
                s.stash_current_if_dirty();
                s.active_path = Some(rel);
                s.page = match result {
                    ReadFile::Utf8(src) => {
                        let mut buf = Buffer::from_str(&src);
                        buf.mark_saved();
                        OpenPage::Editor(EditorState::from_buffer(buf))
                    }
                    ReadFile::NotUtf8 => OpenPage::Notice("This file is not UTF-8.".into()),
                };
                s.status = None;
                tx.instant();
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

fn view(s: &Pen, cx: &mut Cx<Pen>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let paper = PagePalette::paper();
    let page = match &s.page {
        OpenPage::Editor(ed) => col()
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
            .child(editor_view(ed)),
        OpenPage::Notice(msg) => col()
            .key("pen/notice")
            .w_full()
            .h_full()
            .fill(paper.bg)
            .center()
            .transition(Transition::none())
            .child(text(msg.clone()).font(UI_FONT).color(paper.faint)),
        OpenPage::Empty => col()
            .key("pen/page-empty")
            .w_full()
            .h_full()
            .fill(paper.bg)
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
    let sidebar = col().key("pen/sidebar").w_full().h_full().padding(8.0).child(tree);

    SplitView::new(page)
        .sidebar(&s.sidebar, |s: &mut Pen| &mut s.sidebar, sidebar)
        .toolbar(Toolbar::new().title("Pen"))
        .titlebar_inset(cx.titlebar_inset())
        .pane_motion(PaneMotion::new().spring(Spring::new(0.35, 0.0)))
        .reduced_motion(cx.is_reduced_motion())
        .build(&k)
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
