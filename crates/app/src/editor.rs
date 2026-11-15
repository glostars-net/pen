//! The writing page: a viewport over a `Buffer`, drawn in Rune with no
//! `TextArea` wrapper.
//!
//! `editor_view` reads only the visible window (`Buffer::viewport` plus 8 lines
//! of overscan), so opening or typing into a 10 MB file touches ~50 lines per
//! frame. It attaches no state-typed handlers of its own, which lets headless
//! tests mount `EditorState` directly while `main.rs` mounts it inside `Pen`.

use buffer::{Buffer, Preedit, apply_ime};
use highlight::{Theme, highlight_lines};
use rune::{
    Align, Drag, El, Family, Ime, Key, KeyEvent, OpticalSize, Phase, Span, TextStyle, Transition,
    col, el, row, text,
};

use crate::diagnostics::{self, Diag};
use crate::theme_kit::rgb_color;

/// Code size in points (Design: 13px, line-height 1.55, tracking 0).
pub const CODE_SIZE: f32 = 13.0;
/// Exact row pitch at the default size: `13.0 * 1.55 = 20.15` pt.
pub const LINE_H: f32 = 20.15;
/// IBM Plex Mono Regular has `unitsPerEm = 1000` and glyph advance `600`:
/// `13.0 * 600 / 1000 = 7.80` pt exact at the default size.
pub const CHAR_ADVANCE: f32 = 7.80;
/// Writing-page zoom stops. Chrome stays at [`CODE_SIZE`].
pub const ZOOM_MIN: f32 = 9.0;
pub const ZOOM_MAX: f32 = 28.0;

pub fn line_h_for(size: f32) -> f32 {
    size * (LINE_H / CODE_SIZE)
}

pub fn advance_for(size: f32) -> f32 {
    size * (CHAR_ADVANCE / CODE_SIZE)
}
/// Signature notebook left margin before the line numbers (Design: 48 pt).
pub const NOTEBOOK_MARGIN: f32 = 48.0;
/// Breathing room between the right-aligned line number and the code column.
pub const GUTTER_GAP: f32 = 20.0;
/// Top breathing room above line 1 before scrolling.
pub const PAGE_TOP_PAD: f32 = 16.0;
/// Overscan lines beyond the visible window (Task 5: 8 lines).
pub const OVERSCAN_LINES: usize = 8;
/// Caret stroke width (Design: 1.5 px ink stroke).
pub const CARET_WIDTH: f32 = 1.5;
/// Tab stop width in columns.
const TAB_STOP: usize = 4;

fn code_font(size: f32) -> TextStyle {
    TextStyle {
        size,
        weight: 400.0,
        family: Family::Mono,
        line_height: line_h_for(size),
        tracking: 0.0,
        variations: "",
        features: "",
        optical_size: OpticalSize::Auto,
        italic: false,
    }
}

fn line_no_font(size: f32) -> TextStyle {
    TextStyle {
        size,
        weight: 400.0,
        family: Family::Mono,
        line_height: line_h_for(size),
        tracking: 0.0,
        variations: "",
        features: "'tnum' 1",
        optical_size: OpticalSize::Auto,
        italic: false,
    }
}

/// An open editor page: one `Buffer`, one caret, an optional selection anchor,
/// and an optional IME composition.
pub struct EditorState {
    pub buffer: Buffer,
    /// Char index in `buffer` (excluding any active preedit; while composing,
    /// the visual caret sits at the end of `preedit`).
    pub caret: usize,
    /// Selection anchor char index (`anchor == caret` means no selection).
    pub anchor: usize,
    pub preedit: Option<Preedit>,
    /// Vertical scroll offset in points (`>= 0.0`).
    pub scroll_y: f64,
    /// Measured height of the page viewport in points (default 600 for headless).
    pub viewport_height: f32,
    /// Whether this page owns the keyboard (controls `.ime_cursor()`).
    pub focused: bool,
    pub theme: Theme,
    /// Language alias from `highlight::lang_for_path` (e.g. `"rust"`).
    pub language: Option<String>,
    /// Writing-page size in points. Chrome does not follow this.
    pub code_size: f32,
    /// Percent applied on top of [`Self::code_size`]. 100 is unchanged.
    pub ui_scale: i32,
}

impl EditorState {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(src: &str) -> Self {
        Self::from_buffer(Buffer::from_str(src))
    }

    pub fn from_buffer(buffer: Buffer) -> Self {
        EditorState {
            buffer,
            caret: 0,
            anchor: 0,
            preedit: None,
            scroll_y: 0.0,
            viewport_height: 600.0,
            focused: true,
            theme: highlight::paper(),
            language: None,
            code_size: CODE_SIZE,
            ui_scale: 100,
        }
    }

