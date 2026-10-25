//! Reading paths out of pasted text, in every shape a Mac hands them over:
//! one per line, Terminal's backslash-escaped paths, quoted paths, `~/`,
//! and `file://` URLs.

use std::path::PathBuf;

/// Every path in `text`.
pub fn paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .flat_map(|line| {
            // A whole line that names something is one path, spaces and all.
            let whole = expand(line);
            if whole.exists() {
                vec![whole]
            } else {
                words(line).iter().map(|word| expand(word)).collect()
            }
        })
        .collect()
}

/// Split a line the way a shell would: spaces separate, quotes and
/// backslashes keep them. Dragging files into Terminal gives this.
fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (_, c) => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

/// `~/` to the home folder, `file://` URLs to paths.
fn expand(word: &str) -> PathBuf {
    if let Some(rest) = word.strip_prefix("file://") {
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        return PathBuf::from(percent_decode(rest));
    }
    if let Some(rest) = word.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(word)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_of_pasted_path() {
        let text = "\
/Volumes/Drone/DJI_0001.MP4
/Users/j/My\\ Movies/a.mov /Users/j/b.mov
'/Users/j/Quoted Name.mov'
file:///Users/j/Space%20Name.mov

";
        assert_eq!(
            paths(text),
            [
                "/Volumes/Drone/DJI_0001.MP4",
                "/Users/j/My Movies/a.mov",
                "/Users/j/b.mov",
                "/Users/j/Quoted Name.mov",
                "/Users/j/Space Name.mov",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn a_line_that_exists_keeps_its_spaces() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let path = temp.path().join("two words.mov");
        std::fs::write(&path, "x").expect("write");
        assert_eq!(paths(&path.to_string_lossy()), [path]);
    }

    #[test]
    fn home_is_expanded() {
        let home = std::env::var_os("HOME").expect("HOME");
        assert_eq!(paths("~/x.mov"), [PathBuf::from(home).join("x.mov")]);
    }
}
