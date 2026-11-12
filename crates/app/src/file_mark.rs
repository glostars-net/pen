//! Ink file marks. The drawings live in `assets/icons/files`. The tree uses the
//! SVG body so Rune can tint it with the theme foreground.

use std::path::Path;

use rune::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileMark {
    Folder,
    Rust,
    Markdown,
    Json,
    Toml,
    Javascript,
    Typescript,
    Python,
    Html,
    Css,
    Go,
    Shell,
    Image,
    Text,
    Code,
    Git,
}

pub fn mark_for(path: &Path, is_dir: bool) -> FileMark {
    if is_dir {
        return FileMark::Folder;
    }
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if name == ".gitignore" || name == ".gitattributes" {
        return FileMark::Git;
    }
    match path.extension().and_then(|s| s.to_str()) {
        Some("rs") => FileMark::Rust,
        Some("md") => FileMark::Markdown,
        Some("json") => FileMark::Json,
        Some("toml") => FileMark::Toml,
        Some("js" | "mjs" | "cjs") => FileMark::Javascript,
        Some("ts" | "tsx") => FileMark::Typescript,
        Some("py") => FileMark::Python,
        Some("html" | "htm") => FileMark::Html,
        Some("css") => FileMark::Css,
        Some("go") => FileMark::Go,
        Some("sh" | "bash" | "zsh") => FileMark::Shell,
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg") => FileMark::Image,
        Some("txt") => FileMark::Text,
        Some(ext) => {
            let fake = format!("f.{ext}");
            if highlight::lang_for_path(&fake).is_some() {
                FileMark::Code
            } else {
                FileMark::Text
            }
        }
        None => FileMark::Text,
    }
}

pub fn mark_asset(mark: FileMark) -> &'static str {
    match mark {
        FileMark::Folder => "folder.png",
        FileMark::Rust => "rust.png",
        FileMark::Markdown => "markdown.png",
        FileMark::Json => "json.png",
        FileMark::Toml => "toml.png",
        FileMark::Javascript => "javascript.png",
        FileMark::Typescript => "typescript.png",
        FileMark::Python => "python.png",
        FileMark::Html => "html.png",
        FileMark::Css => "css.png",
        FileMark::Go => "go.png",
        FileMark::Shell => "shell.png",
        FileMark::Image => "image.png",
        FileMark::Text => "text.png",
        FileMark::Code => "code.png",
        FileMark::Git => "git.png",
    }
}

pub fn mark_icon(mark: FileMark) -> &'static Icon {
    match mark {
        FileMark::Folder => &FOLDER,
        FileMark::Rust => &RUST,
        FileMark::Markdown => &MARKDOWN,
        FileMark::Json => &JSON,
        FileMark::Toml => &TOML,
        FileMark::Javascript => &JAVASCRIPT,
        FileMark::Typescript => &TYPESCRIPT,
        FileMark::Python => &PYTHON,
        FileMark::Html => &HTML,
        FileMark::Css => &CSS,
        FileMark::Go => &GO,
        FileMark::Shell => &SHELL,
        FileMark::Image => &IMAGE,
        FileMark::Text => &TEXT,
        FileMark::Code => &CODE,
        FileMark::Git => &GIT,
    }
}

const fn icon(name: &'static str, body: &'static str) -> Icon {
    Icon {
        name,
        view: 32.0,
        body,
    }
}

static FOLDER: Icon = icon(
    "pen-folder",
    include_str!("../../../assets/icons/files/folder.body.svg"),
);
static RUST: Icon = icon(
    "pen-rust",
    include_str!("../../../assets/icons/files/rust.body.svg"),
);
static MARKDOWN: Icon = icon(
    "pen-md",
    include_str!("../../../assets/icons/files/markdown.body.svg"),
);
static JSON: Icon = icon(
    "pen-json",
    include_str!("../../../assets/icons/files/json.body.svg"),
);
static TOML: Icon = icon(
    "pen-toml",
    include_str!("../../../assets/icons/files/toml.body.svg"),
);
static JAVASCRIPT: Icon = icon(
    "pen-js",
    include_str!("../../../assets/icons/files/javascript.body.svg"),
);
static TYPESCRIPT: Icon = icon(
    "pen-ts",
    include_str!("../../../assets/icons/files/typescript.body.svg"),
);
static PYTHON: Icon = icon(
    "pen-py",
    include_str!("../../../assets/icons/files/python.body.svg"),
);
static HTML: Icon = icon(
    "pen-html",
    include_str!("../../../assets/icons/files/html.body.svg"),
);
static CSS: Icon = icon(
    "pen-css",
    include_str!("../../../assets/icons/files/css.body.svg"),
);
static GO: Icon = icon(
    "pen-go",
    include_str!("../../../assets/icons/files/go.body.svg"),
);
static SHELL: Icon = icon(
    "pen-sh",
    include_str!("../../../assets/icons/files/shell.body.svg"),
);
static IMAGE: Icon = icon(
    "pen-img",
    include_str!("../../../assets/icons/files/image.body.svg"),
);
static TEXT: Icon = icon(
    "pen-txt",
    include_str!("../../../assets/icons/files/text.body.svg"),
);
static CODE: Icon = icon(
    "pen-code",
    include_str!("../../../assets/icons/files/code.body.svg"),
);
static GIT: Icon = icon(
    "pen-git",
    include_str!("../../../assets/icons/files/git.body.svg"),
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_and_markdown_do_not_share_a_mark() {
        assert_eq!(mark_for(Path::new("a.rs"), false), FileMark::Rust);
        assert_eq!(mark_for(Path::new("a.md"), false), FileMark::Markdown);
        assert_eq!(mark_asset(FileMark::Rust), "rust.png");
        assert_ne!(mark_asset(FileMark::Rust), mark_asset(FileMark::Markdown));
    }
}