    fn render_size(&self) -> f32 {
        self.code_size * (self.ui_scale.clamp(90, 140) as f32) / 100.0
    }

    pub fn line_h(&self) -> f32 {
        line_h_for(self.render_size())
    }

    pub fn char_advance(&self) -> f32 {
        advance_for(self.render_size())
    }

    /// Clamp to 9–28 pt and keep the same line at the top of the page.
    pub fn set_code_size(&mut self, size: f32) {
        let next = size.round().clamp(ZOOM_MIN, ZOOM_MAX);
        let old = self.line_h();
        self.code_size = next;
        let new = self.line_h();
        if old > 0.0 {
            self.scroll_y *= f64::from(new / old);
        }
    }

    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    pub fn with_language(mut self, lang: Option<&str>) -> Self {
        self.language = lang.map(str::to_owned);
        self
    }

    /// Ordered selection char range when `anchor != caret`.
    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        (self.anchor != self.caret)
            .then(|| self.anchor.min(self.caret)..self.anchor.max(self.caret))
    }

    /// Char index where the caret draws (at the end of an active preedit).
    pub fn visual_caret(&self) -> usize {
        match &self.preedit {
            Some(p) => p.start + p.text.chars().count(),
            None => self.caret.min(self.buffer.len_chars()),
        }
    }

    /// Width of the line-number column in points, growing past 4 digits so
    /// 100k-line files never clip their numbers.
    pub fn line_number_width(&self) -> f32 {
        let digits = self.buffer.len_lines().max(1).ilog10() as usize + 1;
        let cols = digits.max(3) as f32;
        (cols * self.char_advance()).ceil()
    }

    /// X offset from the page's left edge to column 0 of code.
    pub fn code_left(&self) -> f32 {
        NOTEBOOK_MARGIN + self.line_number_width() + GUTTER_GAP
    }

    /// Scroll vertically by `dy` points (wheel delta: negative scrolls down in
    /// Rune's `WheelEvent`, matching AppKit/winit).
    pub fn scroll_by(&mut self, dy: f32) {
        let max_y = ((self.buffer.len_lines().saturating_sub(1)) as f64) * f64::from(self.line_h());
        self.scroll_y = (self.scroll_y - f64::from(dy)).clamp(0.0, max_y);
    }

    /// Keep the caret's line inside the visible window after an edit or move.
    pub fn reveal_caret(&mut self) {
        let line = self.buffer.char_to_line(self.visual_caret());
        let top = (line as f64) * f64::from(self.line_h());
        let view =
            f64::from((self.viewport_height - PAGE_TOP_PAD - self.line_h()).max(self.line_h()));
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if top > self.scroll_y + view {
            self.scroll_y = top - view;
        }
    }

    /// Map a pointer drag on the page to caret / selection updates.
    pub fn on_drag(&mut self, drag: &Drag, extend_shift: bool) {
        if self.preedit.is_some() {
            return;
        }
        let idx = self.char_at_point(drag.x, drag.y);
        match drag.phase {
            Phase::Down => {
                self.focused = true;
                if drag.clicks >= 3 {
                    let line = self.buffer.char_to_line(idx);
                    let start = self.buffer.line_to_char(line);
                    let end =
                        (start + self.buffer.line_len_chars(line) + 1).min(self.buffer.len_chars());
                    self.anchor = start;
                    self.caret = end;
                } else if drag.clicks == 2 {
                    let (start, end) = self.word_bounds_at(idx);
                    self.anchor = start;
                    self.caret = end;
                } else if extend_shift {
                    self.caret = idx;
                } else {
                    self.caret = idx;
                    self.anchor = idx;
                }
            }
            Phase::Move => {
                self.caret = idx;
            }
            Phase::Up => {}
        }
    }

    /// Apply an IME event from `App::on_ime` through `buffer::apply_ime`.
    pub fn on_ime(&mut self, ime: &Ime) -> bool {
        match ime {
            Ime::Preedit(text, _) => {
                if self.preedit.is_none() {
                    self.delete_selection();
                    self.preedit = Some(Preedit {
                        start: self.caret,
                        text: String::new(),
                    });
                }
                let _ = apply_ime(&mut self.buffer, &mut self.preedit, text, false);
                self.reveal_caret();
                true
            }
            Ime::Commit(text) => {
                if self.preedit.is_none() {
                    self.delete_selection();
                    self.preedit = Some(Preedit {
                        start: self.caret,
                        text: String::new(),
                    });
                }
                let start = self.preedit.as_ref().map_or(self.caret, |p| p.start);
                if apply_ime(&mut self.buffer, &mut self.preedit, text, true).is_ok() {
                    self.caret = start + text.chars().count();
                    self.anchor = self.caret;
                }
                self.reveal_caret();
                true
            }
            Ime::Enabled | Ime::Disabled => false,
        }
    }

    /// Handle a keystroke when the editor owns focus. Returns true if consumed.
    pub fn on_key(&mut self, ev: &KeyEvent) -> bool {
        if self.preedit.is_some() {
            return false;
        }
        let shift = ev.mods.shift;
        if ev.mods.cmd {
            match &ev.key {
                Key::Command('z') if shift => return self.redo(),
                Key::Command('z') => return self.undo(),
                Key::Command('y') if !cfg!(target_os = "macos") => return self.redo(),
                Key::Command('a') => {
                    self.anchor = 0;
                    self.caret = self.buffer.len_chars();
                    return true;
                }
                Key::Left => {
                    let line = self.buffer.char_to_line(self.caret);
                    self.move_to(self.buffer.line_to_char(line), shift);
                    return true;
                }
                Key::Right => {
                    let line = self.buffer.char_to_line(self.caret);
                    let end = self.buffer.line_to_char(line) + self.buffer.line_len_chars(line);
                    self.move_to(end, shift);
                    return true;
                }
                Key::Up => {
                    self.move_to(0, shift);
                    return true;
                }
                Key::Down => {
                    self.move_to(self.buffer.len_chars(), shift);
                    return true;
                }
                _ => return false,
            }
        }
        match &ev.key {
            Key::Text(t) if !ev.mods.ctrl => {
                self.insert_text(t);
                true
            }
            Key::Enter => {
                self.insert_text("\n");
                true
            }
            Key::Tab => {
                self.insert_text("    ");
                true
            }
            Key::Backspace => {
                if !self.delete_selection() && self.caret > 0 {
                    let prev = self.caret - 1;
                    self.buffer.delete(prev..self.caret);
                    self.caret = prev;
                    self.anchor = prev;
                }
                self.reveal_caret();
                true
            }
            Key::Delete => {
                if !self.delete_selection() && self.caret < self.buffer.len_chars() {
                    self.buffer.delete(self.caret..self.caret + 1);
                }
                self.reveal_caret();
                true
            }
            Key::Left => {
                let to = if !shift && let Some(sel) = self.selection() {
                    sel.start
                } else {
                    self.caret.saturating_sub(1)
                };
                self.move_to(to, shift);
                true
            }
            Key::Right => {
                let to = if !shift && let Some(sel) = self.selection() {
                    sel.end
                } else {
                    (self.caret + 1).min(self.buffer.len_chars())
                };
                self.move_to(to, shift);
                true
            }
            Key::Up => {
                let line = self.buffer.char_to_line(self.caret);
                if line == 0 {
                    self.move_to(0, shift);
                } else {
                    let col = self.caret - self.buffer.line_to_char(line);
                    let prev_start = self.buffer.line_to_char(line - 1);
                    let prev_len = self.buffer.line_len_chars(line - 1);
                    self.move_to(prev_start + col.min(prev_len), shift);
                }
                true
            }
            Key::Down => {
                let line = self.buffer.char_to_line(self.caret);
                if line + 1 >= self.buffer.len_lines() {
                    self.move_to(self.buffer.len_chars(), shift);
                } else {
                    let col = self.caret - self.buffer.line_to_char(line);
                    let next_start = self.buffer.line_to_char(line + 1);
                    let next_len = self.buffer.line_len_chars(line + 1);
                    self.move_to(next_start + col.min(next_len), shift);
                }
                true
            }
            Key::Home => {
                let line = self.buffer.char_to_line(self.caret);
                self.move_to(self.buffer.line_to_char(line), shift);
                true
            }
            Key::End => {
                let line = self.buffer.char_to_line(self.caret);
                self.move_to(
                    self.buffer.line_to_char(line) + self.buffer.line_len_chars(line),
                    shift,
                );
                true
            }
            Key::PageUp => {
                let step = ((self.viewport_height / self.line_h()).floor() as usize).max(1);
                let line = self.buffer.char_to_line(self.caret).saturating_sub(step);
                self.move_to(self.buffer.line_to_char(line), shift);
                true
            }
            Key::PageDown => {
                let step = ((self.viewport_height / self.line_h()).floor() as usize).max(1);
                let line = (self.buffer.char_to_line(self.caret) + step)
                    .min(self.buffer.len_lines().saturating_sub(1));
                self.move_to(self.buffer.line_to_char(line), shift);
                true
            }
            _ => false,
        }
    }

    pub fn insert_text(&mut self, s: &str) {
        self.delete_selection();
        let at = self.caret.min(self.buffer.len_chars());
        self.buffer.insert(at, s);
        self.caret = at + s.chars().count();
        self.anchor = self.caret;
        self.reveal_caret();
    }

    pub fn undo(&mut self) -> bool {
        let changed = self.buffer.undo();
        if changed {
            self.caret = self.caret.min(self.buffer.len_chars());
            self.anchor = self.caret;
            self.reveal_caret();
        }
        changed
    }

    pub fn redo(&mut self) -> bool {
        let changed = self.buffer.redo();
        if changed {
            self.caret = self.caret.min(self.buffer.len_chars());
            self.anchor = self.caret;
            self.reveal_caret();
        }
        changed
    }

    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection()?;
        // Slice via line range rather than materializing the whole file.
        let l0 = self.buffer.char_to_line(sel.start);
        let l1 = self.buffer.char_to_line(sel.end);
        let base = self.buffer.line_to_char(l0);
        let mut chunk = String::new();
        for slice in self.buffer.viewport(l0, l1 - l0 + 1) {
            if slice.line_idx > l0 {
                chunk.push('\n');
            }
            chunk.push_str(&slice.text);
        }
        let start = sel.start.saturating_sub(base);
        let len = sel.end.saturating_sub(sel.start);
        Some(chunk.chars().skip(start).take(len).collect())
    }

    pub fn cut_selection(&mut self) -> Option<String> {
        let out = self.selected_text()?;
        self.delete_selection();
        self.reveal_caret();
        Some(out)
    }

    /// Select the next occurrence of `query` from the caret (or from the start
    /// of the current selection when typing into Find). Returns true on match.
    pub fn find_and_select(&mut self, query: &str, step_past_current: bool) -> bool {
        if query.is_empty() {
            self.anchor = self.caret;
            return false;
        }
        let start = if step_past_current {
            self.selection()
                .map_or(self.caret, |r| (r.start + 1).min(self.buffer.len_chars()))
        } else {
            self.selection().map_or(self.caret, |r| r.start)
        };
        let Some(hit) = self.buffer.find_next(start, query) else {
            return false;
        };
        self.anchor = hit.start;
        self.caret = hit.end;
        self.reveal_caret();
        true
    }

    fn delete_selection(&mut self) -> bool {
        let Some(sel) = self.selection() else {
            return false;
        };
        let start = sel.start;
        self.buffer.delete(sel);
        self.caret = start;
        self.anchor = start;
        true
    }

    fn move_to(&mut self, idx: usize, extend: bool) {
        let clamped = idx.min(self.buffer.len_chars());
        self.caret = clamped;
        if !extend {
            self.anchor = clamped;
        }
        self.reveal_caret();
    }

    fn char_at_point(&self, x: f32, y: f32) -> usize {
        let line_h = self.line_h();
        let first_line = (self.scroll_y / f64::from(line_h)).floor() as usize;
        let sub_y = -((self.scroll_y % f64::from(line_h)) as f32);
        let rel_y = (y - PAGE_TOP_PAD - sub_y).max(0.0);
        let line = (first_line + (rel_y / line_h).floor() as usize)
            .min(self.buffer.len_lines().saturating_sub(1));
        let line_str = self.buffer.line(line);
        let vis_col = ((x - self.code_left()) / self.char_advance())
            .round()
            .max(0.0) as usize;
        let char_col = char_col_at_visual(&line_str, vis_col);
        self.buffer.line_to_char(line) + char_col
    }

    fn word_bounds_at(&self, idx: usize) -> (usize, usize) {
        let line = self.buffer.char_to_line(idx);
        let line_start = self.buffer.line_to_char(line);
        let chars: Vec<char> = self.buffer.line(line).chars().collect();
        if chars.is_empty() {
            return (line_start, line_start);
        }
        let pos = (idx - line_start).min(chars.len() - 1);
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let kind = is_word(chars[pos]);
        let mut s = pos;
        while s > 0 && is_word(chars[s - 1]) == kind {
            s -= 1;
        }
        let mut e = pos + 1;
        while e < chars.len() && is_word(chars[e]) == kind {
            e += 1;
        }
        (line_start + s, line_start + e)
    }

    /// Apply a writing-page chord. Returns true when the buffer changed.
    pub fn apply_chord(&mut self, chord: crate::keys::Chord) -> bool {
        use crate::keys::Chord;
        match chord {
            Chord::ZoomIn => {
                self.set_code_size(self.code_size + 1.0);
                false
            }
            Chord::ZoomOut => {
                self.set_code_size(self.code_size - 1.0);
                false
            }
            Chord::ZoomReset => {
                self.set_code_size(CODE_SIZE);
                false
            }
            Chord::ToggleComment => {
                self.toggle_comment();
                true
            }
            Chord::DeleteLine => {
                self.delete_lines();
                true
            }
            Chord::MoveLineUp => self.move_lines(false),
            Chord::MoveLineDown => self.move_lines(true),
            Chord::Indent => {
                self.shift_indent(false);
                true
            }
            Chord::Outdent => {
                self.shift_indent(true);
                true
            }
            Chord::SelectLine => {
                self.select_lines();
                false
            }
            Chord::InsertLineBelow => {
                self.insert_blank(false);
                true
            }
            Chord::InsertLineAbove => {
                self.insert_blank(true);
                true
            }
            Chord::Palette | Chord::Sidebar => false,
        }
    }

    fn affected_lines(&self) -> (usize, usize) {
        let start_idx = self.anchor.min(self.caret);
        let end_idx = self.anchor.max(self.caret);
        let start = self.buffer.char_to_line(start_idx);
        let mut end = self.buffer.char_to_line(end_idx);
        if end_idx > start_idx && end_idx == self.buffer.line_to_char(end) && end > start {
            end -= 1;
        }
        (start, end)
    }

    fn set_line(&mut self, line: usize, text: &str) {
        let start = self.buffer.line_to_char(line);
        let end = start + self.buffer.line_len_chars(line);
        if start < end {
            self.buffer.delete(start..end);
        }
        if !text.is_empty() {
            self.buffer.insert(start, text);
        }
    }

    fn span_text(&self, start_line: usize, end_line: usize) -> String {
        let mut text = (start_line..=end_line)
            .map(|i| self.buffer.line(i))
            .collect::<Vec<_>>()
            .join("\n");
        if end_line + 1 < self.buffer.len_lines() {
            text.push('\n');
        }
        text
    }

    fn line_end(&self, line: usize) -> usize {
        if line + 1 < self.buffer.len_lines() {
            self.buffer.line_to_char(line + 1)
        } else {
            self.buffer.len_chars()
        }
    }

    fn toggle_comment(&mut self) {
        let mark = crate::keys::comment_mark(self.language.as_deref());
        let (a, b) = self.affected_lines();
        let lines: Vec<String> = (a..=b).map(|i| self.buffer.line(i)).collect();
        let uncomment = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .all(|l| is_commented(l, mark.start, mark.end));
        for (offset, line) in lines.iter().enumerate().rev() {
            if line.trim().is_empty() {
                continue;
            }
            let next = if uncomment {
                uncomment_line(line, mark.start, mark.end)
            } else {
                comment_line(line, mark.start, mark.end)
            };
            self.set_line(a + offset, &next);
        }
    }

    fn delete_lines(&mut self) {
        let (a, b) = self.affected_lines();
        let last = self.buffer.len_lines().saturating_sub(1);
        let (start, end) = if b >= last && a > 0 {
            (
                self.buffer.line_to_char(a).saturating_sub(1),
                self.buffer.len_chars(),
            )
        } else {
            (self.buffer.line_to_char(a), self.line_end(b))
        };
        if start < end {
            self.buffer.delete(start..end);
        }
        let caret = start.min(self.buffer.len_chars());
        self.caret = caret;
        self.anchor = caret;
    }

    fn move_lines(&mut self, down: bool) -> bool {
        let (a, b) = self.affected_lines();
        let last = self.buffer.len_lines().saturating_sub(1);
        if down && b >= last || !down && a == 0 {
            return false;
        }
        let (start, end, text, new_line) = if down {
            let mut block = self.span_text(a, b);
            let mut neighbor = self.span_text(b + 1, b + 1);
            if b + 1 == last && block.ends_with('\n') {
                block.pop();
                if !neighbor.ends_with('\n') {
                    neighbor.push('\n');
                }
            }
            (
                self.buffer.line_to_char(a),
                self.line_end(b + 1),
                format!("{neighbor}{block}"),
                a + 1,
            )
        } else {
            let neighbor = self.span_text(a - 1, a - 1);
            let mut block = self.span_text(a, b);
            if b == last && neighbor.ends_with('\n') && !block.ends_with('\n') {
                block.push('\n');
                let neighbor = neighbor.trim_end_matches('\n');
                (
                    self.buffer.line_to_char(a - 1),
                    self.line_end(b),
                    format!("{block}{neighbor}"),
                    a - 1,
                )
            } else {
                (
                    self.buffer.line_to_char(a - 1),
                    self.line_end(b),
                    format!("{block}{neighbor}"),
                    a - 1,
                )
            }
        };
        let col = self.caret
            - self
                .buffer
                .line_to_char(self.buffer.char_to_line(self.caret));
        self.buffer.delete(start..end);
        self.buffer.insert(start, &text);
        let line_start = self
            .buffer
            .line_to_char(new_line.min(self.buffer.len_lines().saturating_sub(1)));
        let len = self
            .buffer
            .line_len_chars(self.buffer.char_to_line(line_start));
        self.caret = line_start + col.min(len);
        self.anchor = self.caret;
        true
    }

    fn shift_indent(&mut self, out: bool) {
        let (a, b) = self.affected_lines();
        for line in (a..=b).rev() {
            let text = self.buffer.line(line);
            let next = if out {
                outdent_text(&text)
            } else {
                format!("    {text}")
            };
            if next != text {
                self.set_line(line, &next);
            }
        }
    }

    fn select_lines(&mut self) {
        let (a, b) = self.affected_lines();
        self.anchor = self.buffer.line_to_char(a);
        self.caret = self.line_end(b);
    }

    fn insert_blank(&mut self, above: bool) {
        let line = self.buffer.char_to_line(self.caret);
        let at = if above {
            self.buffer.line_to_char(line)
        } else {
            self.buffer.line_to_char(line) + self.buffer.line_len_chars(line)
        };
        self.buffer.insert(at, "\n");
        self.caret = if above { at } else { at + 1 };
        self.anchor = self.caret;
        self.reveal_caret();
    }
}

