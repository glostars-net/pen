//! Pen's library surface: bundled fonts, the editor page, theme mapping, and
//! disk rules shared between the `pen` binary and integration tests.

pub mod agent;
pub mod agent_write;
pub mod complete;
pub mod diagnostics;
pub mod disk;
pub mod dock;
pub mod editor;
pub mod file_mark;
pub mod git_guard;
pub mod git_panel;
pub mod goto;
pub mod keys;
pub mod lsp_host;
pub mod palette;
pub mod recents;
pub mod scaffold;
pub mod settings;
pub mod theme_kit;
pub mod tree_state;

/// Bundled OFL faces (IBM Plex Sans for chrome, IBM Plex Mono for code).
/// No third family, no script face.
pub const FONTS: &[&[u8]] = &[
    include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexMono-Medium.ttf"),
];

pub const FAMILIES: (&str, &str) = ("IBM Plex Sans", "IBM Plex Mono");
