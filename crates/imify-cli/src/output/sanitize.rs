//! Makes text from the API and the identity provider safe to print on a terminal.

use std::borrow::Cow;
use std::fmt::Write as _;

/// `text` with every control character escaped as `\u{..}`, except line feeds. C0 controls,
/// DEL and C1 controls cannot move the cursor, change colors or set the window title.
pub fn printable(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_escaped) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if is_escaped(c) {
            let _ = write!(escaped, "{}", c.escape_unicode());
        } else {
            escaped.push(c);
        }
    }
    Cow::Owned(escaped)
}

fn is_escaped(c: char) -> bool {
    c.is_control() && c != '\n'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_without_control_characters_is_unchanged() {
        assert!(matches!(
            printable("Ada Lovelace (admin)\nline two"),
            Cow::Borrowed("Ada Lovelace (admin)\nline two")
        ));
    }

    #[test]
    fn escape_sequences_are_escaped() {
        assert_eq!(
            printable("bad\u{1b}]0;title\u{7}\u{1b}[2J"),
            "bad\\u{1b}]0;title\\u{7}\\u{1b}[2J"
        );
    }

    #[test]
    fn carriage_returns_del_and_c1_controls_are_escaped() {
        assert_eq!(printable("a\rb\u{7f}c\u{9b}d"), "a\\u{d}b\\u{7f}c\\u{9b}d");
    }
}
