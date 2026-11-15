//! A display name and nothing else that looks like an account.

pub fn clamp_scale(raw: i32) -> i32 {
    raw.clamp(90, 140)
}

/// Empty means clear the stored token. The token is not sent anywhere in this phase.
pub fn parse_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(80).collect())
    }
}

pub fn parse_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "Pen".into();
    }
    trimmed.chars().take(40).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_clamps_and_a_blank_token_clears() {
        assert_eq!(clamp_scale(80), 90);
        assert_eq!(clamp_scale(200), 140);
        assert_eq!(clamp_scale(110), 110);
        assert_eq!(parse_token("  "), None);
        assert_eq!(parse_token("ghp_x").as_deref(), Some("ghp_x"));
    }

    #[test]
    fn name_is_trimmed_and_capped() {
        assert_eq!(parse_name("  Ada  "), "Ada");
        assert_eq!(parse_name(""), "Pen");
        assert_eq!(parse_name(&"a".repeat(50)).chars().count(), 40);
    }
}
