//! Headless frame test for the editor surface (Tasks 5 and 9 of the phase-1
//! plan): renders `"hello\nworld"` in both Paper and Ink at 800×600 and checks
//! the PNGs and scene trees.

use pen::editor::{EditorState, editor_view};
use rune::{App, AppSpec, Cx};

type SceneFn = fn(&mut EditorState);

const SCENES: &[(&str, SceneFn)] = &[
    ("paper", |s| s.theme = highlight::paper()),
    ("ink", |s| s.theme = highlight::ink()),
];

fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
    assert!(
        bytes.len() >= 24 && &bytes[..8] == b"\x89PNG\r\n\x1a\n",
        "not a PNG"
    );
    assert_eq!(&bytes[12..16], b"IHDR");
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    (w, h)
}

fn render_with_theme(dir: &std::path::Path, slug: &str) {
    let png = dir.join(format!("{slug}.png"));
    let tree = dir.join(format!("{slug}-tree.json"));
    let spec = AppSpec {
        fonts: pen::FONTS,
        families: pen::FAMILIES,
        scenes: SCENES,
        ..AppSpec::new(
            "Pen",
            (800.0, 600.0),
            || EditorState::from_str("fn hello() {\n    // world\n}").with_language(Some("rust")),
            |s: &EditorState, _: &mut Cx<EditorState>| editor_view(s),
        )
    };
    let png_arg = png.to_string_lossy().into_owned();
    let tree_arg = tree.to_string_lossy().into_owned();
    App::new(spec)
        .args(
            [
                "--scene", slug, "--snap", &png_arg, "--tree", &tree_arg, "--size", "800x600",
                "--scale", "1",
            ]
            .map(String::from),
        )
        .run();

    let png_bytes = std::fs::read(&png).unwrap_or_default();
    assert_eq!(png_dimensions(&png_bytes), (800, 600));

    let tree_json = std::fs::read_to_string(&tree).unwrap_or_default();
    assert!(
        tree_json.contains("hello"),
        "expected `hello` in {slug} tree:\n{tree_json}"
    );
    assert!(
        tree_json.contains("world"),
        "expected `world` in {slug} tree:\n{tree_json}"
    );
}

#[test]
fn editor_page_renders_paper_and_ink_frames() {
    let dir = std::env::temp_dir().join(format!("pen-editor-frame-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    render_with_theme(&dir, "paper");
    render_with_theme(&dir, "ink");
    assert!(dir.join("paper.png").is_file());
    assert!(dir.join("ink.png").is_file());

    std::fs::remove_dir_all(&dir).ok();
}
