//! Pen: a code editor that feels like a pen on paper, drawn by Rune.
//!
//! Phase 1 (plan/2026-11-10-pen.md): one window, a folder tree, a rope-backed
//! editor page, save, find, and a command palette. The UI thread never does
//! disk or watcher work.

use pen::editor::{EditorState, editor_view};
use rune::{AppSpec, Cursor, Cx, Drag, El, Ime, Key, KeyEvent, Spring, Tx, WheelEvent, col};
use rune_kit::Kit;
use rune_kit::shell::{self, PaneMotion, PaneState, Sidebar, SplitView, Toolbar};

struct Pen {
    /// Folder tree pane. Width per the plan: 248 pt, resizable 160–480.
    sidebar: PaneState,
    editor: Option<EditorState>,
}

fn init() -> Pen {
    Pen {
        sidebar: PaneState::new(248.0).range(160.0, 480.0),
        editor: Some(EditorState::from_str("")),
    }
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
    let sidebar = Sidebar::new().label("Files").build(&k);
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
        on_key: Some(on_key),
        ..AppSpec::new("Pen", (1100.0, 720.0), init, view)
    };
    rune::App::new(spec).on_ime(on_ime).run();
}
