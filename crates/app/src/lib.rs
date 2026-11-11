//! Pen's library surface: bundled fonts, the editor page, and the modules
//! integration tests share with the `pen` binary.

pub mod editor;

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
