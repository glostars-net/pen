//! VS Code-shaped chords. Labels only; the editor applies them.

use rune::{Key, KeyEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleComment,
    DeleteLine,
    MoveLineUp,
    MoveLineDown,
    Indent,
    Outdent,
    SelectLine,
    InsertLineBelow,
    InsertLineAbove,
    Palette,
    Sidebar,
}

pub fn chord(ev: &KeyEvent) -> Option<Chord> {
    let m = ev.mods;
    if m.alt && !m.cmd && !m.ctrl {
        return match ev.key {
            Key::Up => Some(Chord::MoveLineUp),
            Key::Down => Some(Chord::MoveLineDown),
            _ => None,
        };
    }
    if !m.cmd || m.alt || (cfg!(target_os = "macos") && m.ctrl) {
        return None;
    }
    if matches!(ev.key, Key::Enter) {
        return Some(if m.shift {
            Chord::InsertLineAbove
        } else {
            Chord::InsertLineBelow
        });
    }
    let Key::Command(c) = ev.key else {
        return None;
    };
    match c {
        '=' | '+' => Some(Chord::ZoomIn),
        '-' if !m.shift => Some(Chord::ZoomOut),
        '0' if !m.shift => Some(Chord::ZoomReset),
        '/' if !m.shift => Some(Chord::ToggleComment),
        'k' if m.shift => Some(Chord::DeleteLine),
        ']' if !m.shift => Some(Chord::Indent),
        '[' if !m.shift => Some(Chord::Outdent),
        'l' if !m.shift => Some(Chord::SelectLine),
        'p' if m.shift => Some(Chord::Palette),
        'b' if !m.shift => Some(Chord::Sidebar),
        _ => None,
    }
}

/// Line-comment marks. Hue stays in the ink ramp; this only picks the token.
pub struct CommentMark {
    pub start: &'static str,
    pub end: &'static str,
}

pub fn comment_mark(lang: Option<&str>) -> CommentMark {
    match lang {
        Some("python" | "py" | "toml" | "yaml" | "yml" | "sh" | "bash" | "zsh" | "ruby" | "rb") => {
            CommentMark {
                start: "# ",
                end: "",
            }
        }
        Some("lua") => CommentMark {
            start: "-- ",
            end: "",
        },
        Some("html" | "xml" | "svg") => CommentMark {
            start: "<!-- ",
            end: " -->",
        },
        Some("css" | "scss") => CommentMark {
            start: "/* ",
            end: " */",
        },
        _ => CommentMark {
            start: "// ",
            end: "",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::Mods;

    fn ev(key: Key, mods: Mods) -> KeyEvent {
        KeyEvent { key, mods }
    }

    fn cmd(c: char, shift: bool) -> KeyEvent {
        ev(
            Key::Command(c),
            Mods {
                cmd: true,
                shift,
                ..Mods::default()
            },
        )
    }

    #[test]
    fn vscode_chords() {
        assert_eq!(chord(&cmd('=', false)), Some(Chord::ZoomIn));
        assert_eq!(chord(&cmd('+', false)), Some(Chord::ZoomIn));
        assert_eq!(chord(&cmd('-', false)), Some(Chord::ZoomOut));
        assert_eq!(chord(&cmd('0', false)), Some(Chord::ZoomReset));
        assert_eq!(chord(&cmd('/', false)), Some(Chord::ToggleComment));
        assert_eq!(chord(&cmd('k', true)), Some(Chord::DeleteLine));
        assert_eq!(chord(&cmd(']', false)), Some(Chord::Indent));
        assert_eq!(chord(&cmd('[', false)), Some(Chord::Outdent));
        assert_eq!(chord(&cmd('l', false)), Some(Chord::SelectLine));
        assert_eq!(chord(&cmd('p', true)), Some(Chord::Palette));
        assert_eq!(chord(&cmd('b', false)), Some(Chord::Sidebar));
        assert_eq!(
            chord(&ev(
                Key::Up,
                Mods {
                    alt: true,
                    ..Mods::default()
                }
            )),
            Some(Chord::MoveLineUp)
        );
        assert_eq!(
            chord(&ev(
                Key::Down,
                Mods {
                    alt: true,
                    ..Mods::default()
                }
            )),
            Some(Chord::MoveLineDown)
        );
        assert_eq!(
            chord(&ev(
                Key::Enter,
                Mods {
                    cmd: true,
                    ..Mods::default()
                }
            )),
            Some(Chord::InsertLineBelow)
        );
        assert_eq!(
            chord(&ev(
                Key::Enter,
                Mods {
                    cmd: true,
                    shift: true,
                    ..Mods::default()
                }
            )),
            Some(Chord::InsertLineAbove)
        );
        assert_eq!(chord(&cmd('s', false)), None);
        assert_eq!(chord(&cmd('k', false)), None);
    }
}