/// Draw the editor page from `state`. Pure function of `&EditorState`: reads
/// and highlights only `viewport(first_line, visible + OVERSCAN_LINES)`.
pub fn editor_view(state: &EditorState) -> El {
    editor_view_themed(state, &state.theme, false, &[])
}

pub fn editor_view_themed(
    state: &EditorState,
    theme: &Theme,
    reduced_motion: bool,
    diags: &[Diag],
) -> El {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let sel_color = fg.alpha(0.12);
    let line_h = state.line_h();
    let advance = state.char_advance();
    let code_font = code_font(state.render_size());
    let line_no_font = line_no_font(state.render_size());

    let first_line = (state.scroll_y / f64::from(line_h)).floor() as usize;
    let sub_y = -((state.scroll_y % f64::from(line_h)) as f32);
    let visible_lines = ((state.viewport_height / line_h).ceil() as usize).max(1);
    let window = state
        .buffer
        .viewport(first_line, visible_lines + OVERSCAN_LINES);

    let expanded_lines: Vec<String> = window.iter().map(|s| expand_tabs(&s.text)).collect();
    let syntax_per_line = highlight_lines(state.language.as_deref(), &expanded_lines, theme);

    let caret_pos = state.visual_caret();
    let caret_line = state.buffer.char_to_line(caret_pos);
    let selection = state.selection();
    let num_w = state.line_number_width();

    let override_hex = theme
        .diagnostic
        .map(|c| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2));
    let ink = diagnostics::underline_color(theme, override_hex.as_deref());
    let mut rows = col()
        .key("pen/rows")
        .w_full()
        .pt(PAGE_TOP_PAD)
        .offset(0.0, sub_y)
        .instant_changes()
        .transition(Transition::none());
    for (row_pos, slice) in window.iter().enumerate() {
        let i = slice.line_idx;
        let line_start = state.buffer.line_to_char(i);
        let line_chars = slice.text.chars().count();
        let line_end = line_start + line_chars;
        let expanded = &expanded_lines[row_pos];

        let mut code_cell = el().grow(1.0).h(line_h);

        // Selection band behind text (foreground at 12%, radius 0).
        if let Some(sel) = &selection
            && sel.start <= line_end
            && sel.end >= line_start
        {
            let c0 = sel.start.saturating_sub(line_start).min(line_chars);
            let c1 = sel.end.saturating_sub(line_start).min(line_chars);
            let x0 = visual_col(&slice.text, c0) as f32 * advance;
            let extra = if sel.end > line_end { advance } else { 0.0 };
            let x1 = visual_col(&slice.text, c1) as f32 * advance + extra;
            if x1 > x0 {
                code_cell = code_cell.child(
                    el().key(format!("sel-{i}"))
                        .absolute(x0, 0.0)
                        .size(x1 - x0, line_h)
                        .fill(sel_color)
                        .transition(Transition::none()),
                );
            }
        }

        // Code line: five-step OKLab syntax ramp over the viewport only (`text` keeps `wrap = false`).
        let line_spans = &syntax_per_line[row_pos];
        let mut text_el = text(expanded.clone())
            .font(code_font)
            .color(fg)
            .transition(Transition::none());
        if !line_spans.is_empty() {
            let spans: Vec<Span> = line_spans
                .iter()
                .map(|(r, rgb)| Span::new(r.start, r.end).color(rgb_color(*rgb)))
                .collect();
            text_el = text_el.spans(spans);
        }
        code_cell = code_cell.child(text_el);

        // Active IME preedit underline (1 px solid ink under the composition).
        if let Some(pre) = &state.preedit
            && !pre.text.is_empty()
        {
            let pre_end = pre.start + pre.text.chars().count();
            if pre.start <= line_end && pre_end >= line_start {
                let c0 = pre.start.saturating_sub(line_start).min(line_chars);
                let c1 = pre_end.saturating_sub(line_start).min(line_chars);
                let x0 = visual_col(&slice.text, c0) as f32 * advance;
                let x1 = visual_col(&slice.text, c1) as f32 * advance;
                if x1 > x0 {
                    code_cell = code_cell.child(
                        el().key(format!("ime-{i}"))
                            .absolute(x0, line_h - 2.0)
                            .size(x1 - x0, 1.0)
                            .fill(fg)
                            .transition(Transition::none()),
                    );
                }
            }
        }

        // The caret: 1.5 px ink stroke, instant, `.ime_cursor()` when focused.
        if i == caret_line {
            let col_chars = caret_pos.saturating_sub(line_start).min(line_chars);
            let cx = visual_col(&slice.text, col_chars) as f32 * advance;
            let mut caret = el()
                .key("pen/caret")
                .absolute(cx, 2.0)
                .size(CARET_WIDTH, line_h - 4.0)
                .fill(fg)
                .transition(Transition::none());
            if state.focused {
                caret = caret.ime_cursor();
            }
            code_cell = code_cell.child(caret);
        }

        let number = el().w(num_w).h(line_h).child(
            text(format!("{}", i + 1))
                .w(num_w)
                .align(Align::End)
                .font(line_no_font)
                .color(faint)
                .transition(Transition::none()),
        );

        let line_row = row()
            .key(format!("ln-{i}"))
            .w_full()
            .h(line_h)
            .pl(NOTEBOOK_MARGIN)
            .pr(24.0)
            .center_y()
            .transition(Transition::none())
            .child(number)
            .child(el().w(GUTTER_GAP).h(line_h))
            .child(code_cell);

        rows = rows.child(line_row);
    }

    let mut body = row().w_full().h_full().child(rows.grow(1.0));

    // Quiet scroll thumb when the buffer is taller than the page (radius 0).
    let total = state.buffer.len_lines();
    if total > visible_lines {
        let track = (state.viewport_height - 16.0).max(24.0);
        let thumb_h = ((visible_lines as f32 / total as f32) * track).clamp(24.0, track);
        let max_scroll = ((total - 1) as f64 * f64::from(line_h)).max(1.0);
        let frac = (state.scroll_y / max_scroll).clamp(0.0, 1.0) as f32;
        let thumb_y = 8.0 + frac * (track - thumb_h);
        let rail = el().w(6.0).h_full().child(
            el().key("pen/scroll-thumb")
                .absolute(0.0, thumb_y)
                .size(2.0, thumb_h)
                .fill(fg.alpha(0.20))
                .transition(Transition::none()),
        );
        body = body.child(rail);
    }

    let mut page = col()
        .key("pen/editor")
        .w_full()
        .h_full()
        .fill(bg)
        .clip_rounded(0.0)
        .transition(Transition::none())
        .child(body);

    // Arrival fades. Position stays put: scroll and typing are instant.
    let visible: Vec<&Diag> = diags
        .iter()
        .filter(|d| d.line >= first_line && d.line < first_line + window.len())
        .collect();
    if !visible.is_empty() {
        let mut layer = el()
            .key("pen/diags")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .transition(diagnostics::diag_transition(reduced_motion));
        let x = state.code_left();
        for diag in visible {
            let row = diag.line - first_line;
            let width = window
                .get(row)
                .map(|slice| visual_col(&slice.text, slice.text.chars().count()) as f32 * advance)
                .unwrap_or(advance)
                .max(advance);
            let y = PAGE_TOP_PAD + row as f32 * line_h + sub_y + line_h - 2.0;
            layer = layer.child(
                el().key(format!("diag-{}", diag.line))
                    .absolute(x, y)
                    .size(width, 1.0)
                    .fill(rgb_color(ink))
                    .transition(Transition::none()),
            );
        }
        page = page.child(layer);
    }
    page
}

