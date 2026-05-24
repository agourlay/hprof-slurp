//! Content preview for primitive arrays (v0.9.0 feature B).
//!
//! Given a byte slice and the `FieldType` of the originating array,
//! `render_preview` decides whether the content reads as text
//! (UTF-8 / UTF-16 BE) or binary, and renders accordingly. Used by the
//! `--preview-bytes` integration in `summary`, `--paths-from-id`,
//! `--find-referrers id:N`, and the extended `-l` mode.

use crate::parser::gc_record::FieldType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentLabel {
    Json,
    Xml,
    Utf8Text,
    Utf16Text,
    PngImage,
    JpegImage,
    GifImage,
    WebpImage,
    Gzip,
    Zip,
    ProtobufLike,
    RepeatedFill,
    UnknownBinary,
}

impl ContentLabel {
    pub const fn display(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Xml => "XML",
            Self::Utf8Text => "UTF-8 text",
            Self::Utf16Text => "UTF-16 text",
            Self::PngImage => "PNG image",
            Self::JpegImage => "JPEG image",
            Self::GifImage => "GIF image",
            Self::WebpImage => "WebP image",
            Self::Gzip => "gzip compressed",
            Self::Zip => "ZIP archive",
            Self::ProtobufLike => "protobuf-like binary",
            Self::RepeatedFill => "binary/repeated-fill",
            Self::UnknownBinary => "unknown binary",
        }
    }
}

/// Result of a preview render. The two arms are formatted differently
/// by callers (text gets indented as a quote; hex gets a code block).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewKind {
    /// Decoded text snippet, control chars escaped, possibly truncated.
    Text {
        label: ContentLabel,
        snippet: String,
        truncated: bool,
    },
    /// Hexdump-style block, one line per 16 bytes.
    Hex {
        label: ContentLabel,
        lines: Vec<String>,
        total_bytes: usize,
    },
}

impl PreviewKind {
    pub const fn content_label(&self) -> ContentLabel {
        match self {
            Self::Text { label, .. } | Self::Hex { label, .. } => *label,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPreview {
    pub header: String,
    pub first_line: String,
    pub extra_lines: Vec<String>,
}

pub fn render_short_preview(kind: &PreviewKind, text_limit: usize) -> RenderedPreview {
    match kind {
        PreviewKind::Text {
            label,
            snippet,
            truncated,
        } => {
            let trimmed: String = snippet.chars().take(text_limit).collect();
            let suffix = if *truncated || snippet.chars().count() > text_limit {
                "..."
            } else {
                ""
            };
            RenderedPreview {
                header: format!("content: {}", label.display()),
                first_line: format!("{trimmed}{suffix}"),
                extra_lines: Vec::new(),
            }
        }
        PreviewKind::Hex {
            label,
            lines,
            total_bytes,
        } => {
            let mut iter = lines.iter().take(2);
            RenderedPreview {
                header: format!("content: {}, {total_bytes} bytes total", label.display()),
                first_line: iter.next().cloned().unwrap_or_default(),
                extra_lines: iter.cloned().collect(),
            }
        }
    }
}

/// Render `bytes` as a preview. `element_type` selects the decoder:
/// * `Char`   → UTF-16 BE (Java string contents)
/// * `Byte`   → try UTF-8; fall back to hex
/// * everything else → hex
///
/// `total_size_bytes` is the *full* size of the array (not just the
/// truncated `bytes`); used for the "showing first N of M" header in
/// the hex render.
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
        let label = classify_text(&decoded, ContentLabel::Utf16Text);
        PreviewKind::Text {
            label,
            snippet: escape_for_preview(&decoded),
            truncated: bytes.len() < total_size_bytes,
        }
    } else {
        render_hex(bytes, total_size_bytes)
    }
}

fn render_byte_array(bytes: &[u8], total_size_bytes: usize) -> PreviewKind {
    let binary_label = classify_binary(bytes);
    if matches!(
        binary_label,
        ContentLabel::PngImage
            | ContentLabel::JpegImage
            | ContentLabel::GifImage
            | ContentLabel::WebpImage
            | ContentLabel::Gzip
            | ContentLabel::Zip
    ) {
        return render_hex(bytes, total_size_bytes);
    }

    match std::str::from_utf8(bytes) {
        Ok(s) if is_text_like(s) => {
            let label = classify_text(s, ContentLabel::Utf8Text);
            PreviewKind::Text {
                label,
                snippet: escape_for_preview(s),
                truncated: bytes.len() < total_size_bytes,
            }
        }
        _ => render_hex(bytes, total_size_bytes),
    }
}

fn render_hex(bytes: &[u8], total_size_bytes: usize) -> PreviewKind {
    let label = classify_binary(bytes);
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
        label,
        lines,
        total_bytes: total_size_bytes,
    }
}

fn classify_text(s: &str, fallback: ContentLabel) -> ContentLabel {
    let trimmed = s.trim_start();
    if looks_like_json(trimmed) {
        ContentLabel::Json
    } else if looks_like_xml(trimmed) {
        ContentLabel::Xml
    } else {
        fallback
    }
}

fn looks_like_json(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    match bytes[0] {
        b'{' => bytes[1..].iter().any(|b| matches!(b, b'"' | b'}')),
        b'[' => bytes[1..].iter().any(|b| matches!(b, b'{' | b'[' | b'"' | b']')),
        _ => false,
    }
}

fn looks_like_xml(s: &str) -> bool {
    if s.starts_with("<?xml") {
        return true;
    }
    let Some(rest) = s.strip_prefix('<') else {
        return false;
    };
    matches!(
        rest.as_bytes().first(),
        Some(b'a'..=b'z' | b'A'..=b'Z' | b'_' | b':')
    )
}

