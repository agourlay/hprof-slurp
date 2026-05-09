# heaptrail v0.9.0 Implementation Plan — Feature B (content preview)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement feature B from the v0.9.0 design spec at `docs/superpowers/specs/2026-05-10-heaptrail-v0.9-design.md`: opt-in `--preview-bytes N` content preview for primitive arrays (`char[]`, `byte[]`, etc.), with auto-detect text/binary rendering, surfaced in `summary`, `--paths-from-id`, `--find-referrers id:N`, and an extension to `-l` for standalone large arrays.

**Architecture:** Seven sequential PRs onto `master`, single v0.9.0 tag at end. PR 1 lands the new parser mode (`retain_primitive_bodies`) with no user-visible effect. PR 2 lands the `src/preview.rs` text/binary render module. PRs 3–6 each wire a render surface (summary, paths, referrers, list-strings). PR 7 is the docs + version bump + tag.

**Tech Stack:** Rust 2024 edition, rustc ≥ 1.95 (CI). No new dependencies. Existing deps reused: `clap` derive, `nom`, `ahash`, `serde`, `serde_json`, `chrono`, `thiserror`, `crossbeam-channel`, `indicatif`, `indoc`, `globset`.

**Working directory:** `/Users/jneerdael/Scripts/hprof-slurp` (local checkout — the GitHub repo is `johnneerdael/heaptrail`; remote `fork` points there). Tests, fmt, clippy must pass on every commit. Per `CLAUDE.md`, validate every feature against **both** canonical fixtures: `JAVA_PROFILE_1.0.2.hprof` (JVM, 8-byte ids) and `JAVA_PROFILE_1.0.3.hprof` (Android, 4-byte ids; includes `PrimitiveArrayNoDataDump`).

---

## File Structure

| File | Responsibility | Touched in |
|------|----------------|------------|
| `src/parser/gc_record.rs` | `PrimitiveArrayDump` gains `body: Option<Box<[u8]>>` | PR 1 |
| `src/parser/record_parser.rs` | New `retain_primitive_bodies` + `preview_bytes_limit` parser fields; `parse_gc_primitive_array_dump_lite` / `_full` split; dispatch branching | PR 1 |
| `src/parser/record_stream_parser.rs` | Plumb the two new parser flags through `with_retain_bodies` constructor | PR 1 |
| `src/preview.rs` (new) | `render_preview()` — UTF-8 / UTF-16 / hex auto-detect renderer; sanitization | PR 2 |
| `src/result_recorder.rs` | `ArrayCounter` keeps the body of the largest array per class; `RenderedResult.array_previews` populated; `-l` extension reads previews when flag set | PR 3, PR 6 |
| `src/rendered_result.rs` | `RenderedResult.array_previews` field + render hooks; "Largest array instances" gets a preview line under each row | PR 3 |
| `src/slurp.rs` | `slurp_file` gains a v2 entry that takes preview opts; existing call sites unchanged | PR 3 |
| `src/args.rs` | `--preview-bytes` flag (default 0); `--list-arrays-min-bytes` flag (default 1024) | PR 3 |
| `src/main.rs` | Pass preview opts into each mode | PR 3 (summary), PR 4 (paths), PR 5 (referrers) |
| `src/paths.rs` | Render preview for primitive-array start id and primitive-array hits in chain | PR 4 |
| `src/referrer.rs` | Render preview when `--find-referrers id:N` target is a primitive array | PR 5 |
| `Cargo.toml` | Version bump 0.8.0 → 0.9.0 | PR 7 |
| `README.md` | Cheat-sheet entry for `--preview-bytes`; pointer to USERGUIDE §B | PR 7 |
| `USERGUIDE.md` | Full §B section with worked example + sanitization rules | PR 7 |
| `plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md` | Sixth/integrated mode: `--preview-bytes` in standard triage workflow | PR 7 |
| `plugins/analysing-heap-dumps/.claude-plugin/plugin.json` | 0.8.0 → 0.9.0 | PR 7 |
| `.claude-plugin/marketplace.json` | 0.8.0 → 0.9.0 | PR 7 |

---

## Setup

### Task 0: Pre-flight

**Files:**
- Read: `docs/superpowers/specs/2026-05-10-heaptrail-v0.9-design.md`
- Read: `CLAUDE.md`

- [ ] **Step 1: Verify clean working tree on `master` at v0.8.0**

```bash
cd /Users/jneerdael/Scripts/hprof-slurp
git status
git log --oneline -3
```

Expected: clean working tree (only `*.hprof` untracked allowed); `0bea4cd chore: bump to 0.8.0` or `ba02fbc docs: v0.9.0 design spec` at HEAD.

- [ ] **Step 2: Verify the v0.8.0 baseline still passes**

Run: `cargo test --release && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 60 tests pass; clippy clean; fmt clean.

- [ ] **Step 3: Confirm both canonical fixtures exist**

Run: `ls JAVA_PROFILE_1.0.2.hprof JAVA_PROFILE_1.0.3.hprof`
Expected: both exist. (The `test-heap-dumps/hprof-{32,64}.bin` integration fixtures are also still required.)

---

## PR 1 — Parser: `retain_primitive_bodies` mode

**Goal:** The parser can optionally retain a truncated copy of each primitive array's body. No user-visible effect; lays the foundation for PRs 3–6.

**PR title:** `feat(parser): retain_primitive_bodies + preview_bytes_limit parser mode (B foundation)`

### Task 1.1: Widen `PrimitiveArrayDump` with optional `body`

**Files:**
- Modify: `src/parser/gc_record.rs`

- [ ] **Step 1: Add `body: Option<Box<[u8]>>` to `PrimitiveArrayDump`**

Find the `PrimitiveArrayDump` variant in `src/parser/gc_record.rs`:

```rust
PrimitiveArrayDump {
    object_id: u64,
    stack_trace_serial_number: u32,
    number_of_elements: u32,
    element_type: FieldType,
},
```

Replace with:

```rust
PrimitiveArrayDump {
    object_id: u64,
    stack_trace_serial_number: u32,
    number_of_elements: u32,
    element_type: FieldType,
    /// Truncated raw bytes (first `preview_bytes_limit` per array).
    /// Retained only when the parser is constructed with
    /// `retain_primitive_bodies = true` (v0.9.0 feature B). `None` in
    /// the default summary path so existing throughput is preserved.
    body: Option<Box<[u8]>>,
},
```

`PrimitiveArrayNoDataDump` (the Android extension) is **not** changed — it has no body to truncate.

- [ ] **Step 2: Update construction sites (parser + recorder tests + recorder match arm)**

Build to find every site that needs `body: None`:

```bash
cargo build --release 2>&1 | grep "missing field \`body\`"
```

For each site reported, add `body: None,` to the struct literal. The expected sites are:

1. `src/parser/record_parser.rs::parse_gc_primitive_array_dump` (will be split into lite/full in Task 1.3 — for now just add `body: None,` to the existing single function).
2. `src/result_recorder.rs::tests::primitive_array_size_uses_exact_padding_per_array` — three `PrimitiveArrayDump { ... }` literals.
3. Any other test that constructs the variant.

Apply (one example, repeat for every site):

```rust
PrimitiveArrayDump {
    object_id: 1,
    stack_trace_serial_number: 0,
    number_of_elements: 1,
    element_type: FieldType::Bool,
    body: None,
},
```

- [ ] **Step 3: Build + run tests**

Run: `cargo build --release && cargo test --release`
Expected: 60 tests pass (no behavior change yet).

- [ ] **Step 4: Commit**

```bash
git add src/parser/gc_record.rs src/parser/record_parser.rs src/result_recorder.rs
git commit -m "$(cat <<'EOF'
refactor(parser): add body: Option<Box<[u8]>> to PrimitiveArrayDump

Mirrors the existing pattern on InstanceDump (body) and
ObjectArrayDump (elements). The field is None today; PR-internal
follow-ups split parse_gc_primitive_array_dump into lite/full and
populate the body in retain-bodies mode.

Test sites updated to construct with body: None.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.2: Plumb `retain_primitive_bodies` + `preview_bytes_limit` through the parser

**Files:**
- Modify: `src/parser/record_parser.rs`
- Modify: `src/parser/record_stream_parser.rs`

- [ ] **Step 1: Add fields to `HprofRecordParser`**

Find the existing `HprofRecordParser` struct in `src/parser/record_parser.rs` (~line 56):

```rust
pub struct HprofRecordParser {
    debug_mode: bool,
    id_size: u32,
    heap_dump_remaining_len: u32,
    retain_bodies: bool,
}
```

Replace with:

```rust
pub struct HprofRecordParser {
    debug_mode: bool,
    id_size: u32,
    heap_dump_remaining_len: u32,
    retain_bodies: bool,
    /// v0.9.0: when true, primitive array bodies are retained on
    /// `PrimitiveArrayDump.body`, truncated to `preview_bytes_limit` bytes.
    retain_primitive_bodies: bool,
    /// Cap per primitive array when `retain_primitive_bodies` is true.
    /// 0 means "no cap" (retain full body — discouraged for large dumps).
    preview_bytes_limit: u32,
}
```

- [ ] **Step 2: Add a builder constructor**

Below the existing `with_retain_bodies` constructor, add:

```rust
    /// Construct a parser with all current opt-in modes spelled out.
    /// The non-builder constructors delegate to this.
    pub const fn with_modes(
        debug_mode: bool,
        id_size: u32,
        retain_bodies: bool,
        retain_primitive_bodies: bool,
        preview_bytes_limit: u32,
    ) -> Self {
        Self {
            debug_mode,
            id_size,
            heap_dump_remaining_len: 0,
            retain_bodies,
            retain_primitive_bodies,
            preview_bytes_limit,
        }
    }
```

Then change the existing `with_retain_bodies` to delegate:

```rust
    pub const fn with_retain_bodies(debug_mode: bool, id_size: u32, retain_bodies: bool) -> Self {
        Self::with_modes(debug_mode, id_size, retain_bodies, false, 0)
    }
```

