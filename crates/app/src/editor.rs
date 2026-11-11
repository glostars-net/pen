//! The writing page: a viewport over a `Buffer`, drawn in Rune with no
//! `TextArea` wrapper.
//!
//! `editor_view` reads only the visible window (`Buffer::viewport` plus 8 lines
//! of overscan), so opening or typing into a 10 MB file touches ~50 lines per
//! frame. It attaches no state-typed handlers of its own, which lets headless
//! tests mount `EditorState` directly while `main.rs` mounts it inside `Pen`.

use buffer::{Buffer, Preedit, apply_ime};
use rune::color::hex;
use rune::{
    Align, Color, Drag, El, Family, Ime, Key, KeyEvent, OpticalSize, Phase, TextStyle, Transition,
    col, el, row, text,
};

/// Code size in points (Design: 13px, line-height 1.55, tracking 0).
pub const CODE_SIZE: f32 = 13.0;
/// Exact row pitch: `13.0 * 1.55 = 20.15` pt.
pub const LINE_H: f32 = 20.15;
/// IBM Plex Mono Regular has `unitsPerEm = 1000` and glyph advance `600`:
/// `13.0 * 600 / 1000 = 7.80` pt exact.
pub const CHAR_ADVANCE: f32 = 7.80;
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

const CODE_FONT: TextStyle = TextStyle {
    size: CODE_SIZE,
    weight: 400.0,
    family: Family::Mono,
    line_height: LINE_H,
    tracking: 0.0,
    variations: "",
    features: "",
    optical_size: OpticalSize::Auto,
    italic: false,
};

const LINE_NO_FONT: TextStyle = TextStyle {
    size: CODE_SIZE,
    weight: 400.0,
    family: Family::Mono,
    line_height: LINE_H,
    tracking: 0.0,
    variations: "",
    features: "'tnum' 1",
    optical_size: OpticalSize::Auto,
    italic: false,
};

/// Paper palette used until Task 9 swaps in `highlight::Theme`.
#[derive(Clone, Copy, Debug)]
pub struct PagePalette {
    pub bg: Color,
    pub fg: Color,
    pub faint: Color,
    pub selection: Color,
}

