//! Display-width helpers (deviation 22: C pads by bytes). GPL-2.0-only.

use unicode_width::UnicodeWidthStr;

pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// `s` followed by spaces up to display width `w`; longer strings are returned unchanged.
pub fn pad_right(s: &str, w: usize) -> String {
    let mut out = String::from(s);
    for _ in display_width(s)..w {
        out.push(' ');
    }
    out
}

/// Longest prefix of `s` whose display width is at most `w`.
pub fn truncate_to(s: &str, w: usize) -> &str {
    let mut used = 0;
    for (i, ch) in s.char_indices() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > w {
            return &s[..i];
        }
        used += cw;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_characters_count_double() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("日本"), 4);
        assert_eq!(pad_right("日本", 6), "日本  ");
        assert_eq!(pad_right("abcdef", 3), "abcdef");
        assert_eq!(truncate_to("日本語", 4), "日本");
        assert_eq!(truncate_to("abc", 10), "abc");
        assert_eq!(truncate_to("", 3), "");
    }
}