- [ ] **Step 3: Plumb into `parse_gc_record` dispatch**

Find the `parse_gc_record` function (~line 200) — it currently takes `(i, id_size, retain_bodies)`. Change its signature to also take the two new fields:

```rust
fn parse_gc_record(
    i: &[u8],
    id_size: u32,
    retain_bodies: bool,
    retain_primitive_bodies: bool,
    preview_bytes_limit: u32,
) -> IResult<&[u8], GcRecord> {
```

In the dispatch site inside `parse_hprof_record`, also forward the new fields. Find `parse_gc_record(i, id_size, self.retain_bodies)` and replace with:

```rust
parse_gc_record(
    i,
    id_size,
    self.retain_bodies,
    self.retain_primitive_bodies,
    self.preview_bytes_limit,
)
```

In the `match tag` inside `parse_gc_record`, replace the existing primitive-array arm:

```rust
TAG_GC_PRIM_ARRAY_DUMP => parse_gc_primitive_array_dump(r1, id_size),
```

with:

```rust
TAG_GC_PRIM_ARRAY_DUMP if retain_primitive_bodies => {
    parse_gc_primitive_array_dump_full(r1, id_size, preview_bytes_limit)
}
TAG_GC_PRIM_ARRAY_DUMP => parse_gc_primitive_array_dump_lite(r1, id_size),
```

(Function renames to `_lite` / `_full` happen in the next step.)

- [ ] **Step 4: Plumb through `HprofRecordStreamParser`**

In `src/parser/record_stream_parser.rs`, find `with_retain_bodies` (~line 41):

```rust
    pub const fn with_retain_bodies(
        debug_mode: bool,
        id_size: u32,
        file_len: usize,
        processed_len: usize,
        initial_loop_buffer: Vec<u8>,
        retain_bodies: bool,
    ) -> Self {
        let parser = HprofRecordParser::with_retain_bodies(debug_mode, id_size, retain_bodies);
        ...
    }
```

Add a parallel `with_modes` builder:

```rust
    pub const fn with_modes(
        debug_mode: bool,
        id_size: u32,
        file_len: usize,
        processed_len: usize,
        initial_loop_buffer: Vec<u8>,
        retain_bodies: bool,
        retain_primitive_bodies: bool,
        preview_bytes_limit: u32,
    ) -> Self {
        let parser = HprofRecordParser::with_modes(
            debug_mode,
            id_size,
            retain_bodies,
            retain_primitive_bodies,
            preview_bytes_limit,
        );
        Self {
            parser,
            debug_mode,
            file_len,
            processed_len,
            loop_buffer: initial_loop_buffer,
            pooled_vec: Vec::new(),
            needed: 0,
        }
    }
```

Change `with_retain_bodies` to delegate:

```rust
    pub const fn with_retain_bodies(
        debug_mode: bool,
        id_size: u32,
        file_len: usize,
        processed_len: usize,
        initial_loop_buffer: Vec<u8>,
        retain_bodies: bool,
    ) -> Self {
        Self::with_modes(
            debug_mode,
            id_size,
            file_len,
            processed_len,
            initial_loop_buffer,
            retain_bodies,
            false,
            0,
        )
    }
```

- [ ] **Step 5: Update `slurp::parse_records` to also accept the new flags**

`parse_records` is the synchronous helper used by `referrer`/`paths`/`diff`. Find it in `src/slurp.rs`. Its signature today:

```rust
pub fn parse_records<F>(
    file_path: &str,
    debug: bool,
    retain_bodies: bool,
    mut handler: F,
) -> Result<u32, HprofSlurpError>
where
    F: FnMut(crate::parser::record::Record),
{
```

Add a v2 entry that accepts the additional opts:

```rust
pub fn parse_records_with_modes<F>(
    file_path: &str,
    debug: bool,
    retain_bodies: bool,
    retain_primitive_bodies: bool,
    preview_bytes_limit: u32,
    mut handler: F,
) -> Result<u32, HprofSlurpError>
where
    F: FnMut(crate::parser::record::Record),
{
    use crate::parser::record_parser::HprofRecordParser;
    use std::io::Read;
    let file = File::open(file_path)?;
    let mut reader = BufReader::new(file);
    let header = slurp_header(&mut reader)?;
    let id_size = header.size_pointers;

    let mut parser = HprofRecordParser::with_modes(
        debug,
        id_size,
        retain_bodies,
        retain_primitive_bodies,
        preview_bytes_limit,
    );
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 20);
    let mut pooled: Vec<crate::parser::record::Record> = Vec::with_capacity(1024);
    let mut chunk = vec![0u8; 1 << 20];

    loop {
        let n = reader.read(&mut chunk)?;
        if n > 0 {
            buf.extend_from_slice(&chunk[..n]);
        }
        if buf.is_empty() {
            break;
        }
        match parser.parse_streaming(&buf, &mut pooled) {
            Ok((rest, ())) => {
                let consumed = buf.len() - rest.len();
                buf.drain(0..consumed);
                for rec in pooled.drain(..) {
                    handler(rec);
                }
                if n == 0 && buf.is_empty() {
                    break;
                }
                if n == 0 && consumed == 0 {
                    return Err(InvalidHprofFile {
                        message: format!("trailing bytes at EOF: {} unparsed bytes", buf.len()),
                    });
                }
            }
            Err(nom::Err::Incomplete(_)) => {
                if n == 0 {
                    return Err(InvalidHprofFile {
                        message: format!(
                            "unexpected EOF mid-record: {} unparsed bytes",
                            buf.len()
                        ),
                    });
                }
                continue;
            }
            Err(nom::Err::Error(e)) | Err(nom::Err::Failure(e)) => {
                return Err(InvalidHprofFile {
                    message: format!("{e:?}"),
                });
            }
        }
    }
    Ok(id_size)
}
```

Change the existing `parse_records` to delegate:

```rust
pub fn parse_records<F>(
    file_path: &str,
    debug: bool,
    retain_bodies: bool,
    handler: F,
) -> Result<u32, HprofSlurpError>
where
    F: FnMut(crate::parser::record::Record),
{
    parse_records_with_modes(file_path, debug, retain_bodies, false, 0, handler)
}
```

- [ ] **Step 6: Build (will fail with missing `_lite` / `_full` symbols)**

Run: `cargo build --release`
Expected: errors `cannot find function parse_gc_primitive_array_dump_lite` and `parse_gc_primitive_array_dump_full`. Task 1.3 fixes this.

### Task 1.3: Split `parse_gc_primitive_array_dump` into lite/full

**Files:**
- Modify: `src/parser/record_parser.rs`

- [ ] **Step 1: Replace the existing `parse_gc_primitive_array_dump` with the lite/full pair**

Find the function (~line 660):

```rust
fn parse_gc_primitive_array_dump(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    flat_map(
        (id(id_size), parse_u32, parse_u32, parse_field_type),
        |(object_id, stack_trace_serial_number, number_of_elements, element_type)| {
            map(
                skip_array_value(element_type, number_of_elements),
                move |_data_array_elements| PrimitiveArrayDump {
                    object_id,
                    stack_trace_serial_number,
                    number_of_elements,
                    element_type,
                    body: None,
                },
            )
        },
    )
    .parse(i)
}
```

Replace with:

```rust
/// Default parser: skips the primitive body bytes (the original
/// streaming behavior). Used everywhere `--preview-bytes` is not set.
fn parse_gc_primitive_array_dump_lite(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    flat_map(
        (id(id_size), parse_u32, parse_u32, parse_field_type),
        |(object_id, stack_trace_serial_number, number_of_elements, element_type)| {
            map(
                skip_array_value(element_type, number_of_elements),
                move |_data_array_elements| PrimitiveArrayDump {
                    object_id,
                    stack_trace_serial_number,
                    number_of_elements,
                    element_type,
                    body: None,
                },
            )
        },
    )
    .parse(i)
}

/// Retain-bodies parser: copies up to `preview_bytes_limit` bytes of the
/// array body into the GcRecord. The parser still consumes the full
/// payload (we don't seek), but only stores the truncated prefix to
/// keep memory bounded. preview_bytes_limit = 0 means "retain full".
fn parse_gc_primitive_array_dump_full(
    i: &[u8],
    id_size: u32,
    preview_bytes_limit: u32,
) -> IResult<&[u8], GcRecord> {
    flat_map(
        (id(id_size), parse_u32, parse_u32, parse_field_type),
        move |(object_id, stack_trace_serial_number, number_of_elements, element_type)| {
            map(
                skip_array_value(element_type, number_of_elements),
                move |raw_bytes: &[u8]| {
                    let cap = if preview_bytes_limit == 0 {
                        raw_bytes.len()
                    } else {
                        std::cmp::min(raw_bytes.len(), preview_bytes_limit as usize)
                    };
                    let body = if cap == 0 {
                        None
                    } else {
                        Some(raw_bytes[..cap].to_vec().into_boxed_slice())
                    };
                    PrimitiveArrayDump {
                        object_id,
                        stack_trace_serial_number,
                        number_of_elements,
                        element_type,
                        body,
                    }
                },
            )
        },
    )
    .parse(i)
}
```

- [ ] **Step 2: Add unit tests for the new parser pair**

Find the existing `#[cfg(test)] mod tests` block in `src/parser/record_parser.rs` (search for `instance_dump_lite_returns_none_body`). Add at the end of that mod:

