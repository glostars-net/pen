//! A display name and nothing else that looks like an account.

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
    fn name_is_trimmed_and_capped() {
        assert_eq!(parse_name("  Ada  "), "Ada");
        assert_eq!(parse_name(""), "Pen");
        assert_eq!(parse_name(&"a".repeat(50)).chars().count(), 40);
    }
}
