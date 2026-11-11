//! Ink, Paper, and contrast-checked theme files.
//!
//! Pen's default look is monochrome ink on paper: syntax is a five-step OKLab
//! lightness ramp of `fg` toward `bg`, with every step and `faint` verified at
//! a minimum of 4.5:1 WCAG contrast against `bg`. A JSON theme file may supply `accent`
//! or `syntax` colors, and any entry under 4.5:1 is dropped in favor of the
//! derived ramp.

use std::ops::Range;

use rune_kit::syntax::{self, Paint, Syntax};
use serde::Deserialize;

pub use rune_kit::syntax::lang_for_path;

pub const INK_JSON: &str = include_str!("../themes/ink.json");
pub const PAPER_JSON: &str = include_str!("../themes/paper.json");

const MIN_CONTRAST: f32 = 4.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Parse `#rgb` or `#rrggbb`. Panics only on invalid test literals; runtime
    /// JSON parsing goes through [`Rgb::try_from_hex`].
    pub fn from_hex(s: &str) -> Rgb {
        Self::try_from_hex(s).unwrap_or_else(|| panic!("invalid hex color literal: {s}"))
    }

    pub fn try_from_hex(s: &str) -> Option<Rgb> {
        let h = s.trim().strip_prefix('#')?;
        match h.len() {
            3 => {
                let c = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|v| v * 17);
                Some(Rgb(c(0)?, c(1)?, c(2)?))
            }
            6 | 8 => {
                let p = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
                Some(Rgb(p(0)?, p(2)?, p(4)?))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub bg: Rgb,
    pub fg: Rgb,
    pub faint: Rgb,
    pub selection: Rgb,
    pub added: Rgb,
    pub removed: Rgb,
    pub syntax: [Rgb; 5],
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("malformed theme JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing or invalid `{field}` hex color")]
    MissingColor { field: &'static str },
}

#[derive(Deserialize)]
struct RawTheme {
    #[serde(default)]
    name: Option<String>,
    bg: Option<String>,
    fg: Option<String>,
    #[serde(default)]
    accent: Option<String>,
    #[serde(default)]
    added: Option<String>,
    #[serde(default)]
    removed: Option<String>,
    #[serde(default)]
    syntax: Option<Vec<String>>,
}

/// Built-in dark theme (`#12110f` ground, `#f4f1ea` text).
pub fn ink() -> Theme {
    #[allow(clippy::expect_used)]
    load_theme(INK_JSON).expect("bundled ink.json is valid")
}

/// Built-in light theme (`#f6f4f0` ground, `#1a1916` text).
pub fn paper() -> Theme {
    #[allow(clippy::expect_used)]
    load_theme(PAPER_JSON).expect("bundled paper.json is valid")
}

/// WCAG 2.x relative luminance contrast ratio (`1.0..=21.0`).
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f32 {
    let la = rel_luminance(a);
    let lb = rel_luminance(b);
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Parse a theme JSON string and enforce >= 4.5:1 contrast on `faint`,
/// `accent`, and every `syntax` step.
pub fn load_theme(json: &str) -> Result<Theme, ThemeError> {
    let raw: RawTheme = serde_json::from_str(json)?;
    let bg = raw
        .bg
        .as_deref()
        .and_then(Rgb::try_from_hex)
        .ok_or(ThemeError::MissingColor { field: "bg" })?;
    let fg = raw
        .fg
        .as_deref()
        .and_then(Rgb::try_from_hex)
        .ok_or(ThemeError::MissingColor { field: "fg" })?;
    let name = raw.name.unwrap_or_else(|| "Custom".to_owned());

    // Design: start faint at `#a8a49c` (Ink) / `#5c5954` (Paper) — or an OKLab
    // 48% step toward `bg` for custom themes — and walk toward `fg` until >= 4.5:1.
    let faint_seed = if bg == Rgb(0x12, 0x11, 0x0f) && fg == Rgb(0xf4, 0xf1, 0xea) {
        Rgb(0xa8, 0xa4, 0x9c)
    } else if bg == Rgb(0xf6, 0xf4, 0xf0) && fg == Rgb(0x1a, 0x19, 0x16) {
        Rgb(0x5c, 0x59, 0x54)
    } else {
        mix_oklab(fg, bg, 0.48)
    };
    let faint = walk_to_contrast(faint_seed, fg, bg);

    // Selection: foreground at 12% composited over ground (Design: not blue).
    let mix_u8 = |f: u8, b: u8| (0.12 * f32::from(f) + 0.88 * f32::from(b)).round() as u8;
    let selection = Rgb(mix_u8(fg.0, bg.0), mix_u8(fg.1, bg.1), mix_u8(fg.2, bg.2));

    // Five OKLab lightness steps from `fg` toward `bg`, each walked back to >= 4.5:1.
    let weights = [0.0_f32, 0.14, 0.27, 0.40, 0.52];
    let mut syntax = weights.map(|t| walk_to_contrast(mix_oklab(fg, bg, t), fg, bg));

    if let Some(acc) = raw.accent.as_deref().and_then(Rgb::try_from_hex)
        && contrast_ratio(acc, bg) >= MIN_CONTRAST
    {
        syntax[0] = acc;
    }
    if let Some(list) = raw.syntax {
        for (slot, hex_str) in syntax.iter_mut().zip(list.iter()) {
            if let Some(c) = Rgb::try_from_hex(hex_str)
                && contrast_ratio(c, bg) >= MIN_CONTRAST
            {
                *slot = c;
            }
        }
    }

    // Added / removed diff ink steps: crisp forward ink for additions, softer
    // receding ink for removals — both contrast-checked >= 4.5:1 against `bg`.
    let mut added = walk_to_contrast(mix_oklab(fg, bg, 0.10), fg, bg);
    let mut removed = walk_to_contrast(mix_oklab(fg, bg, 0.44), fg, bg);
    if let Some(a) = raw.added.as_deref().and_then(Rgb::try_from_hex)
        && contrast_ratio(a, bg) >= MIN_CONTRAST
    {
        added = a;
    }
    if let Some(r) = raw.removed.as_deref().and_then(Rgb::try_from_hex)
        && contrast_ratio(r, bg) >= MIN_CONTRAST
    {
        removed = r;
    }

    Ok(Theme {
        name,
        bg,
        fg,
        faint,
        selection,
        added,
        removed,
        syntax,
    })
}

/// Highlight only the visible viewport lines through `rune_kit::syntax::highlight`
/// and map token kinds onto `theme.syntax`. Returns per-line `(byte_range, Rgb)`.
pub fn highlight_lines(
    lang: Option<&str>,
    expanded_lines: &[String],
    theme: &Theme,
) -> Vec<Vec<(Range<usize>, Rgb)>> {
    let mut per_line = vec![Vec::new(); expanded_lines.len()];
    let Some(lang) = lang else { return per_line };
    if expanded_lines.is_empty() || !syntax::known(lang) {
        return per_line;
    }

    let total_bytes: usize = expanded_lines.iter().map(|l| l.len() + 1).sum();
    let mut joined = String::with_capacity(total_bytes);
    let mut offsets = Vec::with_capacity(expanded_lines.len());
    for (idx, line) in expanded_lines.iter().enumerate() {
        if idx > 0 {
            joined.push('\n');
        }
        let start = joined.len();
        joined.push_str(line);
        offsets.push(start..joined.len());
    }

    let tokens = syntax::highlight(Some(lang), &joined);
    let mut line_cursor = 0usize;
    for (range, paint) in tokens.iter() {
        let Paint::Kind(kind) = *paint else { continue };
        let rgb = step_for_kind(kind, theme);
        while line_cursor < offsets.len() && offsets[line_cursor].end <= range.start {
            line_cursor += 1;
        }
        let mut li = line_cursor;
        while li < offsets.len() && offsets[li].start < range.end {
            let line_r = &offsets[li];
            let s = range.start.max(line_r.start) - line_r.start;
            let e = range.end.min(line_r.end) - line_r.start;
            if e > s {
                per_line[li].push((s..e, rgb));
            }
            li += 1;
        }
    }
    per_line
}

fn step_for_kind(kind: Syntax, theme: &Theme) -> Rgb {
    match kind {
        Syntax::Keyword | Syntax::Type | Syntax::Heading => theme.syntax[0],
        Syntax::Function | Syntax::Property | Syntax::Link => theme.syntax[1],
        Syntax::String | Syntax::Number | Syntax::Regex | Syntax::Attribute | Syntax::Inserted => {
            theme.syntax[2]
        }
        Syntax::Operator
        | Syntax::Punctuation
        | Syntax::Parameter
        | Syntax::Variable
        | Syntax::Deleted => theme.syntax[3],
        Syntax::Comment => theme.syntax[4],
        _ => theme.fg,
    }
}

fn walk_to_contrast(start: Rgb, fg: Rgb, bg: Rgb) -> Rgb {
    if contrast_ratio(start, bg) >= MIN_CONTRAST || contrast_ratio(fg, bg) < MIN_CONTRAST {
        return start;
    }
    for i in 1..=64 {
        let t = i as f32 / 64.0;
        let candidate = mix_oklab(start, fg, t);
        if contrast_ratio(candidate, bg) >= MIN_CONTRAST {
            return candidate;
        }
    }
    fg
}

fn srgb_to_linear(u: u8) -> f32 {
    let c = f32::from(u) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

fn rel_luminance(c: Rgb) -> f32 {
    0.2126 * srgb_to_linear(c.0) + 0.7152 * srgb_to_linear(c.1) + 0.0722 * srgb_to_linear(c.2)
}

/// Mix `a` toward `b` by `t` in Björn Ottosson's OKLab perceptual space.
fn mix_oklab(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let la = rgb_to_oklab(a);
    let lb = rgb_to_oklab(b);
    let m = [
        la[0] + (lb[0] - la[0]) * t,
        la[1] + (lb[1] - la[1]) * t,
        la[2] + (lb[2] - la[2]) * t,
    ];
    oklab_to_rgb(m)
}

fn rgb_to_oklab(c: Rgb) -> [f32; 3] {
    let (r, g, b) = (
        srgb_to_linear(c.0),
        srgb_to_linear(c.1),
        srgb_to_linear(c.2),
    );
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn oklab_to_rgb([ll, aa, bb]: [f32; 3]) -> Rgb {
    let l_ = ll + 0.396_337_78 * aa + 0.215_803_76 * bb;
    let m_ = ll - 0.105_561_346 * aa - 0.063_854_17 * bb;
    let s_ = ll - 0.089_484_18 * aa - 1.291_485_5 * bb;
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let b = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
    Rgb(linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_and_ink_text_clear_contrast() {
        for raw in [
            include_str!("../themes/paper.json"),
            include_str!("../themes/ink.json"),
        ] {
            let t = load_theme(raw).unwrap();
            assert!(contrast_ratio(t.fg, t.bg) >= 4.5);
            assert!(contrast_ratio(t.faint, t.bg) >= 4.5);
            assert!(contrast_ratio(t.added, t.bg) >= 4.5);
            assert!(contrast_ratio(t.removed, t.bg) >= 4.5);
            assert_ne!(t.added, t.removed);
            for step in t.syntax {
                assert!(contrast_ratio(step, t.bg) >= 4.5);
            }
        }
    }

    #[test]
    fn low_contrast_accent_is_dropped() {
        let t = load_theme(r##"{"name":"x","bg":"#ffffff","fg":"#111111","accent":"#eeeeee"}"##)
            .unwrap();
        assert_ne!(t.syntax[0], Rgb::from_hex("#eeeeee"));
    }

    #[test]
    fn low_contrast_added_and_removed_are_dropped() {
        let t = load_theme(
            r##"{"name":"x","bg":"#ffffff","fg":"#111111","added":"#eeeeee","removed":"#f2f2f2"}"##,
        )
        .unwrap();
        assert_ne!(t.added, Rgb::from_hex("#eeeeee"));
        assert_ne!(t.removed, Rgb::from_hex("#f2f2f2"));
        assert!(contrast_ratio(t.added, t.bg) >= 4.5);
        assert!(contrast_ratio(t.removed, t.bg) >= 4.5);
    }

    #[test]
    fn highlight_lines_colors_rust_viewport() {
        let lines = vec![
            "fn main() {".to_owned(),
            "    // note".to_owned(),
            "}".to_owned(),
        ];
        let spans = highlight_lines(Some("rust"), &lines, &ink());
        assert_eq!(spans.len(), 3);
        assert!(
            !spans[0].is_empty(),
            "expected `fn` / `main` tokens on line 0"
        );
        assert!(!spans[1].is_empty(), "expected comment token on line 1");
    }
}
