//! OpenCode transcript. Messages do not animate. The caret does not wait on the agent.

use highlight::Theme;
use rune::{El, Transition, col, text};

use crate::theme_kit::{UI_FONT, UI_MEDIUM_FONT, rgb_color};

pub struct Transcript {
    pub lines: Vec<String>,
}

impl Transcript {
    pub fn push_user(&mut self, text: &str) {
        self.lines.push(text.to_owned());
    }

    pub fn push_agent(&mut self, text: &str) {
        self.lines.push(text.to_owned());
    }
}

/// Solid  column. The caller supplies the one-line input, or none when OpenCode is missing.
pub fn agent_column(lines: &[String], missing: bool, theme: &Theme, input: Option<El>) -> El {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let mut body = col()
        .key("pen/agent")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(bg)
        .padding(12.0)
        .gap(8.0)
        .transition(Transition::none())
        .child(text("OpenCode").font(UI_MEDIUM_FONT).color(fg));
    if missing {
        return body.child(
            text("OpenCode is not installed.")
                .font(UI_FONT)
                .color(faint),
        );
    }
    for (i, line) in lines.iter().enumerate() {
        body = body.child(
            text(line.clone())
                .key(format!("pen/agent-line-{i}"))
                .font(UI_FONT)
                .color(fg)
                .transition(Transition::none()),
        );
    }
    if let Some(input) = input {
        body = body.child(input);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_keeps_order() {
        let mut t = Transcript { lines: vec![] };
        t.push_user("hi");
        t.push_agent("ok");
        assert_eq!(t.lines, vec!["hi".to_string(), "ok".to_string()]);
    }
}