fn is_commented(line: &str, start: &str, end: &str) -> bool {
    let trimmed = line.trim();
    let token = start.trim_end();
    let tail = end.trim();
    trimmed.starts_with(token) && (tail.is_empty() || trimmed.ends_with(tail))
}

fn comment_line(line: &str, start: &str, end: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let (lead, rest) = line.split_at(indent);
    format!("{lead}{start}{rest}{end}")
}

fn uncomment_line(line: &str, start: &str, end: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let (lead, rest) = line.split_at(indent);
    let token = start.trim_end();
    let Some(body) = rest.trim_start().strip_prefix(token) else {
        return line.to_owned();
    };
    let body = body.strip_prefix(' ').unwrap_or(body);
    let body = if end.is_empty() {
        body
    } else {
        body.trim_end()
            .strip_suffix(end.trim())
            .unwrap_or(body)
            .trim_end()
    };
    format!("{lead}{body}")
}

fn outdent_text(text: &str) -> String {
    if let Some(rest) = text.strip_prefix('\t') {
        return rest.to_owned();
    }
    let spaces = text.chars().take_while(|c| *c == ' ').count().min(4);
    text.chars().skip(spaces).collect()
}

/// Expand `\t` to spaces aligned to 4-column stops so drawing and column math
/// agree pixel-for-pixel.
fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut col = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let spaces = TAB_STOP - (col % TAB_STOP);
            for _ in 0..spaces {
                out.push(' ');
            }
            col += spaces;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}