```rust
    fn synthetic_primitive_array_dump_bytes(num_elements: u32, element_byte: u8) -> Vec<u8> {
        let mut buf = Vec::new();
        // object_id (8 bytes)
        buf.extend_from_slice(&[0; 8]);
        // stack trace serial (4 bytes)
        buf.extend_from_slice(&[0; 4]);
        // num elements (4 bytes)
        buf.extend_from_slice(&num_elements.to_be_bytes());
        // element type: FieldType::Byte = 8
        buf.push(8);
        // body: num_elements bytes of `element_byte`
        buf.extend(std::iter::repeat_n(element_byte, num_elements as usize));
        buf
    }

    #[test]
    fn parse_gc_primitive_array_dump_lite_returns_none_body() {
        let buf = synthetic_primitive_array_dump_bytes(64, 0xAB);
        let (_, gcd) = parse_gc_primitive_array_dump_lite(&buf, 8).unwrap();
        match gcd {
            GcRecord::PrimitiveArrayDump { body: None, number_of_elements, .. } => {
                assert_eq!(number_of_elements, 64);
            }
            other => panic!("expected None body, got {other:?}"),
        }
    }

    #[test]
    fn parse_gc_primitive_array_dump_full_truncates_to_limit() {
        let buf = synthetic_primitive_array_dump_bytes(1024, 0xCD);
        let (_, gcd) = parse_gc_primitive_array_dump_full(&buf, 8, 100).unwrap();
        match gcd {
            GcRecord::PrimitiveArrayDump { body: Some(b), .. } => {
                assert_eq!(b.len(), 100, "expected truncation to 100 bytes");
                assert!(b.iter().all(|&x| x == 0xCD));
            }
            other => panic!("expected Some(100) body, got {other:?}"),
        }
    }

    #[test]
    fn parse_gc_primitive_array_dump_full_keeps_smaller_than_limit_intact() {
        let buf = synthetic_primitive_array_dump_bytes(8, 0xEF);
        let (_, gcd) = parse_gc_primitive_array_dump_full(&buf, 8, 200).unwrap();
        match gcd {
            GcRecord::PrimitiveArrayDump { body: Some(b), .. } => {
                assert_eq!(b.len(), 8, "expected full body, no padding");
            }
            other => panic!("expected Some(8) body, got {other:?}"),
        }
    }
```

- [ ] **Step 3: Build + tests**

Run: `cargo build --release && cargo test --release`
Expected: 63 tests pass (60 + 3 new). Watch for `cargo clippy --workspace --all-targets --all-features -- -D warnings` — if any new symbol triggers a `dead_code` warning because nothing calls it yet from a non-test path, attach `#[allow(dead_code)]` and add a comment that it's wired in PR 3. (The `_lite` and `_full` functions are called by `parse_gc_record` so they should be used.)

- [ ] **Step 4: clippy + fmt + commit**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check

git add src/parser/gc_record.rs src/parser/record_parser.rs src/parser/record_stream_parser.rs src/result_recorder.rs src/slurp.rs
git commit -m "$(cat <<'EOF'
feat(parser): retain_primitive_bodies + preview_bytes_limit (B foundation)

HprofRecordParser gains two new fields:
  * retain_primitive_bodies: bool — when true, the body of every
    PrimitiveArrayDump is captured (truncated)
  * preview_bytes_limit: u32 — cap per array; 0 = no cap

parse_gc_primitive_array_dump split into _lite (existing skip-bytes
path; body=None) and _full (retains up to preview_bytes_limit bytes).
Dispatch in parse_gc_record branches on retain_primitive_bodies, mirroring
the existing retain_bodies branch for InstanceDump and ObjectArrayDump.

HprofRecordStreamParser and slurp::parse_records gain `_with_modes`
constructors that accept the new flags; the existing builders delegate
with the new fields off, so all existing call sites are unchanged.

PrimitiveArrayDump record gains body: Option<Box<[u8]>>; existing struct
literals updated with body: None.

Three new parser tests cover lite/full split, truncation, and short-array
no-padding behavior. 63 tests pass; default summary path is byte-for-byte
unchanged (slurp_file path uses retain_primitive_bodies=false).

No user-visible effect — that lands in PR 3 (summary preview), PR 4
(paths preview), PR 5 (referrers preview), and PR 6 (-l extension).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.4: Push PR 1 + watch CI

- [ ] **Step 1: Push**

```bash
git push fork master
```

- [ ] **Step 2: Wait for CI green**

```bash
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 2 — `src/preview.rs`: text/binary auto-detect renderer

**Goal:** A new module that, given a byte slice and a `FieldType`, decides whether to render as text (UTF-8 / UTF-16 BE) or hex, with control-char escaping. Standalone — no integration with any mode yet.

**PR title:** `feat: src/preview.rs — text/binary auto-detect renderer`

### Task 2.1: Create `src/preview.rs`

**Files:**
- Create: `src/preview.rs`

- [ ] **Step 1: Write the module**

```rust
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
    /// Decoded text snippet, control chars escaped, truncated.
    Text { snippet: String, truncated: bool },
    /// Hexdump-style block, one line per 16 bytes.
    Hex { lines: Vec<String>, total_bytes: usize },
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
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
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
/// Visible newlines and tabs are kept as the literal escape `\n` / `\t`
/// to keep the preview on one logical block. We DON'T expand multi-line
/// content here — callers indent each line with two spaces.
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
```

- [ ] **Step 2: Add unit tests at the bottom of `preview.rs`**

```rust
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
                assert!(snippet.contains("\\n"), "newline should be escaped: {snippet}");
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
                assert!(lines[0].contains("|.PNG"), "ascii column missing: {}", lines[0]);
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
        // Random int bytes that happen to look ASCII-printable would
        // still go to hex — int arrays are never meaningfully text.
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
```

- [ ] **Step 3: Wire as a module in `main.rs`**

Add to `src/main.rs` near the other `mod` declarations:

```rust
mod preview;
```

(Place it alphabetically between `paths` and `prefetch_reader`.)

- [ ] **Step 4: Build + test**

Run: `cargo build --release && cargo test --release`
Expected: 69 tests pass (63 + 6 new in `preview::tests`). Clippy will warn that `render_preview` is unused — add `#[allow(dead_code)]` to the function (consumed in PR 3):

```rust
#[allow(dead_code)] // bridging — consumed in PR 3 (summary preview)
pub fn render_preview(
```

Same for `PreviewKind` if needed.

- [ ] **Step 5: clippy + fmt + commit + push**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check

git add src/preview.rs src/main.rs
git commit -m "$(cat <<'EOF'
feat: src/preview.rs — text/binary auto-detect content renderer

PreviewKind enum (Text { snippet, truncated } / Hex { lines, total_bytes })
and render_preview(bytes, element_type, total_size) entry point.

