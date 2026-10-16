//! Makes external text safe to print on a terminal.
//!
//! Escaped characters are written as `\u{..}`: control characters, which can move the cursor,
//! change colors or set the window title, and Unicode format characters, which can reorder
//! or hide text.

use std::borrow::Cow;
use std::fmt::Write as _;

/// One field of external text, such as a name, a path or a server message, with every
/// control character and format character escaped. Line feeds are escaped too, so a field
/// cannot start a new line: `a`, line feed, `b` becomes `a\u{a}b`.
pub fn printable(text: &str) -> Cow<'_, str> {
    escape(text, |c| is_escaped(c) || c == '\n')
}

/// Complete human output with every control character and format character escaped, except
/// line feeds: `ok`, line feed, ESC, `[2J` becomes `ok`, line feed, `\u{1b}[2J`.
pub fn printable_lines(text: &str) -> Cow<'_, str> {
    escape(text, is_escaped)
}

fn escape(text: &str, escaped: impl Fn(char) -> bool) -> Cow<'_, str> {
    if !text.chars().any(&escaped) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if escaped(c) {
            let _ = write!(out, "{}", c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

fn is_escaped(c: char) -> bool {
    (c.is_control() && c != '\n') || is_format(c)
}

/// Whether `c` is in the Unicode general category `Cf` (format): bidirectional overrides,
/// isolates and marks, zero-width characters, the byte order mark and tag characters.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{600}'..='\u{605}'
            | '\u{61c}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{890}'..='\u{891}'
            | '\u{8e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_without_control_or_format_characters_is_unchanged() {
        assert!(matches!(
            printable("Ada Lovelace (admin), Würfel greifen"),
            Cow::Borrowed("Ada Lovelace (admin), Würfel greifen")
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

    #[test]
    fn a_line_feed_inside_a_field_is_escaped() {
        assert_eq!(
            printable("pick\nerror  fake finding"),
            "pick\\u{a}error  fake finding"
        );
    }

    #[test]
    fn bidirectional_overrides_and_isolates_are_escaped() {
        assert_eq!(
            printable("a\u{202e}b\u{2066}c\u{2069}d\u{200f}"),
            "a\\u{202e}b\\u{2066}c\\u{2069}d\\u{200f}"
        );
    }

    #[test]
    fn zero_width_characters_and_tags_are_escaped() {
        assert_eq!(
            printable("a\u{200b}b\u{200d}c\u{2060}d\u{feff}e\u{e0041}"),
            "a\\u{200b}b\\u{200d}c\\u{2060}d\\u{feff}e\\u{e0041}"
        );
    }

    #[test]
    fn printable_lines_keeps_line_feeds_and_escapes_everything_else() {
        assert_eq!(
            printable_lines("ok\n\u{1b}[2J\u{202e}\r\n"),
            "ok\n\\u{1b}[2J\\u{202e}\\u{d}\n"
        );
    }

    #[test]
    fn printable_lines_of_plain_lines_is_unchanged() {
        assert!(matches!(
            printable_lines("ID  TASK\n1   pick\n"),
            Cow::Borrowed("ID  TASK\n1   pick\n")
        ));
    }
}