/// Visual column (after tab expansion) of `char_col` in `line`.
pub fn visual_col(line: &str, char_col: usize) -> usize {
    let mut vis = 0;
    for ch in line.chars().take(char_col) {
        if ch == '\t' {
            vis += TAB_STOP - (vis % TAB_STOP);
        } else {
            vis += 1;
        }
    }
    vis
}

/// Inverse of [`visual_col`]: closest char column to `target_vis`.
fn char_col_at_visual(line: &str, target_vis: usize) -> usize {
    let mut vis = 0;
    for (idx, ch) in line.chars().enumerate() {
        let w = if ch == '\t' {
            TAB_STOP - (vis % TAB_STOP)
        } else {
            1
        };
        if vis + w / 2 >= target_vis {
            return idx;
        }
        vis += w;
    }
    line.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Chord;

    #[test]
    fn zoom_stops_at_the_ends_and_keeps_the_line() {
        let mut ed = EditorState::from_str("a\nb\nc");
        ed.scroll_y = 20.15;
        ed.apply_chord(Chord::ZoomIn);
        assert_eq!(ed.code_size, 14.0);
        assert!((ed.scroll_y - 20.15 * 14.0 / 13.0).abs() < 0.01);
        ed.set_code_size(28.0);
        ed.apply_chord(Chord::ZoomIn);
        assert_eq!(ed.code_size, 28.0);
        ed.set_code_size(9.0);
        ed.apply_chord(Chord::ZoomOut);
        assert_eq!(ed.code_size, 9.0);
        ed.apply_chord(Chord::ZoomReset);
        assert_eq!(ed.code_size, 13.0);
    }

    #[test]
    fn comment_toggles_and_delete_keeps_the_other_lines() {
        let mut ed = EditorState::from_str("fn a() {}\nkeep").with_language(Some("rust"));
        ed.apply_chord(Chord::ToggleComment);
        assert_eq!(ed.buffer.line(0), "// fn a() {}");
        ed.apply_chord(Chord::ToggleComment);
        assert_eq!(ed.buffer.line(0), "fn a() {}");
        ed.apply_chord(Chord::DeleteLine);
        assert_eq!(ed.buffer.line(0), "keep");
    }

    #[test]
    fn move_line_swaps_with_the_neighbor() {
        let mut ed = EditorState::from_str("a\nb\nc");
        ed.caret = 2;
        ed.anchor = 2;
        assert!(ed.apply_chord(Chord::MoveLineDown));
        assert_eq!(ed.buffer.line(0), "a");
        assert_eq!(ed.buffer.line(1), "c");
        assert_eq!(ed.buffer.line(2), "b");
        ed.caret = ed.buffer.len_chars();
        ed.anchor = ed.caret;
        assert!(ed.apply_chord(Chord::MoveLineUp));
        assert_eq!(ed.buffer.line(1), "b");
        assert_eq!(ed.buffer.line(2), "c");
    }
}
