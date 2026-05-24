use serde::Serialize;

#[derive(Serialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_heap_kb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_heap_kb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphics_kb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gl_kb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_pss_kb: Option<u64>,
    pub warnings: Vec<String>,
}

pub fn parse_meminfo(text: &str) -> NativeContext {
    let mut ctx = NativeContext::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(value) = parse_row(trimmed, "Java Heap:") {
            ctx.java_heap_kb = Some(value);
        } else if let Some(value) = parse_row(trimmed, "Native Heap:") {
            ctx.native_heap_kb = Some(value);
        } else if let Some(value) = parse_row(trimmed, "Graphics:") {
            ctx.graphics_kb = Some(value);
        } else if let Some(value) = parse_row(trimmed, "GL:") {
            ctx.gl_kb = Some(value);
        } else if let Some(value) = parse_row(trimmed, "TOTAL:") {
            ctx.total_pss_kb = Some(value);
        }
    }
    for (name, value) in [
        ("Java Heap", ctx.java_heap_kb),
        ("Native Heap", ctx.native_heap_kb),
        ("Graphics", ctx.graphics_kb),
        ("GL", ctx.gl_kb),
        ("TOTAL", ctx.total_pss_kb),
    ] {
        if value.is_none() {
            ctx.warnings.push(format!("missing meminfo row: {name}"));
        }
    }
    ctx
}

fn parse_row(line: &str, label: &str) -> Option<u64> {
    let rest = line.strip_prefix(label)?.trim();
    rest.split_whitespace().next()?.parse().ok()
}

pub fn render_text(ctx: &NativeContext) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let _ = writeln!(out, "\nNative context (dumpsys meminfo, PSS KiB):");
    if let Some(v) = ctx.java_heap_kb {
        let _ = writeln!(out, "  Java Heap:   {v} KiB");
    }
    if let Some(v) = ctx.native_heap_kb {
        let _ = writeln!(out, "  Native Heap: {v} KiB");
    }
    if let Some(v) = ctx.graphics_kb {
        let _ = writeln!(out, "  Graphics:    {v} KiB");
    }
    if let Some(v) = ctx.gl_kb {
        let _ = writeln!(out, "  GL:          {v} KiB");
    }
    if let Some(v) = ctx.total_pss_kb {
        let _ = writeln!(out, "  TOTAL:       {v} KiB");
    }
    for warning in &ctx.warnings {
        let _ = writeln!(out, "  warning: {warning}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_core_meminfo_rows() {
        let text = r#"
                         Pss  Private  Private  SwapPss
                       Total    Dirty    Clean    Dirty
      Java Heap:       12345    10000        0        0
    Native Heap:       23456    20000        0        0
       Graphics:        3456     3000        0        0
             GL:        4567     4000        0        0
          TOTAL:       99999    90000        0        0
"#;

        let ctx = parse_meminfo(text);

        assert_eq!(ctx.java_heap_kb, Some(12345));
        assert_eq!(ctx.native_heap_kb, Some(23456));
        assert_eq!(ctx.graphics_kb, Some(3456));
        assert_eq!(ctx.gl_kb, Some(4567));
        assert_eq!(ctx.total_pss_kb, Some(99999));
        assert!(ctx.warnings.is_empty());
    }
}
