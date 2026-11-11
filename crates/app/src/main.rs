//! Pen: a code editor that feels like a pen on paper, drawn by Rune.
//!
//! Phase 1 (plan/2026-11-10-pen.md): one window, a folder tree, a rope-backed
//! editor page, save, find, and a command palette. The UI thread never does
//! disk or watcher work; a Tokio worker handles all filesystem calls and wakes
//! the window via `Waker`.

use std::path::PathBuf;

use pen::editor::{EditorState, editor_view};
use project::{FileEntry, Project};
use rune::{
    AppSpec, Cursor, Cx, Drag, El, Ime, Key, KeyEvent, Spring, Tx, Waker, WheelEvent, col,
};
use rune_kit::shell::{self, PaneMotion, PaneState, SplitView, Toolbar};
use rune_kit::{FileNode, FileTree, Kit, Tree, TreeItem, TreeModel};
use tokio::sync::mpsc;

/// Requests sent from the UI thread to the Tokio disk worker.
enum IoCmd {
    OpenProject(PathBuf),
}

/// Answers sent back from the Tokio disk worker to `poll`.
enum IoMsg {
    ProjectListed { project: Project, entries: Vec<FileEntry> },
    Error(String),
}

struct Pen {
    /// Folder tree pane. Width per the plan: 248 pt, resizable 160–480.
    sidebar: PaneState,
    project: Option<Project>,
    files: TreeModel<FileNode<PathBuf>>,
    editor: Option<EditorState>,
    status: Option<String>,
    io_tx: Option<mpsc::Sender<IoCmd>>,
    io_rx: Option<mpsc::Receiver<IoMsg>>,
}

fn init() -> Pen {
    Pen {
        sidebar: PaneState::new(248.0).range(160.0, 480.0),
        project: None,
        files: TreeModel::new().key("pen/tree"),
        editor: None,
        status: None,
        io_tx: None,
        io_rx: None,
    }
}

fn setup(s: &mut Pen, waker: Waker) {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<IoCmd>(32);
    let (msg_tx, msg_rx) = mpsc::channel::<IoMsg>(32);
    // Launch folder from argv, if given (`pen /path/to/dir`).
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
    let mut changed = false;
    while let Ok(msg) = rx.try_recv() {
        changed = true;
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
                s.status = None;
                tx.instant();
            }
            IoMsg::Error(err) => {
                s.status = Some(err);
                tx.instant();
            }
        }
    }
    changed
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
    let page = match &s.editor {
        Some(ed) => col()
            .key("pen/page")
            .w_full()
            .h_full()
            .cursor(Cursor::Text)
            .on_layout_tx(|s: &mut Pen, r: [f32; 4], tx: &mut Tx| {
                if let Some(ed) = &mut s.editor
                    && (ed.viewport_height - r[3]).abs() > 0.5
                {
                    ed.viewport_height = r[3].max(60.0);
                } else {
                    tx.live_only();
                }
            })
            .on_wheel(|s: &mut Pen, ev: WheelEvent, tx: &mut Tx| {
                if let Some(ed) = &mut s.editor {
                    ed.scroll_by(ev.dy);
                    tx.instant();
                    return true;
                }
                false
            })
            .on_drag(|s: &mut Pen, drag: &mut Drag, tx: &mut Tx| {
                let shift = tx.mods().shift;
                if let Some(ed) = &mut s.editor {
                    ed.on_drag(drag, shift);
                    tx.instant();
                }
            })
            .on_paste(|s: &mut Pen, clip: &str, tx: &mut Tx| {
                if let Some(ed) = &mut s.editor {
                    ed.insert_text(clip);
                    tx.instant();
                }
            })
            .child(editor_view(ed)),
        None => col().key("pen/page-empty").w_full().h_full(),
    };

    let tree = Tree::new(|s: &Pen| &s.files, |s: &mut Pen| &mut s.files, tree_label)
        .name("Files")
        .row(|n, r| TreeItem::file(n, r))
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
        if ev.key == Key::Command('c') {
            if let Some(ed) = &s.editor
                && let Some(sel) = ed.selected_text()
            {
                tx.copy(sel);
                return true;
            }
        } else if ev.key == Key::Command('x')
            && let Some(ed) = &mut s.editor
            && let Some(cut) = ed.cut_selection()
        {
            tx.copy(cut);
            tx.instant();
            return true;
        }
    }
    if let Some(ed) = &mut s.editor
        && ed.focused
        && ed.on_key(ev)
    {
        tx.instant();
        return true;
    }
    false
}

fn on_ime(s: &mut Pen, ime: &Ime, tx: &mut Tx) -> bool {
    if let Some(ed) = &mut s.editor
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