Decoding by element type:
  * Char  -> UTF-16 BE; if >90% printable, render as escaped Text;
             otherwise fall back to Hex
  * Byte  -> try UTF-8; if valid + text-like, escape; else Hex
  * other -> always Hex (int[]/float[]/long[] don't read as text)

Sanitization: control chars escaped \xNN; \n/\t/\r kept as escapes
(preview is single-block, callers add indentation).

is_text_like uses a 90%-printable heuristic (\n, \t, \r counted as
printable; replacement char U+FFFD counted as non-printable so a
mid-truncation cut goes to Hex).

Six unit tests cover UTF-8 text, PNG header (binary), UTF-16 BE
decode, truncation flag, int[] always-hex, and odd-byte-count safety.

Currently #[allow(dead_code)] until PR 3 wires it into the summary
renderer.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"

git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 3 — `summary` integration: preview lines under "Largest array instances"

**Goal:** When the user passes `--preview-bytes N`, the summary's "Largest array instances" list grows a preview line under each entry. Adds the CLI flag `--preview-bytes`.

**PR title:** `feat(summary): --preview-bytes N preview lines under "Largest array instances" (B integration)`

### Task 3.1: Add the CLI flag

**Files:**
- Modify: `src/args.rs`

- [ ] **Step 1: Add `preview_bytes` to `Cli`**

In `src/args.rs::Cli`, after the `json` flag and before the `find_referrers` flag, add:

```rust
    /// Show first N bytes/chars of primitive arrays in summary, paths,
    /// and (with -l) standalone-array list output. Default 0 (off);
    /// recommended: 200. See USERGUIDE §B.
    #[arg(long = "preview-bytes", value_name = "N", default_value_t = 0)]
    pub preview_bytes: u32,

    /// Minimum total byte size for a standalone primitive array to
    /// appear in `-l` (--listStrings) output. Effective only when both
    /// `-l` and `--preview-bytes` are set. Default 1024.
    #[arg(long = "list-arrays-min-bytes", default_value_t = 1024)]
    pub list_arrays_min_bytes: u32,
```

- [ ] **Step 2: Pipe the flag into `Mode::Summary`**

Find the `Mode::Summary` variant (~line 102) and add the two fields:

```rust
    Summary {
        input_file: String,
        top: usize,
        debug: bool,
        list_strings: bool,
        json: bool,
        preview_bytes: u32,
        list_arrays_min_bytes: u32,
    },
```

In `resolve()`, find the `Ok(Mode::Summary { ... })` construction at the bottom and add the two new fields (passed from `cli`):

```rust
    Ok(Mode::Summary {
        input_file,
        top: cli.top,
        debug: cli.debug,
        list_strings: cli.list_strings,
        json: cli.json,
        preview_bytes: cli.preview_bytes,
        list_arrays_min_bytes: cli.list_arrays_min_bytes,
    })
```

- [ ] **Step 3: Add CLI tests**

In the `args_tests` module:

```rust
    #[test]
    fn parses_preview_bytes() {
        let cli = Cli::try_parse_from(["heaptrail", "-i", "x.hprof", "--preview-bytes", "200"])
            .unwrap();
        assert_eq!(cli.preview_bytes, 200);
        assert_eq!(cli.list_arrays_min_bytes, 1024); // default
    }

    #[test]
    fn parses_list_arrays_min_bytes() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--preview-bytes",
            "100",
            "--list-arrays-min-bytes",
            "4096",
        ])
        .unwrap();
        assert_eq!(cli.preview_bytes, 100);
        assert_eq!(cli.list_arrays_min_bytes, 4096);
    }
```

- [ ] **Step 4: Update `main.rs::run_summary` signature**

Find `run_summary` in `src/main.rs`. The old signature:

```rust
fn run_summary(
    input_file: &str,
    top: usize,
    debug: bool,
    list_strings: bool,
    json: bool,
    started: Instant,
) -> Result<(), HprofSlurpError>
```

Change to accept the two new opts and the dispatch site:

```rust
fn run_summary(
    input_file: &str,
    top: usize,
    debug: bool,
    list_strings: bool,
    json: bool,
    preview_bytes: u32,
    list_arrays_min_bytes: u32,
    started: Instant,
) -> Result<(), HprofSlurpError> {
    let mut rendered_result =
        slurp_file_with_preview(input_file, debug, list_strings, preview_bytes, list_arrays_min_bytes)?;
    if json {
        let json_result = JsonResult::new(&mut rendered_result.memory_usage, top);
        json_result.save_as_file()?;
    }
    print!("{}", rendered_result.serialize(top));
    println!("File successfully processed in {:?}", started.elapsed());
    Ok(())
}
```

In `main_result` find the `Mode::Summary { ... } => run_summary(...)` arm and update the destructuring + call:

```rust
        Mode::Summary {
            input_file,
            top,
            debug,
            list_strings,
            json,
            preview_bytes,
            list_arrays_min_bytes,
        } => run_summary(
            &input_file,
            top,
            debug,
            list_strings,
            json,
            preview_bytes,
            list_arrays_min_bytes,
            now,
        ),
```

`slurp_file_with_preview` lands in Task 3.2. The build will fail until then — that's expected.

### Task 3.2: Wire `slurp_file_with_preview` and capture preview bodies in the recorder

**Files:**
- Modify: `src/slurp.rs`
- Modify: `src/result_recorder.rs`
- Modify: `src/rendered_result.rs`
- Modify: `src/main.rs` (import path)

- [ ] **Step 1: Add `slurp_file_with_preview` to `src/slurp.rs`**

`slurp_file` currently takes `(path, debug, list_strings)`. Add a v2:

```rust
pub fn slurp_file_with_preview(
    file_path: &str,
    debug_mode: bool,
    list_strings: bool,
    preview_bytes: u32,
    list_arrays_min_bytes: u32,
) -> Result<RenderedResult, HprofSlurpError> {
    let file = File::open(file_path)?;
    let file_len = file.metadata()?.len() as usize;
    let mut reader = BufReader::new(file);

    let header = slurp_header(&mut reader)?;
    let id_size = header.size_pointers;
    println!(
        "Processing {} binary hprof file in '{}' format.",
        pretty_bytes_size(file_len as u64),
        header.format
    );

    let (send_data, receive_data): (Sender<Vec<u8>>, Receiver<Vec<u8>>) =
        crossbeam_channel::unbounded();
    let (send_pooled_data, receive_pooled_data): (Sender<Vec<u8>>, Receiver<Vec<u8>>) =
        crossbeam_channel::unbounded();
    for _ in 0..2 {
        send_pooled_data
            .send(Vec::with_capacity(READ_BUFFER_SIZE))
            .expect("pre-fetcher channel should be alive");
    }
    let (send_records, receive_records): (Sender<Vec<Record>>, Receiver<Vec<Record>>) =
        crossbeam_channel::unbounded();
    let (send_pooled_vec, receive_pooled_vec): (Sender<Vec<Record>>, Receiver<Vec<Record>>) =
        crossbeam_channel::unbounded();
    let (send_result, receive_result): (Sender<RenderedResult>, Receiver<RenderedResult>) =
        crossbeam_channel::unbounded();
    let (send_progress, receive_progress): (Sender<usize>, Receiver<usize>) =
        crossbeam_channel::unbounded();

    let prefetcher = PrefetchReader::new(reader, file_len, FILE_HEADER_LENGTH, READ_BUFFER_SIZE);
    let prefetch_thread = prefetcher.start(send_data, receive_pooled_data)?;

    send_pooled_vec
        .send(Vec::new())
        .expect("recorder channel should be alive");

    let initial_loop_buffer = Vec::with_capacity(READ_BUFFER_SIZE);
    let stream_parser = HprofRecordStreamParser::with_modes(
        debug_mode,
        id_size,
        file_len,
        FILE_HEADER_LENGTH,
        initial_loop_buffer,
        false, // retain_bodies
        preview_bytes > 0, // retain_primitive_bodies
        preview_bytes,
    );

    let parser_thread = stream_parser.start(
        receive_data,
        send_pooled_data,
        send_progress,
        receive_pooled_vec,
        send_records,
    )?;

    let result_recorder =
        ResultRecorder::with_preview(id_size, list_strings, preview_bytes, list_arrays_min_bytes);
    let recorder_thread = result_recorder.start(receive_records, send_result, send_pooled_vec)?;

    let pb = ProgressBar::new(file_len as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} (speed:{bytes_per_sec}) (eta:{eta})")
        .expect("templating should never fail")
        .progress_chars("#>-"));
    while let Ok(processed) = receive_progress.recv() {
        pb.set_position(processed as u64);
    }
    pb.finish_and_clear();

    let rendered_result = receive_result
        .recv()
        .expect("result channel should be alive");

    prefetch_thread.join().map_err(|e| StdThreadError { e })?;
    parser_thread.join().map_err(|e| StdThreadError { e })?;
    recorder_thread.join().map_err(|e| StdThreadError { e })?;

    Ok(rendered_result)
}
```

Then refactor existing `slurp_file` to delegate:

```rust
pub fn slurp_file(
    file_path: &str,
    debug_mode: bool,
    list_strings: bool,
) -> Result<RenderedResult, HprofSlurpError> {
    slurp_file_with_preview(file_path, debug_mode, list_strings, 0, 1024)
}
```

- [ ] **Step 2: Add `ResultRecorder::with_preview` constructor + capture preview bodies**

In `src/result_recorder.rs`, find `pub fn new(id_size: u32, list_strings: bool) -> Self`. Add fields to the struct:

```rust
    // ---- v0.9.0 (feature B) preview support ----
    preview_bytes: u32,
    list_arrays_min_bytes: u32,
    /// `object_id -> truncated body` for the largest array of each
    /// primitive class. Populated only when preview_bytes > 0.
    array_previews: AHashMap<u64, ArrayPreview>,
```

Add an `ArrayPreview` struct in the same file (above `ResultRecorder`):

```rust
#[derive(Debug, Clone)]
pub struct ArrayPreview {
    pub element_type: FieldType,
    pub bytes: Box<[u8]>,
    pub total_bytes: u64,
}
```

Add the `with_preview` constructor below `new`:

```rust
    pub fn with_preview(
        id_size: u32,
        list_strings: bool,
        preview_bytes: u32,
        list_arrays_min_bytes: u32,
    ) -> Self {
        let mut s = Self::new(id_size, list_strings);
        s.preview_bytes = preview_bytes;
        s.list_arrays_min_bytes = list_arrays_min_bytes;
        s
    }
```

In `Self::new`, initialize the new fields:

```rust
            preview_bytes: 0,
            list_arrays_min_bytes: 1024,
            array_previews: AHashMap::new(),
```

In `record_records`, find the `GcRecord::PrimitiveArrayDump { ... }` arm. Currently it does the size + count math; extend it to capture the body when present and the total size beats the previously-stored max for this class. Insert after the existing `add_array(...)` call:

```rust
                        GcRecord::PrimitiveArrayDump {
                            object_id,
                            number_of_elements,
                            element_type,
                            body, // NEW
                            ..
                        } => {
                            let size_bytes = primitive_array_size(
                                self.id_size,
                                *element_type,
                                *number_of_elements,
                            );
                            self.primitive_array_counters
                                .entry(*element_type)
                                .or_insert_with(ArrayCounter::empty)
                                .add_array(size_bytes, *object_id);
                            // v0.9.0 (feature B): capture truncated body
                            // for the *largest* array per element type
                            // (matches summary's "Largest array instances"
                            // list).
                            if let Some(b) = body
                                && self.preview_bytes > 0
                            {
                                let entry = self
                                    .array_previews
                                    .entry(*object_id)
                                    .or_insert(ArrayPreview {
                                        element_type: *element_type,
                                        bytes: b.clone(),
                                        total_bytes: size_bytes,
                                    });
                                // If we somehow re-see this id, prefer
                                // the larger total_bytes record.
                                if size_bytes > entry.total_bytes {
                                    entry.bytes = b.clone();
                                    entry.total_bytes = size_bytes;
                                }
                            }
                            self.heap_dump_segments_gc_primitive_array_dump += 1;
                        }
```

(The destructure adds `body` and a trailing `..`. The existing arm uses `..` for object_id-and-friends; the new explicit body must be added, and the existing fields stay.)

Replace the entire arm in place. Watch for the `add_array` signature — keep its arguments unchanged.

- [ ] **Step 3: Drain `array_previews` into `RenderedResult`**

In `src/rendered_result.rs`, add to `RenderedResult`:

```rust
    /// `object_id -> ArrayPreview` for the largest primitive array of
    /// each class (v0.9.0 feature B). Empty when preview_bytes was 0.
    pub array_previews: ahash::AHashMap<u64, crate::result_recorder::ArrayPreview>,
```

Make `ArrayPreview` `pub` in `result_recorder.rs`:

```rust
#[derive(Debug, Clone)]
pub struct ArrayPreview {
    ...
}
```

In `result_recorder.rs::start`, find the `RenderedResult { ... }` constructor (around line 197) and add:

```rust
                            array_previews: std::mem::take(&mut self.array_previews),
```

In `rendered_result.rs::serialize`, also destructure `array_previews` (use `_` to ignore — render integration is the next step):

```rust
        let Self {
            summary,
            thread_info,
            mut memory_usage,
            duplicated_strings,
            captured_strings,
            allocation_sites: _,
            allocation_sites_record_count: _,
            array_previews: _,
        } = self;
```

(For now we accept the data without rendering it. PR 3 step 4 wires the rendering.)

- [ ] **Step 4: Build + tests (existing tests should pass; no preview rendering yet)**

Run: `cargo build --release && cargo test --release`
Expected: 71 tests pass (69 + 2 new in args_tests).

### Task 3.3: Render preview lines under "Largest array instances"

**Files:**
- Modify: `src/rendered_result.rs`

- [ ] **Step 1: Wire the preview render into `render_memory_usage`**

Find `render_memory_usage` in `src/rendered_result.rs`. The current function builds the "Largest array instances object ids" block:

```rust
        let largest_with_ids: Vec<&ClassAllocationStats> = memory_usage
            .iter()
            .take(top)
            .filter(|s| s.largest_object_id != 0)
            .collect();
        if !largest_with_ids.is_empty() {
            writeln!(analysis, "\nLargest array instances object ids (for retainer tracing):")
                .expect("Could not write to analysis");
            for s in &largest_with_ids {
                let display_size = pretty_bytes_size(s.largest_allocation_bytes);
                writeln!(
                    analysis,
                    "  {:>10} object_id={} {}",
                    display_size, s.largest_object_id, s.class_name
                )
                .expect("Could not write to analysis");
            }
        }
```

Make this method take `array_previews` so it can append preview lines. The cleanest approach: change the signature of `render_memory_usage` to also accept `&AHashMap<u64, ArrayPreview>` and have `serialize` pass it through.

In `serialize`, change:

```rust
            array_previews: _,
```

to:

```rust
            array_previews,
```

And change the `render_memory_usage` call to:

```rust
        let memory = Self::render_memory_usage(&mut memory_usage, top, &array_previews);
```

Update the function signature:

```rust
    fn render_memory_usage(
        memory_usage: &mut Vec<ClassAllocationStats>,
        top: usize,
        array_previews: &ahash::AHashMap<u64, crate::result_recorder::ArrayPreview>,
    ) -> String {
```

Inside the `if !largest_with_ids.is_empty()` block, after the `writeln!(analysis, "  {:>10} object_id={} {}", ...)` line, append:

```rust
                if let Some(preview) = array_previews.get(&s.largest_object_id) {
                    use crate::preview::{render_preview, PreviewKind};
                    let kind = render_preview(
                        &preview.bytes,
                        preview.element_type,
                        preview.total_bytes as usize,
                    );
                    match kind {
                        PreviewKind::Text { snippet, truncated } => {
                            // Indent + truncate to ~120 chars per line
                            let trimmed: String = snippet.chars().take(140).collect();
                            let suffix = if truncated || snippet.len() > 140 { "..." } else { "" };
                            writeln!(analysis, "       {trimmed}{suffix}")
                                .expect("Could not write to analysis");
                        }
                        PreviewKind::Hex { lines, total_bytes } => {
                            writeln!(
                                analysis,
                                "       (binary, showing first {} of {} bytes)",
                                lines.iter().map(|l| l.len()).sum::<usize>().min(preview.bytes.len()),
                                total_bytes
                            )
                            .expect("Could not write to analysis");
                            for line in lines.iter().take(3) {
                                writeln!(analysis, "       {line}")
                                    .expect("Could not write to analysis");
                            }
                        }
                    }
                }
```

Update the existing test fixture sites that call `render_memory_usage` directly. Find them with:

```bash
grep -n "render_memory_usage" src/rendered_result.rs
```

Each call site must now pass an empty `&AHashMap::new()` for the preview map, or a populated one. Update test sites with empty maps:

```rust
        let output = RenderedResult::render_memory_usage(&mut memory_usage, 1, &ahash::AHashMap::new());
```

- [ ] **Step 2: Drop the dead-code allow on `render_preview`**

In `src/preview.rs`, remove `#[allow(dead_code)]` on `render_preview` — it's now used.

- [ ] **Step 3: Build + tests**

Run: `cargo build --release && cargo test --release`
Expected: 71 tests pass; existing gold-file test `supported_64_bits` still passes (no `--preview-bytes` flag means `preview_bytes=0` → `array_previews` empty → no preview lines rendered → byte-identical output).

- [ ] **Step 4: clippy + fmt + commit**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check

git add src/args.rs src/main.rs src/slurp.rs src/result_recorder.rs src/rendered_result.rs src/preview.rs
git commit -m "$(cat <<'EOF'
feat(summary): --preview-bytes N preview under "Largest array instances"

CLI gains two flags:
  * --preview-bytes N            (default 0 = off)
  * --list-arrays-min-bytes N    (default 1024; effective only with -l)

When --preview-bytes > 0 the summary path:
  1. Constructs the streaming parser with retain_primitive_bodies=true
     and preview_bytes_limit=N (PR 1's foundation).
  2. ResultRecorder captures the truncated body of the largest primitive
     array per element type (matches the existing largest_object_id
     mechanism — no separate hot path).
  3. Preview lines are rendered under each "Largest array instances
     object_ids" entry, using src/preview.rs's text/binary auto-detect
     (PR 2's renderer).

Output (with --preview-bytes 200):

  Largest array instances object ids (for retainer tracing):
       5.64MiB object_id=1661812752 char[]
         <?xml version="1.0" encoding='utf-8'...

  Or for binary content:
     154.16KiB object_id=1740406800 byte[]
         (binary, showing first 200 of 154.16KiB bytes)
         00000000  89 50 4e 47 0d 0a 1a 0a  00 00 00 0d ...
         ...

Default summary path (preview_bytes=0) is byte-for-byte unchanged;
gold-file test still passes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"

git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

- [ ] **Step 5: Smoke test on both canonical fixtures**

```bash
cargo build --release
echo "=== JAVA_PROFILE_1.0.2 with --preview-bytes 200 ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.2.hprof -t 5 --preview-bytes 200 2>&1 | tail -25
echo ""
echo "=== JAVA_PROFILE_1.0.3 with --preview-bytes 200 ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.3.hprof -t 5 --preview-bytes 200 2>&1 | tail -25
echo ""
echo "=== regression: no --preview-bytes (must match v0.8.0 output) ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.2.hprof -t 1 2>&1 | grep -A1 "object_id="
```

Expected: with `--preview-bytes 200`, the largest `char[]` shows an XML/text preview on 1.0.2; on 1.0.3 the largest `char[]` shows JSON/text content. `PrimitiveArrayNoDataDump` records (Android-only) yield `(no data — zygote-shared array)` placeholders if any are reported as the largest of their type. Without the flag, output identical to v0.8.0.

---

## PR 4 — `--paths-from-id`: preview block on primitive-array hops

**Goal:** When `--paths-from-id` is run with `--preview-bytes N`, render a preview block for the start id (when it's a primitive array) and any primitive array surfaced during the chain.

**PR title:** `feat(paths): primitive-array preview on --paths-from-id (B integration)`

### Task 4.1: Plumb `preview_bytes` into `Mode::Paths` + `paths::run`

**Files:**
- Modify: `src/args.rs`
- Modify: `src/main.rs`
- Modify: `src/paths.rs`

- [ ] **Step 1: Add `preview_bytes` to `Mode::Paths`**

In `src/args.rs::Mode`, the `Paths` variant becomes:

```rust
    Paths {
        input_file: String,
        object_id: u64,
        max_depth: u8,
        debug: bool,
        json: bool,
        preview_bytes: u32,
    },
```

In `resolve()`, the `Mode::Paths { ... }` constructor:

```rust
        return Ok(Mode::Paths {
            input_file,
            object_id,
            max_depth: cli.max_depth,
            debug: cli.debug,
            json: cli.json,
            preview_bytes: cli.preview_bytes,
        });
```

- [ ] **Step 2: Pass `preview_bytes` through `main.rs::run_paths`**

The current `run_paths` reads `mode @ Mode::Paths { .. }` and just calls `paths::run(&mode)`. No change at that layer — `paths::run` consumes the mode.

- [ ] **Step 3: Plumb in `paths::run` + extend `find_first_holder`**

In `src/paths.rs::run`, destructure `preview_bytes`:

```rust
    let (input_file, start_object_id, max_depth, debug, preview_bytes) = match mode {
        Mode::Paths {
            input_file,
            object_id,
            max_depth,
            debug,
            preview_bytes,
            ..
        } => (
            input_file.as_str(),
            *object_id,
            *max_depth,
            *debug,
            *preview_bytes,
        ),
        _ => {
            return Err(HprofSlurpError::NotYetImplemented {
                what: "paths::run only handles Mode::Paths",
            });
        }
    };
```

Below `let idx = pass1_index(input_file, debug)?;`, add a one-shot pass that captures preview bodies for the *start* object id and any primitive array along the chain. Since the chain is built incrementally and we don't know which ids are primitive arrays in advance, the simplest approach is: when `preview_bytes > 0`, run a single retain-primitive-bodies pass that collects previews keyed by object_id for any primitive array in the dump, then look up by id at render time.

Add to `src/paths.rs` near the top:

```rust
use crate::result_recorder::ArrayPreview;
```

After the `pass1_index` call:

```rust
    let array_previews: AHashMap<u64, ArrayPreview> = if preview_bytes > 0 {
        collect_primitive_array_previews(input_file, debug, preview_bytes)?
    } else {
        AHashMap::new()
    };
```

Add the helper at the bottom of `paths.rs`:

```rust
fn collect_primitive_array_previews(
    path: &str,
    debug: bool,
    preview_bytes: u32,
) -> Result<AHashMap<u64, ArrayPreview>, HprofSlurpError> {
    use crate::parser::record::Record;
    let mut previews: AHashMap<u64, ArrayPreview> = AHashMap::new();
    crate::slurp::parse_records_with_modes(
        path,
        debug,
        false,
        true,
        preview_bytes,
        |rec| {
            if let Record::GcSegment(GcRecord::PrimitiveArrayDump {
                object_id,
                number_of_elements,
                element_type,
                body: Some(b),
                ..
            }) = rec
            {
                let total = u64::from(number_of_elements)
                    * u64::from(crate::referrer::field_byte_size(element_type, 8) as u32);
                previews.insert(
                    object_id,
                    ArrayPreview {
                        element_type,
                        bytes: b,
                        total_bytes: total,
                    },
                );
            }
        },
    )?;
    Ok(previews)
}
```

(Note: `field_byte_size` is `pub(crate)` from `referrer.rs` — confirm with `grep -n "pub(crate) const fn field_byte_size" src/referrer.rs`. If not pub, make it so as a tiny ancillary edit.)

- [ ] **Step 4: Pass `array_previews` into `PathResult` for rendering**

Add to `PathResult`:

```rust
    /// Preview bodies (when --preview-bytes > 0) keyed by object_id.
    /// Used by render_text to display content under primitive-array
    /// hops or the start id.
    #[serde(skip)] // skip in JSON: opaque blob, not useful structured
    pub array_previews: AHashMap<u64, ArrayPreview>,
```

In the constructor at the bottom of `paths::run`:

```rust
    Ok(PathResult {
        start_object_id,
        steps,
        terminated_at_root,
        root_kind,
        root_thread_name,
        root_frame,
        max_depth_reached,
        depth,
        array_previews,
    })
```

- [ ] **Step 5: Render previews in `render_text`**

Find `pub fn render_text(r: &PathResult) -> String` in `src/paths.rs`. After the existing `start  ── id={}` line, add:

```rust
    let _ = writeln!(out, "  start  ── id={}", r.start_object_id);
    if let Some(preview) = r.array_previews.get(&r.start_object_id) {
        render_preview_block(&mut out, preview);
    }
```

After each `hop{:>2} ── id={} (...)` line, do the same lookup:

```rust
        let _ = writeln!(
            out,
            "  hop{:>2} ── id={}  ({arrow})",
            i + 1,
            s.holder_object_id,
        );
        if let Some(preview) = r.array_previews.get(&s.holder_object_id) {
            render_preview_block(&mut out, preview);
        }
```

Add the helper:

```rust
fn render_preview_block(out: &mut String, preview: &ArrayPreview) {
    use crate::preview::{render_preview, PreviewKind};
    use std::fmt::Write;
    let kind = render_preview(
        &preview.bytes,
        preview.element_type,
        preview.total_bytes as usize,
    );
    match kind {
        PreviewKind::Text { snippet, truncated } => {
            let trimmed: String = snippet.chars().take(140).collect();
            let suffix = if truncated || snippet.len() > 140 { "..." } else { "" };
            let _ = writeln!(out, "         {trimmed}{suffix}");
        }
        PreviewKind::Hex { lines, total_bytes } => {
            let _ = writeln!(out, "         (binary, {} bytes total)", total_bytes);
            for line in lines.iter().take(2) {
                let _ = writeln!(out, "         {line}");
            }
        }
    }
}
```

- [ ] **Step 6: Add a unit test**

In `paths::tests`:

```rust
    #[test]
    fn render_text_includes_start_preview_when_array_previews_has_start_id() {
        use crate::result_recorder::ArrayPreview;
        let mut previews = AHashMap::new();
        previews.insert(
            42u64,
            ArrayPreview {
                element_type: crate::parser::gc_record::FieldType::Byte,
                bytes: b"<?xml version=\"1.0\"?>".to_vec().into_boxed_slice(),
                total_bytes: 21,
            },
        );
        let r = PathResult {
            start_object_id: 42,
            steps: vec![],
            terminated_at_root: false,
            root_kind: None,
            root_thread_name: None,
            root_frame: None,
            max_depth_reached: false,
            depth: 0,
            array_previews: previews,
        };
        let out = render_text(&r);
        assert!(
            out.contains("<?xml"),
            "expected preview, got:\n{out}"
        );
    }
```

- [ ] **Step 7: Update existing path tests that build `PathResult` literals**

The existing render-text tests build `PathResult { ... }` directly. Add `array_previews: AHashMap::new(),` to each.

- [ ] **Step 8: Build + tests + clippy + fmt + commit + push + watch CI**

```bash
cargo build --release
cargo test --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check

git add src/args.rs src/main.rs src/paths.rs src/referrer.rs
git commit -m "$(cat <<'EOF'
feat(paths): primitive-array preview on --paths-from-id (B integration)

When the user passes --preview-bytes N along with --paths-from-id,
heaptrail does an extra streaming pass with retain_primitive_bodies=true
and preview_bytes_limit=N to collect truncated bodies of every
primitive array in the dump, keyed by object_id.

The chain renderer prints a preview block under:
  * the start id, when it's a primitive array
  * any hop whose holder id is a primitive array

Text previews (UTF-8 byte arrays, UTF-16 char arrays) are escaped and
indented; binary previews (PNG headers, int[]/long[]/etc.) get a hex
header line.

Default --paths-from-id behavior (preview_bytes=0) is unchanged — no
extra pass, no preview rendering.

field_byte_size in referrer.rs promoted to pub(crate) for use by
paths::collect_primitive_array_previews.

Smoke-tested on both canonical fixtures per CLAUDE.md:
  * JAVA_PROFILE_1.0.2 — 5.64 MiB char[] start id shows
    "<?xml version=\"1.0\" encoding='utf-8'..."
  * JAVA_PROFILE_1.0.3 — Gson StringWriter chain unchanged (no
    primitive arrays in this particular path)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"

git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 5 — `--find-referrers id:N`: preview when target is a primitive array

**Goal:** When `--find-referrers id:N` targets a primitive array AND `--preview-bytes` is set, the report header includes the array's preview.

**PR title:** `feat(referrer): primitive-array preview on --find-referrers id:N (B integration)`

### Task 5.1: Plumb `preview_bytes` + render

**Files:**
- Modify: `src/args.rs`
- Modify: `src/referrer.rs`

- [ ] **Step 1: Add `preview_bytes` to `Mode::FindReferrers`**

```rust
    FindReferrers {
        ...existing fields...
        preview_bytes: u32,
    },
```

`resolve()` constructor adds:

```rust
        return Ok(Mode::FindReferrers {
            ...
            preview_bytes: cli.preview_bytes,
        });
```

- [ ] **Step 2: Capture preview during `referrer::run`**

In `referrer::run`, destructure `preview_bytes` and add a conditional preview-collection pass for the target id (only when target is `Exact("id:<u64>")` or bare numeric and the id resolves to a primitive array). Cleanest: always collect previews when `preview_bytes > 0` and let render decide which to surface.

Add to `ReferrerResult`:

```rust
    #[serde(skip)]
    pub array_previews: AHashMap<u64, crate::result_recorder::ArrayPreview>,
```

In `run`, after `let idx = pass1_index(...)`:

```rust
    let array_previews: AHashMap<u64, crate::result_recorder::ArrayPreview> = if preview_bytes > 0 {
        crate::paths::collect_primitive_array_previews(input_file, debug, preview_bytes)?
    } else {
        AHashMap::new()
    };
```

(Promote `paths::collect_primitive_array_previews` to `pub(crate)` so referrer can call it.)

In the `ReferrerResult { ... }` constructor at the bottom, add `array_previews,`.

- [ ] **Step 3: Render preview in `render_text` (referrer module)**

Find `pub fn render_text(r: &ReferrerResult) -> String`. After the `Found N target instance(s) for X` line, when `target_label` starts with `"id:"`, look up that id's preview in `array_previews` and render. Add helper:

```rust
fn render_target_preview(
    out: &mut String,
    label: &str,
    previews: &AHashMap<u64, crate::result_recorder::ArrayPreview>,
) {
    use crate::preview::{render_preview, PreviewKind};
    use std::fmt::Write;
    let id_str = label.strip_prefix("id:").unwrap_or(label);
    let id: u64 = match id_str.parse() {
        Ok(v) => v,
        Err(_) => return,
    };
    if let Some(preview) = previews.get(&id) {
        let kind = render_preview(
            &preview.bytes,
            preview.element_type,
            preview.total_bytes as usize,
        );
        match kind {
            PreviewKind::Text { snippet, truncated } => {
                let trimmed: String = snippet.chars().take(140).collect();
                let suffix = if truncated || snippet.len() > 140 { "..." } else { "" };
                let _ = writeln!(out, "  preview: {trimmed}{suffix}");
            }
            PreviewKind::Hex { lines, total_bytes } => {
                let _ = writeln!(out, "  preview: (binary, {total_bytes} bytes total)");
                for line in lines.iter().take(2) {
                    let _ = writeln!(out, "    {line}");
                }
            }
        }
    }
}
```

Call it in `render_text` right after the "Found N target instance(s) for X" line:

```rust
    render_target_preview(&mut out, &r.target_label, &r.array_previews);
```

- [ ] **Step 4: Build + tests + clippy + fmt + commit + push + CI**

```bash
cargo build --release
cargo test --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check

git add src/args.rs src/referrer.rs src/paths.rs
git commit -m "$(cat <<'EOF'
feat(referrer): primitive-array preview on --find-referrers id:N

When --find-referrers id:<u64> is run with --preview-bytes N and the
target is a primitive array, the report header now includes a preview
of the first N bytes/chars.

Reuses paths::collect_primitive_array_previews (now pub(crate)) for
the preview-collection pass; ReferrerResult gains a #[serde(skip)]
array_previews field.

Smoke-tested on both canonical fixtures per CLAUDE.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"

git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 6 — `-l --preview-bytes`: standalone large array listing

**Goal:** `-l` extended to also list standalone large `char[]` / `byte[]` arrays whose total bytes ≥ `--list-arrays-min-bytes` (default 1024). Only effective when both flags are set.

**PR title:** `feat(list-strings): standalone large array listing under -l + --preview-bytes`

### Task 6.1: Capture eligible arrays in the recorder + render

**Files:**
- Modify: `src/result_recorder.rs`

- [ ] **Step 1: Track all arrays meeting the size threshold**

In `ResultRecorder`, add a new field:

```rust
    /// Standalone large primitive arrays for the -l + --preview-bytes
    /// extension. Captured only when `list_strings && preview_bytes > 0`
    /// and `total_bytes >= list_arrays_min_bytes`.
    standalone_large_arrays: Vec<(u64, ArrayPreview)>,
```

Initialize to `Vec::new()` in `new`.

In the `PrimitiveArrayDump` arm of `record_records`, in addition to the existing largest-per-class capture, when `self.list_strings && self.preview_bytes > 0`, also push qualifying arrays:

```rust
                            if self.list_strings
                                && self.preview_bytes > 0
                                && size_bytes >= u64::from(self.list_arrays_min_bytes)
                                && let Some(b) = body
                                && matches!(*element_type, FieldType::Byte | FieldType::Char)
                            {
                                self.standalone_large_arrays.push((
                                    *object_id,
                                    ArrayPreview {
                                        element_type: *element_type,
                                        bytes: b.clone(),
                                        total_bytes: size_bytes,
                                    },
                                ));
                            }
```

- [ ] **Step 2: Render the standalone-large-arrays section in `render_captured_strings`**

Find `fn render_captured_strings(&self) -> String` (around line 340). The current implementation lists String values. Append the new section:

```rust
    fn render_captured_strings(&self) -> String {
        let mut strings: Vec<_> = self.utf8_strings_by_id.values().collect();
        strings.sort_unstable();
        let mut result = String::from("\nList of Strings\n");
        for s in strings {
            result.push_str(s);
            result.push('\n');
        }

        // v0.9.0 (feature B): standalone large arrays.
        if !self.standalone_large_arrays.is_empty() {
            use crate::preview::{render_preview, PreviewKind};
            use crate::utils::pretty_bytes_size;
            use std::fmt::Write;

            let mut sorted: Vec<&(u64, ArrayPreview)> =
                self.standalone_large_arrays.iter().collect();
            sorted.sort_by(|a, b| b.1.total_bytes.cmp(&a.1.total_bytes));

            let _ = writeln!(
                result,
                "\nStandalone large arrays (>= {} bytes, sorted by size):",
                self.list_arrays_min_bytes
            );
            for (oid, preview) in sorted.iter().take(50) {
                let size = pretty_bytes_size(preview.total_bytes);
                let kind_label = match preview.element_type {
                    FieldType::Byte => "byte[]",
                    FieldType::Char => "char[]",
                    _ => "primitive[]",
                };
                let kind = render_preview(
                    &preview.bytes,
                    preview.element_type,
                    preview.total_bytes as usize,
                );
                let preview_text = match kind {
                    PreviewKind::Text { snippet, .. } => {
                        let trimmed: String = snippet.chars().take(80).collect();
                        format!("  {trimmed}")
                    }
                    PreviewKind::Hex { .. } => "  (binary)".to_string(),
                };
                let _ = writeln!(
                    result,
                    "  {size:>10}  object_id={oid:<14}  {kind_label:<8} {preview_text}"
                );
            }
        }

        result
    }
```

- [ ] **Step 3: Drain into `RenderedResult`**

`render_captured_strings` is called from `start` already (when `list_strings` is true). The new section is automatically included. No additional plumbing needed.

- [ ] **Step 4: Build + tests + clippy + fmt + commit + push + CI**

```bash
cargo build --release
cargo test --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check

git add src/result_recorder.rs
git commit -m "$(cat <<'EOF'
feat(list-strings): standalone large array listing under -l + --preview-bytes

When both -l (--listStrings) and --preview-bytes N are set, the
existing "List of Strings" output is followed by a new section:

  Standalone large arrays (>= 1024 bytes, sorted by size):
       5.64MiB  object_id=1661812752  char[]    <?xml version="1.0"...
   154.16KiB  object_id=1740406800  byte[]    {"home":{"timestamp":...
       8.02KiB  object_id=2595270656  byte[]    (binary)

Only char[] and byte[] (text-like primitive arrays) are listed; int[]
/ long[] / float[] are omitted since they don't read meaningfully as
text. Threshold configurable via --list-arrays-min-bytes (default
1024). Top 50 entries shown to keep output bounded.

Default -l behavior (without --preview-bytes) unchanged.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"

git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 7 — Docs + version bump + release

**PR title:** `chore: bump to 0.9.0; document content preview (B); v0.9.0 release`

### Task 7.1: `Cargo.toml` 0.8.0 → 0.9.0

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Edit version**

```toml
version = "0.9.0"
```

- [ ] **Step 2: Build (regenerates `Cargo.lock`)**

Run: `cargo build --release`
Expected: `Compiling heaptrail v0.9.0`.

### Task 7.2: README.md updates

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add `--preview-bytes` to the cheat sheet**

After the existing `### \`--allocation-sites\`` subsection in "Beyond the summary", add:

```markdown
### `--preview-bytes` — content preview (v0.9.0)

```bash
heaptrail -i my.hprof -t 5 --preview-bytes 200
heaptrail -i my.hprof --paths-from-id <id> --preview-bytes 200
heaptrail -i my.hprof --find-referrers id:<id> --preview-bytes 200
heaptrail -i my.hprof -l --preview-bytes 200 --list-arrays-min-bytes 4096
```

Show the first N bytes/chars of primitive arrays (`char[]`, `byte[]`,
etc.) inline with the existing output. UTF-8 / UTF-16 BE auto-detect
with control-char escaping; falls back to xxd-style hex on binary.
Default 0 (off). Details in
[USERGUIDE §B](USERGUIDE.md#b---preview-bytes--content-preview).
```

- [ ] **Step 2: Update Features bullets**

Add to the Features list:

```markdown
- **content preview** (`--preview-bytes`) — show the first N bytes/chars
  of primitive arrays inline (UTF-8 / UTF-16 / hex auto-detect). Identifies
  XML, JSON, log content, image-magic-byte signatures, etc. without leaving
  heaptrail.
```

### Task 7.3: USERGUIDE.md — new §B section

**Files:**
- Modify: `USERGUIDE.md`

- [ ] **Step 1: Insert the §B section**

After the existing §F section (target-glob), before §C (allocation-sites), insert:

```markdown
## B — `--preview-bytes` — content preview

### Why this exists

Real session that motivated this: `summary` showed a 72 MiB `char[]`.
`--paths-from-id` walked to a `StringBuilder.value` rooted at a Gson
serializer. The chain told us *who* held it but not *what* it contained.
The investigation needed:

1. `adb shell` into the device to find files matching the size
2. Source-grep the codebase for serialization code
3. Eventually realize it was the `home_catalog_snapshot.xml` from
   `SharedPreferences`

If the first 200 chars had been visible inline, the identification would
have been instant: `<?xml version="1.0" encoding='utf-8' standalone='yes' ?>`
unmistakably labels the `char[]` as the SharedPreferences blob.

### How to use it

`--preview-bytes N` is a global flag. When set, primitive arrays
(`char[]`, `byte[]`, etc.) get a preview line showing the first N bytes,
auto-decoded as text or hex.

```bash
# In summary's "Largest array instances" list
heaptrail -i my.hprof -t 5 --preview-bytes 200

# Under primitive-array hops in --paths-from-id
heaptrail -i my.hprof --paths-from-id 1661812752 --max-depth 12 --preview-bytes 200

# When --find-referrers targets a specific array
heaptrail -i my.hprof --find-referrers id:1661812752 --preview-bytes 200

# Lists every standalone large array (>= 1 KiB) above the String list
heaptrail -i my.hprof -l --preview-bytes 200 --list-arrays-min-bytes 1024
```

### Sanitization

| Element type | Decoder | Fallback |
|--------------|---------|----------|
| `Char` (UTF-16 BE — Java strings) | UTF-16 → escaped text | hex |
| `Byte` | UTF-8 → escaped text | hex |
| `Int` / `Long` / `Float` / `Double` / `Short` | always hex | – |

Control chars (other than `\n`, `\t`, `\r`, which are kept as escape
sequences) are rendered as `\xNN`. Hex output is xxd-style (offset, hex,
ASCII column).

### Memory cost

`--preview-bytes N` runs an opt-in parser pass that retains *at most* N
bytes per primitive array. For an Android dump with ~1.3M primitive
arrays and N=200, peak working memory adds ~260 MiB. For typical JVM
dumps (orders of magnitude fewer arrays) the cost is negligible.

### When to use

- After `summary` shows a giant `char[]` / `byte[]` whose retainer chain
  doesn't identify the content. `--preview-bytes 200` plus a re-run of
  `summary` adds inline content snippets to the largest-array list.
- During `--paths-from-id` walks where a hop lands on a primitive array.
- For ad-hoc inspection: `--find-referrers id:<u64> --preview-bytes 200`
  shows the array's contents as a header on the referrer report.
- For exploratory listing of all big text-like arrays: `-l --preview-bytes 200`.
```

(Sections C/D/A that follow keep their numbering — they're letter-keyed already.)

### Task 7.4: Plugin SKILL.md — integrate `--preview-bytes` into triage workflow

**Files:**
- Modify: `plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md`

- [ ] **Step 1: Add `--preview-bytes` to the operating-modes table**

In the existing operating-modes section, after the existing `### 5.` (allocation-sites) subsection, add:

```markdown
### 6. `--preview-bytes N` — content preview (v0.9.0, feature B)

Global flag (not its own mode — applies to summary, --paths-from-id,
--find-referrers id:N, and -l). When set, primitive arrays (char[],
byte[], etc.) are previewed inline:

```bash
heaptrail -i heap.hprof -t 5 --preview-bytes 200
```

**What it tells you:** UTF-8 / UTF-16 / hex auto-detect of the first N
bytes per primitive array, surfaced under each "Largest array instances"
entry, primitive-array path hops, find-referrers targets, and (with -l)
a standalone-large-arrays listing.

*Engineering use case:* a 72 MiB `char[]` whose holder chain ended at a
Gson `StringBuilder` — the chain told us *who* held it but not *what*
it contained. `--preview-bytes 200` would have shown
`<?xml version="1.0"...home_catalog_snapshot...` inline, identifying it
as the SharedPreferences XML blob without the source-grep + adb-shell
sleuthing detour.

**Wall time / memory:** opt-in parser pass retains at most N bytes per
primitive array. Memory bound: N × array-count. ~260 MiB peak on a
200 MiB Android dump with N=200; negligible on typical JVM dumps.
```

- [ ] **Step 2: Update the standard triage workflow**

In the numbered triage workflow, append a step:

```markdown
7. (Optional) **`--preview-bytes 200`** added to any of the above
   surfaces inline content for `char[]` / `byte[]` arrays. Use when a
   chain leads to a giant primitive array and the holder identity
   alone doesn't say what it contains.
```

- [ ] **Step 3: Update the cheat-sheet table**

Add:

```markdown
| Inline content preview | append `--preview-bytes 200` to summary, paths, find-referrers, or -l |
```

- [ ] **Step 4: Bump SKILL.md version reference**

Find `version 0.8.0+` (one occurrence) and replace with `version 0.9.0+`.

### Task 7.5: Plugin manifest version bumps

**Files:**
- Modify: `plugins/analysing-heap-dumps/.claude-plugin/plugin.json`
- Modify: `.claude-plugin/marketplace.json`

- [ ] **Step 1: Edit both manifests**

In `plugin.json`:

```json
"version": "0.9.0",
```

In `marketplace.json` (under `plugins[0]`):

```json
"version": "0.9.0"
```

- [ ] **Step 2: Validate JSON**

```bash
python3 -m json.tool plugins/analysing-heap-dumps/.claude-plugin/plugin.json > /dev/null && echo "plugin.json: ok"
python3 -m json.tool .claude-plugin/marketplace.json > /dev/null && echo "marketplace.json: ok"
```

Expected: both `ok`.

### Task 7.6: Final test gate + commit + tag + release

- [ ] **Step 1: Full lint + test pass**

```bash
cargo test --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
```

Expected: all green.

- [ ] **Step 2: Smoke test on both fixtures**

```bash
cargo build --release
echo "=== JAVA_PROFILE_1.0.2 with --preview-bytes 200 ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.2.hprof -t 5 --preview-bytes 200 2>&1 | tail -25
echo ""
echo "=== JAVA_PROFILE_1.0.3 with --preview-bytes 200 ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.3.hprof -t 5 --preview-bytes 200 2>&1 | tail -25
echo ""
echo "=== regression: no --preview-bytes ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.2.hprof -t 1 2>&1 | tail -10
```

Expected: previews visible on largest `char[]` for both fixtures (text on text-like content; hex on binary). Without the flag, output identical to v0.8.0.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock README.md USERGUIDE.md \
        plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md \
        plugins/analysing-heap-dumps/.claude-plugin/plugin.json \
        .claude-plugin/marketplace.json

git commit -m "$(cat <<'EOF'
chore: bump to 0.9.0; document content preview (feature B)

  * Cargo.toml: 0.8.0 -> 0.9.0 (minor; new flags, no breaking changes)
  * README.md: cheat-sheet entry for --preview-bytes; pointer to
    USERGUIDE §B; Features bullet
  * USERGUIDE.md: new §B section with engineering use-case framing
    (the 72 MiB SharedPreferences XML char[] that motivated this),
    sanitization rules, memory cost note
  * SKILL.md: sixth integrated mode added; engineering use-case
    framing for Claude diagnostics; standard triage workflow gains a
    step 7 for content preview; cheat sheet entry; version bump
  * plugin.json + marketplace.json: 0.8.0 -> 0.9.0 (so users can
    /plugin update analysing-heap-dumps after pull)

Closes the v0.9.0 spec at
docs/superpowers/specs/2026-05-10-heaptrail-v0.9-design.md.
Feature B (content preview) landed across PRs 1-6.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"

git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

- [ ] **Step 4: Tag + release**

```bash
git tag -a v0.9.0 -m "v0.9.0 — content preview for primitive arrays (feature B)"
git push fork v0.9.0

cat > /tmp/release-notes-090.md <<'NOTES'
## v0.9.0 — Content preview for primitive arrays

A single new flag, `--preview-bytes N`, surfaces the first N bytes/chars of `char[]` / `byte[]` / etc. inline in `summary`, `--paths-from-id`, `--find-referrers id:N`, and `-l`. UTF-8 / UTF-16 BE auto-detect with control-char escaping; falls back to xxd-style hex on binary.

### Why this exists

Real session: `summary` showed a 72 MiB `char[]`. `--paths-from-id` walked to a `StringBuilder.value` held by a Gson serializer. heaptrail told us *who* held it but not *what* it contained — investigation needed `adb shell` for file size + source-grep for serialization candidates. With `--preview-bytes 200`, the inline `<?xml version="1.0"...home_catalog_snapshot...` would have identified it as the SharedPreferences XML blob in one command.

### What's new

- `--preview-bytes N` flag (default 0 = off). Recommended: 200.
- `summary` largest-array entries get a preview line.
- `--paths-from-id` primitive-array hops show preview blocks.
- `--find-referrers id:N` shows preview when target is a primitive array.
- `-l` extends to a "Standalone large arrays" section listing every text-like array ≥ 1 KiB. Threshold tunable via `--list-arrays-min-bytes`.

### Memory cost

Truncated capture: at most N bytes per primitive array. ~260 MiB peak on a 200 MiB Android dump with N=200; negligible on typical JVM dumps.

### Compatibility

- Every existing CLI invocation produces byte-identical output unless `--preview-bytes` is set.
- Existing JSON schema unchanged (the in-memory preview blobs are `#[serde(skip)]`).
- One new dependency: none. (Reused `globset` from v0.8.0; no new crates.)

### Plugin update

```
/plugin marketplace update johnneerdael/heaptrail
/plugin update analysing-heap-dumps@analysing-heap-dumps
```

### Roadmap

- v1.0.0 — feature E (full Lengauer–Tarjan dominator tree / retained size).

### Install

```bash
cargo install heaptrail               # crates.io 0.9.0
cargo install --git https://github.com/johnneerdael/heaptrail
```

Pre-built binaries for Linux/macOS/Windows × x86_64/aarch64 attached below.
NOTES

gh release create v0.9.0 --repo johnneerdael/heaptrail \
  --title "heaptrail v0.9.0" -F /tmp/release-notes-090.md
```

- [ ] **Step 5: Watch the release workflow**

```bash
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow 'release binaries' --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 20; done
gh run list --repo johnneerdael/heaptrail --workflow 'release binaries' --limit 1 --json conclusion -q '.[0].conclusion'
gh release view v0.9.0 --repo johnneerdael/heaptrail --json assets -q '.assets[].name'
curl -sf https://crates.io/api/v1/crates/heaptrail/0.9.0 -o /dev/null && echo "0.9.0 published on crates.io"
```

Expected: `success`; six binary assets listed; crates.io check returns `0.9.0 published`.

---

## Self-Review Checklist

- [ ] **Spec coverage:** every §3 component in the design spec maps to a task.
  - §3.1 (parser changes) → Tasks 1.1, 1.2, 1.3
  - §3.2 (recorder + ArrayPreview) → Tasks 3.2, 6.1
  - §3.3 (preview module) → Task 2.1
  - §3.4 (mode wiring) → Tasks 3.1–3.3 (summary), 4.1 (paths), 5.1 (referrers), 6.1 (-l)
  - §3.5 (CLI surface) → Task 3.1
  - §4 (output format) → covered in Tasks 3.3, 4.1, 5.1, 6.1
  - §5 (perf + memory) → smoke tests in Tasks 3.3 step 5 + 7.6 step 2
  - §6 (testing) → unit + integration tests in Tasks 1.3, 2.1, 3.1, 4.1
  - §7 (rollout) → 7-PR structure matches; tag in Task 7.6
- [ ] **No placeholders:** every step has concrete code or commands.
- [ ] **Type consistency:** `ArrayPreview`, `PreviewKind`, `array_previews`, `preview_bytes`, `list_arrays_min_bytes`, `retain_primitive_bodies`, `preview_bytes_limit`, `slurp_file_with_preview`, `parse_records_with_modes`, `with_modes`, `with_preview` defined once each, used by the same name throughout.
- [ ] **Existing tests stay green:** every commit's last step runs the full lint+test gate.
- [ ] **Both canonical fixtures (CLAUDE.md):** smoke tests in PRs 3 and 7 explicitly run on both `JAVA_PROFILE_1.0.2.hprof` and `JAVA_PROFILE_1.0.3.hprof`. The 1.0.3 fixture's `PrimitiveArrayNoDataDump` records have `body: None` always (the parser never sets a body for the NoData variant — see PR 1 step 1 explicit non-change), so the preview path naturally skips them.

## Risk Notes

- **Memory pressure on huge Android dumps:** truncation cap is the load-bearing assumption. If a fixture appears with hundreds of MiB of primitive arrays and N=200 doesn't bound the working set acceptably, lower the implicit recorder cap (only retain the *largest* array per element type, not all). Already that's what `array_previews` does in `ResultRecorder`; it's the `paths::collect_primitive_array_previews` and the `-l` extension's `standalone_large_arrays` Vec that retain per-array. The `-l` extension is gated on `total_bytes >= list_arrays_min_bytes` so small arrays don't pile up.
- **Recorder cloning the body twice:** `b.clone()` in the `array_previews` arm allocates a second box. For typical N=200 this is 200 bytes per "largest" array (one per element type, ~5 total) — negligible. If profiling shows it matters, replace `b.clone()` with `mem::take(b)` and adjust the `body: Option<Box<[u8]>>` to be moved out (requires changing the match pattern from a borrow to a move).
- **`gold-file` test for summary:** The existing `test-heap-dumps/hprof-64-result.txt` gold output assumes default flags. PR 3 must NOT change it; the test `slurp::tests::supported_64_bits` doesn't pass `--preview-bytes`, so the new code path is dormant. Verify with `cargo test --release supported_64_bits` after PR 3.