impl PagePalette {
    /// Paper from Design (`#f6f4f0` ground, `#1a1916` ink, `#5c5954` faint at
    /// 6.35:1, selection = foreground at 12%).
    pub fn paper() -> Self {
        let bg = hex("#f6f4f0").unwrap_or(Color::WHITE);
        let fg = hex("#1a1916").unwrap_or(Color::BLACK);
        let faint = hex("#5c5954").unwrap_or(fg);
        Self { bg, fg, faint, selection: fg.alpha(0.12) }
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
    pub palette: PagePalette,
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
            palette: PagePalette::paper(),
        }
    }

    /// Ordered selection char range when `anchor != caret`.
    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        (self.anchor != self.caret).then(|| self.anchor.min(self.caret)..self.anchor.max(self.caret))
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
        (cols * CHAR_ADVANCE).ceil()
    }

    /// X offset from the page's left edge to column 0 of code.
    pub fn code_left(&self) -> f32 {
        NOTEBOOK_MARGIN + self.line_number_width() + GUTTER_GAP
    }

    /// Scroll vertically by `dy` points (wheel delta: negative scrolls down in
    /// Rune's `WheelEvent`, matching AppKit/winit).
    pub fn scroll_by(&mut self, dy: f32) {
        let max_y = ((self.buffer.len_lines().saturating_sub(1)) as f64) * f64::from(LINE_H);
        self.scroll_y = (self.scroll_y - f64::from(dy)).clamp(0.0, max_y);
    }

    /// Keep the caret's line inside the visible window after an edit or move.
    pub fn reveal_caret(&mut self) {
        let line = self.buffer.char_to_line(self.visual_caret());
        let top = (line as f64) * f64::from(LINE_H);
        let view = f64::from((self.viewport_height - PAGE_TOP_PAD - LINE_H).max(LINE_H));
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
                    let end = (start + self.buffer.line_len_chars(line) + 1).min(self.buffer.len_chars());
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
                    self.preedit = Some(Preedit { start: self.caret, text: String::new() });
                }
                let _ = apply_ime(&mut self.buffer, &mut self.preedit, text, false);
                self.reveal_caret();
                true
            }
            Ime::Commit(text) => {
                if self.preedit.is_none() {
                    self.delete_selection();
                    self.preedit = Some(Preedit { start: self.caret, text: String::new() });
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
                self.move_to(self.buffer.line_to_char(line) + self.buffer.line_len_chars(line), shift);
                true
            }
            Key::PageUp => {
                let step = ((self.viewport_height / LINE_H).floor() as usize).max(1);
                let line = self.buffer.char_to_line(self.caret).saturating_sub(step);
                self.move_to(self.buffer.line_to_char(line), shift);
                true
            }
            Key::PageDown => {
                let step = ((self.viewport_height / LINE_H).floor() as usize).max(1);
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

    fn delete_selection(&mut self) -> bool {
        let Some(sel) = self.selection() else { return false };
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
        let first_line = (self.scroll_y / f64::from(LINE_H)).floor() as usize;
        let sub_y = -((self.scroll_y % f64::from(LINE_H)) as f32);
        let rel_y = (y - PAGE_TOP_PAD - sub_y).max(0.0);
        let line = (first_line + (rel_y / LINE_H).floor() as usize)
            .min(self.buffer.len_lines().saturating_sub(1));
        let line_str = self.buffer.line(line);
        let vis_col = ((x - self.code_left()) / CHAR_ADVANCE).round().max(0.0) as usize;
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
}

/// Draw the editor page from `state`. Pure function of `&EditorState`: reads
/// only `viewport(first_line, visible + OVERSCAN_LINES)`.
pub fn editor_view(state: &EditorState) -> El {
    let p = state.palette;
    let first_line = (state.scroll_y / f64::from(LINE_H)).floor() as usize;
    let sub_y = -((state.scroll_y % f64::from(LINE_H)) as f32);
    let visible_lines = ((state.viewport_height / LINE_H).ceil() as usize).max(1);
    let window = state.buffer.viewport(first_line, visible_lines + OVERSCAN_LINES);

    let caret_pos = state.visual_caret();
    let caret_line = state.buffer.char_to_line(caret_pos);
    let selection = state.selection();
    let num_w = state.line_number_width();

    let mut rows = col().key("pen/rows").w_full().pt(PAGE_TOP_PAD).offset(0.0, sub_y);
    for slice in &window {
        let i = slice.line_idx;
        let line_start = state.buffer.line_to_char(i);
        let line_chars = slice.text.chars().count();
        let line_end = line_start + line_chars;
        let expanded = expand_tabs(&slice.text);

        let mut code_cell = el().grow(1.0).h(LINE_H);

        // Selection band behind text (foreground at 12%, radius 0).
        if let Some(sel) = &selection
            && sel.start <= line_end
            && sel.end >= line_start
        {
            let c0 = sel.start.saturating_sub(line_start).min(line_chars);
            let c1 = sel.end.saturating_sub(line_start).min(line_chars);
            let x0 = visual_col(&slice.text, c0) as f32 * CHAR_ADVANCE;
            let extra = if sel.end > line_end { CHAR_ADVANCE } else { 0.0 };
            let x1 = visual_col(&slice.text, c1) as f32 * CHAR_ADVANCE + extra;
            if x1 > x0 {
                code_cell = code_cell.child(
                    el()
                        .key(format!("sel-{i}"))
                        .absolute(x0, 0.0)
                        .size(x1 - x0, LINE_H)
                        .fill(p.selection)
                        .transition(Transition::none()),
                );
            }
        }

        // The code text itself.
        code_cell = code_cell.child(
            text(expanded).font(CODE_FONT).color(p.fg).transition(Transition::none()),
        );

        // Active IME preedit underline (1 px solid ink under the composition).
        if let Some(pre) = &state.preedit
            && !pre.text.is_empty()
        {
            let pre_end = pre.start + pre.text.chars().count();
            if pre.start <= line_end && pre_end >= line_start {
                let c0 = pre.start.saturating_sub(line_start).min(line_chars);
                let c1 = pre_end.saturating_sub(line_start).min(line_chars);
                let x0 = visual_col(&slice.text, c0) as f32 * CHAR_ADVANCE;
                let x1 = visual_col(&slice.text, c1) as f32 * CHAR_ADVANCE;
                if x1 > x0 {
                    code_cell = code_cell.child(
                        el()
                            .key(format!("ime-{i}"))
                            .absolute(x0, LINE_H - 2.0)
                            .size(x1 - x0, 1.0)
                            .fill(p.fg)
                            .transition(Transition::none()),
                    );
                }
            }
        }

        // The caret: 1.5 px ink stroke, instant, `.ime_cursor()` when focused.
        if i == caret_line {
            let col_chars = caret_pos.saturating_sub(line_start).min(line_chars);
            let cx = visual_col(&slice.text, col_chars) as f32 * CHAR_ADVANCE;
            let mut caret = el()
                .key("pen/caret")
                .absolute(cx, 2.0)
                .size(CARET_WIDTH, LINE_H - 4.0)
                .fill(p.fg)
                .transition(Transition::none());
            if state.focused {
                caret = caret.ime_cursor();
            }
            code_cell = code_cell.child(caret);
        }

        let number = el()
            .w(num_w)
            .h(LINE_H)
            .child(
                text(format!("{}", i + 1))
                    .w(num_w)
                    .align(Align::End)
                    .font(LINE_NO_FONT)
                    .color(p.faint)
                    .transition(Transition::none()),
            );

        let line_row = row()
            .key(format!("ln-{i}"))
            .w_full()
            .h(LINE_H)
            .pl(NOTEBOOK_MARGIN)
            .pr(24.0)
            .center_y()
            .transition(Transition::none())
            .child(number)
            .child(el().w(GUTTER_GAP).h(LINE_H))
            .child(code_cell);

        rows = rows.child(line_row);
    }

    let mut body = row().w_full().h_full().child(rows.grow(1.0));

    // Quiet scroll thumb when the buffer is taller than the page (radius 0).
    let total = state.buffer.len_lines();
    if total > visible_lines {
        let track = (state.viewport_height - 16.0).max(24.0);
        let thumb_h = ((visible_lines as f32 / total as f32) * track).clamp(24.0, track);
        let max_scroll = ((total - 1) as f64 * f64::from(LINE_H)).max(1.0);
        let frac = (state.scroll_y / max_scroll).clamp(0.0, 1.0) as f32;
        let thumb_y = 8.0 + frac * (track - thumb_h);
        let rail = el().w(6.0).h_full().child(
            el()
                .key("pen/scroll-thumb")
                .absolute(0.0, thumb_y)
                .size(2.0, thumb_h)
                .fill(p.fg.alpha(0.20))
                .transition(Transition::none()),
        );
        body = body.child(rail);
    }

    col()
        .key("pen/editor")
        .w_full()
        .h_full()
        .fill(p.bg)
        .clip_rounded(0.0)
        .instant_changes()
        .transition(Transition::none())
        .child(body)
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
        let w = if ch == '\t' { TAB_STOP - (vis % TAB_STOP) } else { 1 };
        if vis + w / 2 >= target_vis {
            return idx;
        }
        vis += w;
    }
    line.chars().count()
}
