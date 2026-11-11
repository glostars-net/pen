//! Pen: a code editor that feels like a pen on paper, drawn by Rune.
//!
//! Phase 1 (plan/2026-11-10-pen.md): one window, a folder tree, a rope-backed
//! editor page, save, find, and a command palette. The UI thread never does
//! disk or watcher work.

use rune::{AppSpec, Cx, El, Spring, col};
use rune_kit::Kit;
use rune_kit::shell::{PaneMotion, PaneState, Sidebar, SplitView, Toolbar};

/// The window's state. Phase-1 tasks grow this struct; nothing here is
/// reserved for later phases.
struct Pen {
    /// Folder tree pane. Width per the plan: 248 pt, resizable 160–480.
    sidebar: PaneState,
}

fn init() -> Pen {
    Pen { sidebar: PaneState::new(248.0).range(160.0, 480.0) }
}

fn view(s: &Pen, cx: &mut Cx<Pen>) -> El {
    let k = Kit::neutral(cx.system_dark());
    // The page. Empty until Task 5 draws the editor surface.
    let page = col().key("page").w_full().h_full();
    let sidebar = Sidebar::new().label("Files").build(&k);
    SplitView::new(page)
        .sidebar(&s.sidebar, |s: &mut Pen| &mut s.sidebar, sidebar)
        .toolbar(Toolbar::new().title("Pen"))
        .titlebar_inset(cx.titlebar_inset())
        // Pane motion per the design: critically damped, 0.35 s, no bounce.
        .pane_motion(PaneMotion::new().spring(Spring::new(0.35, 0.0)))
        .reduced_motion(cx.is_reduced_motion())
        .build(&k)
}

fn main() {
    rune::prewarm_gpu();
    if let Err(e) = rune::set_app_icon(include_bytes!("../assets/icon.png").as_slice()) {
        eprintln!("pen: app icon not set: {e}");
    }
    rune::App::new(AppSpec::new("Pen", (1100.0, 720.0), init, view)).run();
}