fn classify_binary(bytes: &[u8]) -> ContentLabel {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        ContentLabel::PngImage
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        ContentLabel::JpegImage
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        ContentLabel::GifImage
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        ContentLabel::WebpImage
    } else if bytes.starts_with(&[0x1f, 0x8b]) {
        ContentLabel::Gzip
    } else if bytes.starts_with(&[0x50, 0x4b, 0x03, 0x04])
        || bytes.starts_with(&[0x50, 0x4b, 0x05, 0x06])
        || bytes.starts_with(&[0x50, 0x4b, 0x07, 0x08])
    {
        ContentLabel::Zip
    } else if looks_repeated(bytes) {
        ContentLabel::RepeatedFill
    } else if looks_protobuf_like(bytes) {
        ContentLabel::ProtobufLike
    } else {
        ContentLabel::UnknownBinary
    }
}

fn looks_repeated(bytes: &[u8]) -> bool {
    bytes.len() >= 16
        && (bytes.windows(2).all(|w| w[0] == w[1])
            || bytes
                .chunks_exact(2)
                .map(|c| [c[0], c[1]])
                .all(|p| p == [bytes[0], bytes[1]]))
}

fn looks_protobuf_like(bytes: &[u8]) -> bool {
    if bytes.len() < 8 {
        return false;
    }
    let plausible_tags = bytes
        .iter()
        .filter(|&&b| b != 0 && b < 0x80 && (b & 0x07) <= 5 && (b >> 3) > 0)
        .count();
    plausible_tags * 4 >= bytes.len()
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
            PreviewKind::Text {
                snippet, truncated, ..
            } => {
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
            PreviewKind::Hex {
                lines, total_bytes, ..
            } => {
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

    #[test]
    fn classifies_json_text() {
        let bytes = br#"{"items":[1,2],"ok":true}"#;
        let p = render_preview(bytes, FieldType::Byte, bytes.len());
        assert_eq!(p.content_label(), ContentLabel::Json);
    }

    #[test]
    fn classifies_xml_text() {
        let bytes = b"<?xml version=\"1.0\"?><root/>";
        let p = render_preview(bytes, FieldType::Byte, bytes.len());
        assert_eq!(p.content_label(), ContentLabel::Xml);
    }

    #[test]
    fn classifies_utf8_text() {
        let bytes = b"plain readable text";
        let p = render_preview(bytes, FieldType::Byte, bytes.len());
        assert_eq!(p.content_label(), ContentLabel::Utf8Text);
    }

    #[test]
    fn classifies_utf16_text() {
        let bytes = [0x00, 0x48, 0x00, 0x69];
        let p = render_preview(&bytes, FieldType::Char, bytes.len());
        assert_eq!(p.content_label(), ContentLabel::Utf16Text);
    }

    #[test]
    fn classifies_png_signature() {
        let bytes = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        let p = render_preview(&bytes, FieldType::Byte, bytes.len());
        assert_eq!(p.content_label(), ContentLabel::PngImage);
    }

    #[test]
    fn classifies_jpeg_signature() {
        let bytes = [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10];
        let p = render_preview(&bytes, FieldType::Byte, bytes.len());
        assert_eq!(p.content_label(), ContentLabel::JpegImage);
    }

    #[test]
    fn classifies_gif_signature() {
        let p = render_preview(b"GIF89a", FieldType::Byte, 6);
        assert_eq!(p.content_label(), ContentLabel::GifImage);
    }

    #[test]
    fn classifies_webp_signature() {
        let p = render_preview(b"RIFF\x24\x00\x00\x00WEBP", FieldType::Byte, 12);
        assert_eq!(p.content_label(), ContentLabel::WebpImage);
    }

    #[test]
    fn classifies_gzip_signature() {
        let bytes = [0x1f, 0x8b, 0x08, 0x00];
        let p = render_preview(&bytes, FieldType::Byte, bytes.len());
        assert_eq!(p.content_label(), ContentLabel::Gzip);
    }

    #[test]
    fn classifies_zip_signature() {
        let bytes = [0x50, 0x4b, 0x03, 0x04];
        let p = render_preview(&bytes, FieldType::Byte, bytes.len());
        assert_eq!(p.content_label(), ContentLabel::Zip);
    }

    #[test]
    fn classifies_repeated_fill_binary() {
        let bytes = [0xaa; 32];
        let p = render_preview(&bytes, FieldType::Byte, bytes.len());
        assert_eq!(p.content_label(), ContentLabel::RepeatedFill);
    }

    #[test]
    fn classifies_unknown_binary_when_no_signature_matches() {
        let bytes = [0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87];
        let p = render_preview(&bytes, FieldType::Byte, bytes.len());
        assert_eq!(p.content_label(), ContentLabel::UnknownBinary);
    }

    #[test]
    fn short_text_preview_includes_content_label() {
        let preview = render_preview(br#"{"ok":true}"#, FieldType::Byte, 11);
        let rendered = render_short_preview(&preview, 80);
        assert_eq!(rendered.header, "content: JSON");
        assert_eq!(rendered.first_line, r#"{"ok":true}"#);
        assert!(rendered.extra_lines.is_empty());
    }

    #[test]
    fn short_binary_preview_includes_content_label_and_hex_lines() {
        let bytes = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        let preview = render_preview(&bytes, FieldType::Byte, bytes.len());
        let rendered = render_short_preview(&preview, 80);
        assert_eq!(rendered.header, "content: PNG image, 8 bytes total");
        assert!(rendered.first_line.contains("89 50 4e 47"));
        assert!(rendered.extra_lines.is_empty());
    }
}
