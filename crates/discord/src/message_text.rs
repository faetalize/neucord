//! What the message box's text becomes before it goes out, as the official
//! client does it: JavaScript's trim, and the markdown parser's preprocessing
//! (`f` in 478676, which `parse` in 451909 runs the text through).

/// `text` without the whitespace JavaScript's `trim` takes off its ends,
/// which isn't Rust's set: U+FEFF counts and U+0085 doesn't.
pub fn trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

fn is_js_space(c: char) -> bool {
    c != '\u{85}' && (c.is_whitespace() || c == '\u{FEFF}')
}

/// `text` as the parser sees it: `\r\n` and `\r` are newlines, form feeds go
/// and a tab is four spaces.
pub fn preprocess(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").replace('\u{c}', "").replace('\t', "    ")
}

/// A new message's text: trimmed, then preprocessed.
pub fn message(text: &str) -> String {
    preprocess(trim(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_as_javascript_does() {
        assert_eq!(trim("\u{FEFF} hi \u{FEFF}"), "hi");
        assert_eq!(trim("\u{85}hi\u{85}"), "\u{85}hi\u{85}", "U+0085 isn't JavaScript whitespace");
        assert_eq!(trim("\u{2028}\u{a0}hi\u{3000}\n"), "hi");
        assert_eq!(trim("\u{b}\u{c}hi"), "hi");
    }

    #[test]
    fn preprocesses_as_the_parser_does() {
        assert_eq!(preprocess("a\r\nb\rc\u{c}d\te"), "a\nb\ncd    e");
    }

    #[test]
    fn a_message_is_trimmed_before_its_tabs_turn_to_spaces() {
        assert_eq!(message("\thi\t\r\n"), "hi");
        assert_eq!(message("a\tb"), "a    b");
    }
}
