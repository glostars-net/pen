//! Maps a [`highlight::Theme`] onto Rune's linear [`Color`] and `rune_kit::Kit`.
//!
//! Design rules enforced here:
//! - Solid surfaces (`page`, `bg_base`, `bg_deep`, `layer_01` share `theme.bg`).
//! - Hairline rules are `fg` at 20% alpha; focus rings are 1px solid `fg`.
//! - Added and removed diff colors map to `theme.added` / `theme.removed` ink steps (no green/red).
//! - Chrome controls use 6px radius; the writing surface uses 0.
//! - UI labels are 13px IBM Plex Sans; faint/muted text never drops below `theme.faint` (>= 4.5:1).

use highlight::{Rgb, Theme};
use rune::color::srgb_decode;
use rune::{Color, Family, OpticalSize, Spring, TextStyle};
use rune_kit::syntax::{Highlighter, Paint, Syntax, builtin};
use rune_kit::{Kit, PaletteSeed, StateColors};

pub fn rgb_color(c: Rgb) -> Color {
    Color {
        r: srgb_decode(f32::from(c.0) / 255.0),
        g: srgb_decode(f32::from(c.1) / 255.0),
        b: srgb_decode(f32::from(c.2) / 255.0),
        a: 1.0,
    }
}

pub const UI_FONT: TextStyle = TextStyle {
    size: 13.0,
    weight: 400.0,
    family: Family::Sans,
    line_height: 20.0,
    tracking: 0.0,
    variations: "",
    features: "",
    optical_size: OpticalSize::Auto,
    italic: false,
};

pub const UI_MEDIUM_FONT: TextStyle = TextStyle {
    size: 13.0,
    weight: 500.0,
    family: Family::Sans,
    line_height: 20.0,
    tracking: 0.0,
    variations: "",
    features: "",
    optical_size: OpticalSize::Auto,
    italic: false,
};

pub const MONO_UI_FONT: TextStyle = TextStyle {
    size: 13.0,
    weight: 400.0,
    family: Family::Mono,
    line_height: 20.0,
    tracking: 0.0,
    variations: "",
    features: "",
    optical_size: OpticalSize::Auto,
    italic: false,
};

fn ink_state_colors(bg: Color, step: Color) -> StateColors {
    StateColors {
        bg: step.alpha(0.10).mix(bg, 0.90),
        fg: step,
        border: step.alpha(0.28),
        icon: step,
    }
}

pub fn build_kit(theme: &Theme) -> Kit {
    let bg = rgb_color(theme.bg);
    let fg = rgb_color(theme.fg);
    let faint = rgb_color(theme.faint);
    let added = rgb_color(theme.added);
    let removed = rgb_color(theme.removed);
    let accent = rgb_color(theme.syntax[0]);
    let rule = fg.alpha(0.20);

    let mut k = Kit::from_palette(PaletteSeed { bg, fg, accent });
    // Solid paper/ink surfaces: sidebar and page share one ground, separated by a 1px hairline.
    k.c.page = bg;
    k.c.bg_base = bg;
    k.c.bg_deep = bg;
    k.c.layer_01 = bg;
    k.c.surface_base = bg;
    k.c.surface_weak = bg;
    k.c.text_base = fg;
    k.c.text_muted = faint;
    k.c.text_faint = faint;
    k.c.icon_base = fg;
    k.c.icon_muted = faint;
    k.c.icon_faint = faint;
    k.c.border_muted = rule;
    k.c.border_base = rule;
    k.c.border_weak = rule;
    k.c.border_strong = fg.alpha(0.35);
    k.c.border_focus = fg;
    k.c.hover = fg.alpha(0.06);
    k.c.pressed = fg.alpha(0.12);

    // Task 4: map kit success/danger/info onto the theme's contrast-checked ink steps.
    k.c.success = ink_state_colors(bg, added);
    k.c.danger = ink_state_colors(bg, removed);
    k.c.warning = ink_state_colors(bg, faint);
    k.c.info = ink_state_colors(bg, faint);

    // Borders-only depth: no drop shadows or frosted elevation.
    k.elevation.raised.clear();
    k.elevation.floating.clear();
    k.elevation.overlay.clear();
    k.elevation.md.clear();

    // Radius: 6px on chrome controls (0 on the page itself, set in `editor_view`).
    k.radius.xs = 6.0;
    k.radius.sm = 6.0;
    k.radius.md = 6.0;
    k.radius.lg = 6.0;
    k.radius.xl = 6.0;

    // 13px IBM Plex Sans across UI labels; IBM Plex Mono in diff/code blocks.
    k.ty.ui = UI_FONT;
    k.ty.ui_medium = UI_MEDIUM_FONT;
    k.ty.ui_compact = TextStyle {
        line_height: 18.0,
        ..UI_FONT
    };
    k.ty.body = UI_FONT;
    k.ty.body_medium = UI_MEDIUM_FONT;
    k.ty.text_small = UI_FONT;
    k.ty.text_small_medium = UI_MEDIUM_FONT;
    k.ty.text_xsmall = TextStyle {
        size: 11.0,
        line_height: 16.0,
        features: "'tnum' 1",
        ..UI_FONT
    };
    k.ty.text_xsmall_medium = TextStyle {
        size: 11.0,
        line_height: 16.0,
        features: "'tnum' 1",
        ..UI_MEDIUM_FONT
    };
    k.ty.mono = MONO_UI_FONT;

    // Disclosure spring matches Design (0.35s, bounce 0).
    k.motion.disclosure = Spring::new(0.35, 0.0);
    k
}

/// Monochrome syntax highlighter for `rune_kit::diff::DiffState`: maps every
/// lexer token onto `theme.syntax`'s five contrast-checked ink steps so the
/// diff view never falls back to chromatic rainbow defaults.
pub fn diff_highlighter(theme: &Theme) -> Highlighter {
    let steps = theme.syntax.map(rgb_color);
    let fg = rgb_color(theme.fg);
    let key = (theme.bg, theme.fg, theme.syntax);
    Highlighter::new(move |lang, code| {
        builtin(lang, code)
            .into_iter()
            .map(|(range, paint)| {
                let c = match paint {
                    Paint::Color(c) => c,
                    Paint::Kind(kind) => match kind {
                        Syntax::Keyword | Syntax::Type | Syntax::Heading => steps[0],
                        Syntax::Function | Syntax::Property | Syntax::Link => steps[1],
                        Syntax::String
                        | Syntax::Number
                        | Syntax::Regex
                        | Syntax::Attribute
                        | Syntax::Inserted => steps[2],
                        Syntax::Operator
                        | Syntax::Punctuation
                        | Syntax::Parameter
                        | Syntax::Variable
                        | Syntax::Deleted => steps[3],
                        Syntax::Comment => steps[4],
                        _ => fg,
                    },
                };
                (range, Paint::Color(c))
            })
            .collect()
    })
    .key(key)
}
