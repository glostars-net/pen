//! Headless frame test for the editor surface (Task 5 of the phase-1 plan):
//! renders `"hello\nworld"` through Rune's documented `--snap` + `--tree`
//! path at 800×600 and checks both the PNG dimensions and the scene tree.

use pen::editor::{EditorState, editor_view};
use rune::{App, AppSpec, Cx};

/// Read `(width, height)` from a PNG's 24-byte header (8-byte magic + IHDR).
fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
    assert!(bytes.len() >= 24 && &bytes[..8] == b"\x89PNG\r\n\x1a\n", "not a PNG");
    assert_eq!(&bytes[12..16], b"IHDR");
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    (w, h)
}

#[test]
fn editor_page_renders_800x600_png_and_tree_has_hello() {
    let dir = std::env::temp_dir().join(format!("pen-editor-frame-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("frame.png");
    let tree = dir.join("tree.json");

    let spec = AppSpec {
        fonts: pen::FONTS,
        families: pen::FAMILIES,
        ..AppSpec::new(
            "Pen",
            (800.0, 600.0),
            || EditorState::from_str("hello\nworld"),
            |s: &EditorState, _: &mut Cx<EditorState>| editor_view(s),
        )
    };
    App::new(spec)
        .args(
            [
                "--snap",
                png.to_str().unwrap(),
                "--tree",
                tree.to_str().unwrap(),
                "--size",
                "800x600",
                "--scale",
                "1",
            ]
            .map(String::from),
        )
        .run();

    let png_bytes = std::fs::read(&png).unwrap();
    assert_eq!(png_dimensions(&png_bytes), (800, 600));

    let tree_json = std::fs::read_to_string(&tree).unwrap();
    assert!(tree_json.contains("hello"), "expected `hello` in scene tree:\n{tree_json}");
    assert!(tree_json.contains("world"), "expected `world` in scene tree:\n{tree_json}");

    std::fs::remove_dir_all(&dir).ok();
}
