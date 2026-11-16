//! Tab completion. The language server wins. The buffer is the fallback.

pub fn completion_labels(result: &serde_json::Value) -> Vec<String> {
    let Some(items) = result
        .as_array()
        .cloned()
        .or_else(|| result.get("items").and_then(|v| v.as_array()).cloned())
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            item.get("insertText")
                .or_else(|| item.get("label"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .take(20)
        .collect()
}

pub fn completion_label(result: &serde_json::Value) -> Option<String> {
    completion_labels(result).into_iter().next()
}

/// Longest word in `text` that starts with `prefix`, not equal to the prefix.
pub fn buffer_word(text: &str, prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return None;
    }
    let mut best: Option<String> = None;
    for word in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if word.starts_with(prefix)
            && word != prefix
            && best.as_deref().is_none_or(|b| word.len() > b.len())
        {
            best = Some(word.to_owned());
        }
    }
    best
}

pub struct CompletionList {
    pub items: Vec<String>,
    pub selected: usize,
}

pub fn move_selection(list: &mut CompletionList, down: bool) {
    if list.items.is_empty() {
        return;
    }
    let n = list.items.len();
    list.selected = if down {
        (list.selected + 1) % n
    } else {
        (list.selected + n - 1) % n
    };
}

pub fn selected_label(list: &CompletionList) -> Option<&str> {
    list.items.get(list.selected).map(String::as_str)
}

pub fn clone_name(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.contains(['\n', '\r']) || url.starts_with('-') {
        return None;
    }
    if url.split('/').any(|part| part == ".." || part == ".") {
        return None;
    }
    let tail = url.trim_end_matches('/').rsplit(['/', ':']).next()?;
    let name = tail.strip_suffix(".git").unwrap_or(tail);
    if name.is_empty() || name.contains("..") || name.contains(['/', '\\']) || name.starts_with('.')
    {
        return None;
    }
    Some(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_label_beats_a_missing_list() {
        let raw = serde_json::json!({"items":[{"label":"Vec","insertText":"Vec"}]});
        assert_eq!(completion_label(&raw).as_deref(), Some("Vec"));
        assert_eq!(completion_label(&serde_json::json!({})), None);
    }

    #[test]
    fn buffer_word_is_the_longest_prefix() {
        assert_eq!(
            buffer_word("let value = 1; let values = 2;", "val").as_deref(),
            Some("values")
        );
        assert_eq!(buffer_word("let value = 1;", ""), None);
    }

    #[test]
    fn completion_selection_wraps() {
        let mut list = CompletionList {
            items: vec!["a".into(), "b".into(), "c".into()],
            selected: 2,
        };
        move_selection(&mut list, true);
        assert_eq!(selected_label(&list), Some("a"));
        move_selection(&mut list, false);
        assert_eq!(selected_label(&list), Some("c"));
    }

    #[test]
    fn clone_name_rejects_a_parent_escape() {
        assert_eq!(
            clone_name("https://github.com/glostars-net/pen.git").as_deref(),
            Some("pen")
        );
        assert_eq!(clone_name("../pen"), None);
        assert_eq!(clone_name("-n"), None);
    }
}
