//! Content preview for primitive arrays (v0.9.0 feature B).
//!
//! Given a byte slice and the `FieldType` of the originating array,
//! `render_preview` decides whether the content reads as text
//! (UTF-8 / UTF-16 BE) or binary, and renders accordingly. Used by the
//! `--preview-bytes` integration in `summary`, `--paths-from-id`,
//! `--find-referrers id:N`, and the extended `-l` mode.

use crate::parser::gc_record::FieldType;

/// Result of a preview render. The two arms are formatted differently
/// by callers (text gets indented as a quote; hex gets a code block).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewKind {
    /// Decoded text snippet, control chars escaped, possibly truncated.
    Text { snippet: String, truncated: bool },
    /// Hexdump-style block, one line per 16 bytes.
    Hex {
        lines: Vec<String>,
        total_bytes: usize,
    },
}

/// Render `bytes` as a preview. `element_type` selects the decoder:
/// * `Char`   → UTF-16 BE (Java string contents)
/// * `Byte`   → try UTF-8; fall back to hex
/// * everything else → hex
///
/// `total_size_bytes` is the *full* size of the array (not just the
/// truncated `bytes`); used for the "showing first N of M" header in
/// the hex render.
#[allow(dead_code)] // bridging — consumed in PR 3 (summary preview integration)
pub fn render_preview(
    bytes: &[u8],
    element_type: FieldType,
    total_size_bytes: usize,
) -> PreviewKind {
    match element_type {
        FieldType::Char => render_utf16_be(bytes, total_size_bytes),
        FieldType::Byte => render_byte_array(bytes, total_size_bytes),
        _ => render_hex(bytes, total_size_bytes),
    }
}

fn render_utf16_be(bytes: &[u8], total_size_bytes: usize) -> PreviewKind {
    // Decode as UTF-16 BE. If we cut mid-surrogate at the truncation
    // boundary, drop the trailing odd byte.
    let usable_len = bytes.len() - (bytes.len() % 2);
    let mut chars = Vec::with_capacity(usable_len / 2);
    for pair in bytes[..usable_len].chunks_exact(2) {
        chars.push(u16::from_be_bytes([pair[0], pair[1]]));
    }
    let decoded = String::from_utf16_lossy(&chars);
    if is_text_like(&decoded) {
        PreviewKind::Text {
            snippet: escape_for_preview(&decoded),
            truncated: bytes.len() < total_size_bytes,
        }
    } else {
        render_hex(bytes, total_size_bytes)
    }
}

fn render_byte_array(bytes: &[u8], total_size_bytes: usize) -> PreviewKind {
    match std::str::from_utf8(bytes) {
        Ok(s) if is_text_like(s) => PreviewKind::Text {
            snippet: escape_for_preview(s),
            truncated: bytes.len() < total_size_bytes,
        },
        _ => render_hex(bytes, total_size_bytes),
    }
}

fn render_hex(bytes: &[u8], total_size_bytes: usize) -> PreviewKind {
    let mut lines: Vec<String> = Vec::new();
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let offset = i * 16;
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let mut hex_part = String::new();
        for (j, h) in hex.iter().enumerate() {
            if j == 8 {
                hex_part.push(' ');
            }
            hex_part.push_str(h);
            hex_part.push(' ');
        }
        // pad right column for short final lines
        let pad = (16 - chunk.len()) * 3 + if chunk.len() <= 8 { 1 } else { 0 };
        let ascii: String = chunk
            .iter()
            .map(|&b| {
                if (0x20..0x7f).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        lines.push(format!(
            "{offset:08x}  {hex_part}{}|{ascii}|",
            " ".repeat(pad)
        ));
    }
    PreviewKind::Hex {
        lines,
        total_bytes: total_size_bytes,
    }
}

/// Heuristic: ≥90% of chars are printable ASCII, common whitespace, or
/// printable Unicode. Replacement char (U+FFFD from lossy decoding) is
/// counted as non-printable.
fn is_text_like(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let total = s.chars().count();
    let printable = s
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || c == '\r' || (c >= ' ' && c != '\u{fffd}'))
        .count();
    printable * 10 >= total * 9
}

/// Replace control chars (other than \n, \t, \r) with `\xNN` escapes.
/// Visible newlines/tabs/CRs are kept as the literal escape `\n` /
/// `\t` / `\r` to keep the preview on one logical block. Callers
/// indent each rendered line with whatever spacing they prefer.
fn escape_for_preview(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                let code = c as u32;
                if code <= 0xff {
                    out.push_str(&format!("\\x{code:02x}"));
                } else {
                    out.push_str(&format!("\\u{{{code:04x}}}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_text_is_detected_and_escaped() {
        let bytes = b"<?xml version=\"1.0\"?>\n<root>";
        let p = render_preview(bytes, FieldType::Byte, bytes.len());
        match p {
            PreviewKind::Text { snippet, truncated } => {
                assert!(snippet.contains("<?xml"), "got: {snippet}");
                assert!(
                    snippet.contains("\\n"),
                    "newline should be escaped: {snippet}"
                );
                assert!(!truncated);
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn binary_bytes_use_hex_path() {
        let bytes = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]; // PNG header
        let p = render_preview(&bytes, FieldType::Byte, bytes.len());
        match p {
            PreviewKind::Hex { lines, total_bytes } => {
                assert_eq!(total_bytes, 8);
                assert_eq!(lines.len(), 1);
                assert!(lines[0].contains("89 50 4e 47"), "got: {}", lines[0]);
                assert!(
                    lines[0].contains("|.PNG"),
                    "ascii column missing: {}",
                    lines[0]
                );
            }
            other => panic!("expected Hex, got {other:?}"),
        }
    }

    #[test]
    fn utf16_be_text_decodes_correctly() {
        // "Hi" in UTF-16 BE = 00 48 00 69
        let bytes = [0x00, 0x48, 0x00, 0x69];
        let p = render_preview(&bytes, FieldType::Char, bytes.len());
        match p {
            PreviewKind::Text { snippet, .. } => {
                assert_eq!(snippet, "Hi");
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn truncation_flag_set_when_bytes_shorter_than_total() {
        let p = render_preview(b"hello", FieldType::Byte, 1024);
        match p {
            PreviewKind::Text { truncated, .. } => assert!(truncated),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn int_array_always_hex() {
        // Bytes that happen to look ASCII-printable still go to hex —
        // int[] values aren't text-shaped semantically.
        let bytes = [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48];
        let p = render_preview(&bytes, FieldType::Int, bytes.len());
        assert!(matches!(p, PreviewKind::Hex { .. }));
    }

    #[test]
    fn odd_byte_count_truncates_safely_for_utf16() {
        // 5 bytes is invalid UTF-16; should not panic, drops the last byte.
        let bytes = [0x00, 0x48, 0x00, 0x69, 0xff];
        let _ = render_preview(&bytes, FieldType::Char, bytes.len());
    }
}
