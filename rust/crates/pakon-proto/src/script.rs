//! Capture scripts (`.pakscan`, `.pakfw`) as produced by the capture tools.
//!
//! ```text
//! # comment
//! O <hex>                              EP1 command out (+ read reply)
//! M <n>                                read n image bytes from EP 0x86
//! C <bmReqType> <bReq> <wValue> <wIndex> <wLen> [data]   control transfer
//! ```
//!
//! Used by the tests as the ground truth generated sequences must match, and
//! later by the CLI to replay recordings. Parsing is lenient where the C
//! readers are: an `O` line's hex ends at the first non-hex character.

use std::fmt;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// Raw frame bytes sent on EP1 OUT.
    Out(Vec<u8>),
    /// Image bytes read from EP 0x86.
    Image(usize),
    /// Control transfer, fields kept as text (`bmReqType bReq wValue ...`).
    Control(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "script line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for ScriptError {}

fn hex_prefix(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let nib = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        match (nib(b[i]), nib(b[i + 1])) {
            (Some(h), Some(l)) => out.push(h << 4 | l),
            _ => break,
        }
        i += 2;
    }
    out
}

/// Parse a script's text. Blank lines and `#` comments are skipped.
pub fn parse(text: &str) -> Result<Vec<Line>, ScriptError> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let err = |msg: String| ScriptError { line: i + 1, msg };
        let line = raw.trim_end();
        let Some(kind) = line.chars().next() else { continue };
        let rest = line.get(2..).unwrap_or("").trim_start();
        match kind {
            '#' => {}
            'O' => out.push(Line::Out(hex_prefix(rest))),
            'M' => {
                let n = rest.split_whitespace().next().unwrap_or("");
                out.push(Line::Image(n.parse().map_err(|_| err(format!("bad image length {n:?}")))?));
            }
            'C' => out.push(Line::Control(rest.to_string())),
            _ => return Err(err(format!("unknown line {line:?}"))),
        }
    }
    Ok(out)
}

pub fn parse_file(path: impl AsRef<Path>) -> Result<Vec<Line>, ScriptError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)
        .map_err(|e| ScriptError { line: 0, msg: format!("{}: {e}", path.display()) })?;
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        let s = "# x\nO 0403100085\nM 20480\nC 40 a4 00a5 1234 0000\n\nO 030124 trailing\n";
        assert_eq!(
            parse(s).unwrap(),
            vec![
                Line::Out(vec![4, 3, 0x10, 0, 0x85]),
                Line::Image(20480),
                Line::Control("40 a4 00a5 1234 0000".into()),
                Line::Out(vec![3, 1, 0x24]),
            ]
        );
        assert!(parse("X 1\n").is_err());
        assert!(parse("M x\n").is_err());
    }
}
