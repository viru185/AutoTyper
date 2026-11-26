//! Turning input text into the keystrokes that will be sent.
//!
//! Pure functions only, so they are easy to unit test.

/// One thing to type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    /// A printable character.
    Char(char),
    /// A line break (`\r\n`, `\r` or `\n`): pressed as the Enter key.
    Enter,
    /// A tab: pressed as the Tab key.
    Tab,
}

/// Splits `text` into [`Token`]s.
///
/// * every line-break style becomes a single [`Token::Enter`];
/// * other control characters (NUL, bell, form feed, ...) are dropped because they cannot be
///   typed meaningfully; format characters such as zero-width joiners are kept;
/// * with `trim_trailing_newline`, one trailing line break is removed: text copied from a
///   password manager or a terminal often ends with one, and typing it would press Enter.
pub fn tokenize(text: &str, trim_trailing_newline: bool) -> Vec<Token> {
    let text = if trim_trailing_newline {
        trim_one_newline(text)
    } else {
        text
    };

    let mut tokens = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                // `\r\n` is one line break, not two.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                tokens.push(Token::Enter);
            }
            '\n' => tokens.push(Token::Enter),
            '\t' => tokens.push(Token::Tab),
            c if c.is_control() => {}
            c => tokens.push(Token::Char(c)),
        }
    }
    tokens
}

fn trim_one_newline(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .or_else(|| text.strip_suffix('\r'))
        .unwrap_or(text)
}

/// `(characters, lines)` for the "1,234 chars · 12 lines" label.
pub fn stats(text: &str) -> (usize, usize) {
    let chars = text.chars().count();
    let lines = if text.is_empty() {
        0
    } else {
        text.lines().count().max(1)
    };
    (chars, lines)
}

/// Formats a count with thousands separators: `1234` → `"1,234"`.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use Token::{Char, Enter, Tab};

    #[test]
    fn plain_text_is_one_token_per_char() {
        assert_eq!(
            tokenize("aB3!", false),
            vec![Char('a'), Char('B'), Char('3'), Char('!')]
        );
    }

    #[test]
    fn every_line_break_style_is_one_enter() {
        assert_eq!(
            tokenize("a\r\nb\nc\rd", false),
            vec![
                Char('a'),
                Enter,
                Char('b'),
                Enter,
                Char('c'),
                Enter,
                Char('d')
            ]
        );
    }

    #[test]
    fn tabs_are_tab_keys_and_other_controls_are_dropped() {
        assert_eq!(
            tokenize("a\tb\u{0}\u{7}\u{c}c", false),
            vec![Char('a'), Tab, Char('b'), Char('c')]
        );
    }

    #[test]
    fn non_bmp_characters_survive() {
        assert_eq!(tokenize("é😀", false), vec![Char('é'), Char('😀')]);
    }

    #[test]
    fn only_one_trailing_newline_is_trimmed() {
        assert_eq!(tokenize("pw\r\n", true), vec![Char('p'), Char('w')]);
        assert_eq!(tokenize("pw\n\n", true), vec![Char('p'), Char('w'), Enter]);
        assert_eq!(tokenize("pw\n", false), vec![Char('p'), Char('w'), Enter]);
        assert_eq!(tokenize("", true), vec![]);
    }

    #[test]
    fn stats_count_chars_and_lines() {
        assert_eq!(stats(""), (0, 0));
        assert_eq!(stats("abc"), (3, 1));
        assert_eq!(stats("a\nb\n"), (4, 2));
        assert_eq!(stats("é😀"), (2, 1));
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1234), "1,234");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
