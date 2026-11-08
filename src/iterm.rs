//! iTerm2 inline images (`OSC 1337;File=...`), used by `rterm play` and other tools.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ItermFile {
    pub args: HashMap<String, String>,
    /// Raw file bytes (png/jpeg/gif...).
    pub data: Vec<u8>,
    /// Hint for width: pixels, or `Cell(u32)`.
    pub width: Option<SizeHint>,
    pub height: Option<SizeHint>,
    pub inline: bool,
    pub do_not_move_cursor: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SizeHint {
    Px(u32),
    Cells(u32),
}

/// Parse content after `1337;`, i.e. `File=name=x;size=123;QUJD`.
pub fn parse(content: &str) -> Option<ItermFile> {
    let rest = content.strip_prefix("File=")?;
    let colon = rest.find(':')?;
    let (args_str, b64) = (&rest[..colon], &rest[colon + 1..]);
    let mut args = HashMap::new();
    for part in split_args(args_str) {
        if let Some(eq) = part.find('=') {
            args.insert(part[..eq].to_string(), part[eq + 1..].to_string());
        }
    }
    let data =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.trim()).ok()?;
    Some(ItermFile {
        width: args.get("width").and_then(|v| parse_size(v)),
        height: args.get("height").and_then(|v| parse_size(v)),
        inline: matches!(args.get("inline").map(String::as_str), Some("1")),
        do_not_move_cursor: args.get("doNotMoveCursor").map(String::as_str) == Some("1"),
        args,
        data,
    })
}

fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in s.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ';' if !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_size(v: &str) -> Option<SizeHint> {
    let v = v.trim();
    if let Some(n) = v.strip_suffix("cells") {
        return n.trim().parse().ok().map(SizeHint::Cells);
    }
    if let Some(n) = v.strip_suffix("px") {
        return n.trim().parse().ok().map(SizeHint::Px);
    }
    if let Some(n) = v.strip_suffix('%') {
        // Percent of the screen is resolved by the caller using cell size — treat as px hint.
        return n.trim().parse().ok().map(SizeHint::Px);
    }
    v.parse().ok().map(SizeHint::Px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn parses_file_with_args() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let content =
            format!("File=name=\"my pic.png\";inline=1;width=12 cells;doNotMoveCursor=1:{b64}");
        let f = parse(&content).unwrap();
        assert_eq!(f.args.get("name").unwrap(), "my pic.png");
        assert!(f.inline);
        assert!(f.do_not_move_cursor);
        assert_eq!(f.width, Some(SizeHint::Cells(12)));
        assert_eq!(f.data, b"hello");
    }

    #[test]
    fn rejects_non_file() {
        assert!(parse("Invalid=1;AAAA").is_none());
        assert!(parse("File=name=x;!!!notbase64!!!").is_none());
    }

    #[test]
    fn pixel_width() {
        let f = parse("File=width=64;height=32:AAAA").unwrap();
        assert_eq!(f.width, Some(SizeHint::Px(64)));
        assert_eq!(f.height, Some(SizeHint::Px(32)));
    }
}
