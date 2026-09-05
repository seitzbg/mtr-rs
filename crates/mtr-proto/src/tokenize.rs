//! Line tokenizer. Ported from packet/cmdparse.c (mtr 0.96, commit 7b01773). GPL-2.0-only.

use crate::{MAX_ARGUMENTS, ParseError};

/// A tokenized protocol line: `<token> <name> [<key> <value>]*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line<'a> {
    pub token: i32,
    pub name: &'a str,
    pub args: Vec<(&'a str, &'a str)>,
}

impl<'a> Line<'a> {
    /// First value for `key` — `find_parameter()` in command.c (used by `check-support`).
    pub fn first(&self, key: &str) -> Option<&'a str> {
        self.args.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    /// Last value for `key` — `send-probe` decodes arguments in order, so the last one wins.
    pub fn last(&self, key: &str) -> Option<&'a str> {
        self.args
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
    }
}

/// C-locale `isspace()`: space, \t, \n, \v, \f, \r (cmdparse.c:42-59).
fn is_c_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r')
}

/// `strtol(s, NULL, 10)` narrowed to `int` (cmdparse.c:94-99): optional sign, leading
/// digits, trailing garbage ignored, no digits => 0, out of `long` range => error.
/// Parses the signed literal directly to handle `LONG_MIN` correctly.
pub fn parse_token(s: &str) -> Result<i32, ParseError> {
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'-') => ("-", &s[1..]),
        Some(b'+') => ("", &s[1..]),
        _ => ("", s),
    };
    let end = rest.bytes().take_while(u8::is_ascii_digit).count();
    let digits = &rest[..end];
    if digits.is_empty() {
        return Ok(0);
    }
    let value_str = format!("{}{}", sign, digits);
    let value: i64 = value_str.parse().map_err(|_| ParseError::TokenOverflow)?;
    Ok(value as i32) // C assigns the long to an int: wrapping narrowing
}

/// Split one line into token, command name and key/value pairs.
pub fn tokenize(line: &str) -> Result<Line<'_>, ParseError> {
    let toks: Vec<&str> = line.split(is_c_space).filter(|t| !t.is_empty()).collect();
    if toks.len() < 2 {
        return Err(ParseError::TooShort);
    }
    if toks.len() > MAX_ARGUMENTS * 2 + 2 {
        return Err(ParseError::TooManyArguments);
    }
    if !(toks.len() - 2).is_multiple_of(2) {
        return Err(ParseError::DanglingKey);
    }
    let token = parse_token(toks[0])?;
    let args = toks[2..].chunks(2).map(|kv| (kv[0], kv[1])).collect();
    Ok(Line {
        token,
        name: toks[1],
        args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ParseError;

    #[test]
    fn splits_token_name_and_pairs() {
        let l = tokenize("5 send-probe ip-4 1.2.3.4 ttl 3").unwrap();
        assert_eq!(l.token, 5);
        assert_eq!(l.name, "send-probe");
        assert_eq!(l.args, vec![("ip-4", "1.2.3.4"), ("ttl", "3")]);
    }

    #[test]
    fn accepts_any_c_isspace_separator_including_cr() {
        let l = tokenize("7\tno-reply \r\n").unwrap();
        assert_eq!((l.token, l.name, l.args.len()), (7, "no-reply", 0));
        let l = tokenize("  8 \x0b\x0creply  ip-4   10.0.0.1  ").unwrap();
        assert_eq!(l.args, vec![("ip-4", "10.0.0.1")]);
    }

    #[test]
    fn rejects_short_odd_and_oversized_lines() {
        assert_eq!(tokenize("").unwrap_err(), ParseError::TooShort);
        assert_eq!(tokenize("   ").unwrap_err(), ParseError::TooShort);
        assert_eq!(tokenize("5").unwrap_err(), ParseError::TooShort);
        assert_eq!(
            tokenize("5 send-probe ttl").unwrap_err(),
            ParseError::DanglingKey
        );
        let mut big = String::from("5 send-probe");
        for i in 0..17 {
            big.push_str(&format!(" k{i} v{i}"));
        }
        assert_eq!(tokenize(&big).unwrap_err(), ParseError::TooManyArguments);
        big.truncate(big.len() - " k16 v16".len());
        assert_eq!(tokenize(&big).unwrap().args.len(), 16);
    }

    #[test]
    fn first_and_last_lookup() {
        let l = tokenize("1 check-support feature udp feature tcp").unwrap();
        assert_eq!(l.first("feature"), Some("udp"));
        assert_eq!(l.last("feature"), Some("tcp"));
        assert_eq!(l.first("nope"), None);
    }

    #[test]
    fn token_parses_like_strtol_base_10_narrowed_to_int() {
        assert_eq!(parse_token("42").unwrap(), 42);
        assert_eq!(parse_token("-3").unwrap(), -3);
        assert_eq!(parse_token("+9").unwrap(), 9);
        assert_eq!(parse_token("12abc").unwrap(), 12);
        assert_eq!(parse_token("abc").unwrap(), 0);
        assert_eq!(parse_token("4294967297").unwrap(), 1); // long -> int narrowing wraps
        assert_eq!(
            parse_token("99999999999999999999").unwrap_err(),
            ParseError::TokenOverflow
        );
    }

    #[test]
    fn token_handles_long_min_boundary() {
        // LONG_MIN narrows to int 0 (i64::MIN as i32 = 0)
        assert_eq!(parse_token("-9223372036854775808").unwrap(), 0);
        // Positive overflow
        assert_eq!(
            parse_token("9223372036854775808").unwrap_err(),
            ParseError::TokenOverflow
        );
        // Negative overflow
        assert_eq!(
            parse_token("-9223372036854775809").unwrap_err(),
            ParseError::TokenOverflow
        );
    }

    #[test]
    fn token_sign_only_returns_zero() {
        // sign only, no digits → 0 like strtol
        assert_eq!(parse_token("-").unwrap(), 0);
        assert_eq!(parse_token("+").unwrap(), 0);
    }
}
