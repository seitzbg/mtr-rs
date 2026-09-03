//! `MTR_OPTIONS` splitting (ui/mtr.c:923-988) and `-F` file reading (ui/mtr.c:236-274) —
//! mtr 0.96, commit 7b01773. GPL-2.0-only.

use std::io::Read as _;

/// C reserves 128 argv slots for the environment options.
pub const MAX_ENV_ARGS: usize = 128;

/// Split `$MTR_OPTIONS` into words: whitespace separated, `'…'`/`"…"` quoting, `\x` escapes.
pub fn split_mtr_options(input: &str) -> Result<Vec<String>, String> {
    let is_space = |c: char| c.is_ascii_whitespace() || c == '\u{0B}';
    let mut words = Vec::new();
    let mut chars = input.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| is_space(*c)) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        if words.len() >= MAX_ENV_ARGS {
            eprintln!(
                "mtr: Warning: extra arguments ignored: {}",
                chars.collect::<String>()
            );
            break;
        }
        let mut word = String::new();
        let mut quote: Option<char> = None;
        loop {
            match chars.next() {
                None => {
                    if quote.is_some() {
                        return Err("unterminated quote in MTR_OPTIONS".to_string());
                    }
                    break;
                }
                Some(c) if quote == Some(c) => quote = None,
                Some(c) if quote.is_none() && (c == '\'' || c == '"') => quote = Some(c),
                Some('\\') => match chars.next() {
                    Some(n) => word.push(n),
                    None => {
                        if quote.is_some() {
                            return Err("unterminated quote in MTR_OPTIONS".to_string());
                        }
                        break;
                    }
                },
                Some(c) if quote.is_none() && is_space(c) => break,
                Some(c) => word.push(c),
            }
        }
        words.push(word);
    }
    Ok(words)
}

/// `read_from_file()`: one host per line, trimmed; `-` reads stdin. Blank lines skipped (deviation 4).
pub fn read_hosts_file(path: &str) -> Result<Vec<String>, String> {
    let text = if path == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| format!("open {path}: {e}"))?;
        s
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("open {path}: {e}"))?
    };
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words_quotes_and_escapes() {
        assert_eq!(split_mtr_options("-r  -c 5").unwrap(), ["-r", "-c", "5"]);
        assert_eq!(
            split_mtr_options("-o \"LS NABWV\" -y '0,1'").unwrap(),
            ["-o", "LS NABWV", "-y", "0,1"]
        );
        assert_eq!(split_mtr_options(r"a\ b 'c\'d'").unwrap(), ["a b", "c'd"]);
        assert!(split_mtr_options("").unwrap().is_empty());
        assert!(split_mtr_options("   \t ").unwrap().is_empty());
        assert_eq!(
            split_mtr_options("-o 'unterminated").unwrap_err(),
            "unterminated quote in MTR_OPTIONS"
        );
    }

    #[test]
    fn reads_hosts_file_skipping_blank_lines() {
        let path = std::env::temp_dir().join(format!("mtr-rs-hosts-{}", std::process::id()));
        std::fs::write(&path, " a.example \n\n\tb.example\n").unwrap();
        assert_eq!(
            read_hosts_file(path.to_str().unwrap()).unwrap(),
            ["a.example", "b.example"]
        );
        std::fs::remove_file(&path).unwrap();
        assert!(
            read_hosts_file("/nonexistent/hosts")
                .unwrap_err()
                .starts_with("open /nonexistent/hosts")
        );
    }
}
