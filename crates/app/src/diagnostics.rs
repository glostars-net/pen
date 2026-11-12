//! Faint diagnostic underlines. A color under 4.5:1 is dropped.

use highlight::{Rgb, Theme, contrast_ratio};
use rune::{Spring, SpringPair, Transition};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub line: usize,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagSnapshot {
    pub by_file: Vec<(std::path::PathBuf, Vec<Diag>)>,
}

impl DiagSnapshot {
    pub fn for_file(&self, rel: &std::path::Path) -> Vec<Diag> {
        self.by_file
            .iter()
            .find(|(path, _)| path == rel)
            .map(|(_, diags)| diags.clone())
            .unwrap_or_default()
    }
}

/// Theme faint ink, unless `override_hex` clears 4.5:1 against `theme.bg`.
pub fn underline_color(theme: &Theme, override_hex: Option<&str>) -> Rgb {
    let Some(hex) = override_hex else {
        return theme.faint;
    };
    let Some(rgb) = Rgb::try_from_hex(hex) else {
        return theme.faint;
    };
    if contrast_ratio(rgb, theme.bg) < 4.5 {
        theme.faint
    } else {
        rgb
    }
}

pub fn message_on_line(diags: &[Diag], line: usize) -> Option<&str> {
    diags
        .iter()
        .find(|d| d.line == line)
        .map(|d| d.message.as_str())
}

/// Opacity fade when a diagnostic set arrives. Bounce 0. Reduced motion is instant.
/// Typing, the caret, and go-to-definition do not use this.
pub fn diag_transition(reduced: bool) -> Transition {
    if reduced {
        Transition::none()
    } else {
        Transition::opacity().springs(SpringPair::new(
            Spring::new(0.20, 0.0),
            Spring::new(0.15, 0.0),
        ))
    }
}

/// A crashed server may restart once a minute. A missing binary never restarts.
pub fn allow_restart(
    missing: bool,
    last_attempt: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    if missing {
        return false;
    }
    match last_attempt {
        None => true,
        Some(then) => now.saturating_duration_since(then) >= std::time::Duration::from_secs(60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_contrast_diagnostic_color_is_dropped() {
        let theme =
            highlight::load_theme(r##"{"name":"p","bg":"#ffffff","fg":"#111111"}"##).unwrap();
        let c = underline_color(&theme, Some("#eeeeee"));
        assert_eq!(c, theme.faint);
        let kept = underline_color(&theme, Some("#111111"));
        assert_eq!(kept, Rgb::from_hex("#111111"));
    }

    #[test]
    fn diagnostic_motion_is_opacity_unless_reduced() {
        assert_eq!(diag_transition(true), Transition::none());
        assert_ne!(diag_transition(false), Transition::none());
    }

    #[test]
    fn missing_binary_does_not_restart() {
        let now = std::time::Instant::now();
        assert!(!allow_restart(true, None, now));
        assert!(allow_restart(false, None, now));
        assert!(!allow_restart(
            false,
            Some(now),
            now + std::time::Duration::from_secs(10)
        ));
        assert!(allow_restart(
            false,
            Some(now),
            now + std::time::Duration::from_secs(60)
        ));
    }
}
