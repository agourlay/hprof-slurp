# heaptrail v0.9.0 — Design Spec (Feature B: content preview)

**Date:** 2026-05-10
**Target version:** 0.9.0 (minor bump from 0.8.0)
**Status:** approved (design decisions locked; ready for implementation plan)

## 1. Scope & Overview

heaptrail v0.9.0 adds **content preview** for primitive arrays (`char[]`,
`byte[]`, etc.) — feature B of the original v1.0.0 vision. When investigating
a giant `char[]` whose holder chain alone doesn't identify what it
*contains*, the first ~200 bytes are usually enough to recognize it (XML
header, JSON object, log line, image magic byte, etc.).

### Why this exists (the engineering pain point)

Real session that motivated this: `summary` showed a 72 MiB `char[]`.
`--paths-from-id` walked to a `StringBuilder.value` rooted at a Gson
serializer. heaptrail told us *who* held it but not *what* it contained.
The investigation needed:

1. `adb shell` into the device to find files matching the size
2. Source-grep the codebase for serialization code
3. Eventually realize it was the `home_catalog_snapshot.xml` from
   `SharedPreferences`

If the first 200 chars had been visible inline:

```
char[] (5.64 MiB) preview:
  <?xml version="1.0" encoding='utf-8' standalone='yes' ?>
  <map>
      <set name="home_catalog_snapshot">
          <string>{"items":[{"id":"123",...
```

…the identification would have been instant.

### Feature scope

| Surface | Behavior with `--preview-bytes N` |
|---------|-----------------------------------|
| `summary` "Largest array instances" list | Each largest-of-its-class primitive array gets a preview line of the first N bytes/chars under its row. |
| `--paths-from-id` | When the chain starts at a primitive array (or passes through one), the renderer prints a preview block for that array. |
| `--find-referrers id:N` | When the target is a primitive array, the report header includes a preview. |
| `-l` / `--listStrings` | Extended: when both `-l` and `--preview-bytes N` are present, also lists standalone large `char[]` / `byte[]` arrays (not just `java.lang.String` instances) above a size threshold (default: 1 KiB). |

`--preview-bytes` is **off by default**. Existing invocations produce
byte-identical output.

## 2. Architecture decisions (locked from v0.8.0 brainstorming + this round)

| Decision | Choice |
|----------|--------|
| API shape | Opt-in `--preview-bytes N` flag (default off). Applies across all modes that surface primitive arrays. |
| Auto-detect text vs binary | Yes. If the first N bytes are >90% printable ASCII or valid UTF-8 (or for `char[]` if N/2 char[] elements are valid UTF-16 chars in the printable range), render as text with control chars escaped (`\n`, `\t`). Otherwise render as truncated hexdump (xxd-style). |
| Memory strategy | **Truncated capture.** Parser retains at most `--preview-bytes N` bytes per primitive array. Memory bound: `N × count_of_primitive_arrays`. For the canonical 200 MiB Android dump (≈1.3M arrays, N=200) ≈ 260 MiB peak — bounded, predictable. |
| New parser mode | `retain_primitive_bodies: bool` + `preview_bytes_limit: usize` plumbed through `HprofRecordParser` analogous to existing `retain_bodies`. Default false; `slurp_file` passes false; modes that need previews construct the parser with this enabled. |
| Sanitization | Replace control chars (0x00–0x08, 0x0B–0x0C, 0x0E–0x1F, 0x7F) with `\xNN` escapes; preserve `\n`, `\t`, `\r` literally for readability. Unicode handled per UTF-8 / UTF-16 decode below. |

## 3. Component design

### 3.1 Parser changes (`src/parser/`)

`PrimitiveArrayDump` gains an optional `body: Option<Box<[u8]>>` field
(parallel to `InstanceDump.body` / `ObjectArrayDump.elements`):

```rust
PrimitiveArrayDump {
    object_id: u64,
    stack_trace_serial_number: u32,
    number_of_elements: u32,
    element_type: FieldType,
    /// Truncated raw bytes (first `preview_bytes_limit` per array).
    /// Retained only when the parser is constructed with
    /// `retain_primitive_bodies = true` and rendered by `--preview-bytes`.
    body: Option<Box<[u8]>>,
}
```

Same for `PrimitiveArrayNoDataDump` (the Android extension): the
NoData variant explicitly has no body, so its `body` field stays absent.
We don't preview NoData arrays.

`HprofRecordParser` adds two fields:

```rust
pub struct HprofRecordParser {
    debug_mode: bool,
    id_size: u32,
    heap_dump_remaining_len: u32,
    retain_bodies: bool,
    /// New in v0.9.0: capture truncated primitive array bodies.
    retain_primitive_bodies: bool,
    /// Max bytes per primitive array. 0 = no truncation (capture all).
    preview_bytes_limit: u32,
}
```

`parse_gc_primitive_array_dump` becomes a lite/full pair (mirrors
v0.7.0's lite/full split for instance and object arrays):

- `_lite` (default): exactly as today — `skip_array_value` consumes the
  bytes; `body: None`.
- `_full`: `take(min(payload_len, preview_bytes_limit))` retained as the
  body, *then* skip remainder. Boxed slice attached.

`parse_gc_record` dispatch branches on `retain_primitive_bodies` the
same way it already does on `retain_bodies` for the other two array
record types.

### 3.2 Recorder changes (`src/result_recorder.rs`)

`ArrayCounter` (the per-class array stats) gains an optional preview
slot for the largest array of its class:

```rust
struct ArrayCounter {
    number_of_arrays: u64,
    max_size_bytes_seen: u64,
    max_size_object_id: u64,
    /// Truncated body of the largest array of this class (when
    /// `retain_primitive_bodies` is on). Replaced whenever a larger
    /// array is seen, so we end up with the body of the
    /// `max_size_object_id` array — exactly what the summary's
    /// "Largest array instances object ids" list points at.
    max_size_body: Option<Box<[u8]>>,
    total_size_bytes: u64,
}
```

Captured at `add_array` time when the parser provides a body.

`RenderedResult` gains:

```rust
pub struct RenderedResult {
    // ... existing fields ...
    /// `object_id -> truncated body` for the largest array of each
    /// class. Consumed by the summary renderer to print preview lines.
    pub array_previews: AHashMap<u64, ArrayPreview>,
}

pub struct ArrayPreview {
    pub element_type: FieldType,
    pub bytes: Box<[u8]>,
    pub total_bytes: u64,  // full size, not truncated
}
```

### 3.3 Preview rendering (new `src/preview.rs` module)

```rust
pub enum PreviewKind {
    Text { snippet: String },
    Hex { lines: Vec<String> },
}

/// Detect whether `bytes` is text-like and render accordingly.
/// `element_type` matters: char[] is UTF-16 BE (Java), byte[] is raw,
/// String.value (also char[]) follows char[] rules.
pub fn render_preview(bytes: &[u8], element_type: FieldType, max_chars: usize) -> PreviewKind;
```

Decoding:

| Element type | Decoder | Heuristic |
|--------------|---------|-----------|
| `Char` (UTF-16 BE) | Decode as UTF-16 chars; check >90% are printable + space/tab/newline | Java strings, XML/JSON in char arrays |
| `Byte` | Try UTF-8 decode; if fails or >10% control chars, switch to hex | Serialized bytes, file contents |
| Other primitives | Always hex (numbers don't read meaningfully as text) | int[], float[], etc. |

Output formatting:

```
char[] (5.64 MiB) preview:
  <?xml version="1.0" encoding='utf-8' standalone='yes' ?>\n
  <map>\n
      <set name="home_catalog_snapshot">\n
          <string>{"items":[{"id":"123",...
```

For binary content:

```
byte[] (28.6 KiB) preview (binary):
  00000000  89 50 4e 47 0d 0a 1a 0a  00 00 00 0d 49 48 44 52  |.PNG........IHDR|
  00000010  00 00 03 e8 00 00 02 1c  08 06 00 00 00 9e 04 9d  |................|
  ...
```

Truncation indicator:

- Text: `...` suffix when truncated.
- Hex: `(showing first N of M bytes)` line.

### 3.4 Mode wiring

| Mode | Wire-up |
|------|---------|
| `summary` | If `--preview-bytes N > 0`, construct the slurp parser with `retain_primitive_bodies=true, preview_bytes_limit=N`. Append preview lines under each "Largest array instances" entry. |
| `--paths-from-id` | If `--preview-bytes N > 0`, the path-walk parser sets `retain_primitive_bodies=true`. Render preview for the start id (when it's a primitive array) and any primitive array hit during a hop. |
| `--find-referrers id:N` | Same — when target is a primitive array, render its preview. |
| `--allocation-sites` | No preview integration (allocation sites are about *how* objects came into being, not *what* they contain). |
| `-l` (--listStrings) | When `-l` + `--preview-bytes N`, also list standalone large `char[]` / `byte[]` arrays whose total bytes ≥ 1024 (configurable via `--list-arrays-min-bytes`, default 1024). |

### 3.5 CLI surface

```rust
/// Show first N bytes/chars of primitive arrays (char[], byte[], etc.)
/// in summary, --paths-from-id, --find-referrers id:N, and (when -l is
/// also set) -l output. Default 0 (off). Recommended: 200.
#[arg(long = "preview-bytes", value_name = "N", default_value_t = 0)]
pub preview_bytes: u32,

/// Minimum total byte size for a standalone array to appear in -l
/// output. Only effective when -l + --preview-bytes are both set.
#[arg(long = "list-arrays-min-bytes", default_value_t = 1024)]
pub list_arrays_min_bytes: u32,
```

## 4. Output format

### `summary` with `--preview-bytes 200`

```
Largest array instances object ids (for retainer tracing):
   5.64MiB object_id=1661812752 char[]
     <?xml version="1.0" encoding='utf-8' standalone='yes' ?>
     <map>
         <set name="home_catalog_snapshot">
             <string>{"items":[{"id":"123",...
 418.64KiB object_id=2595270656 int[]
     (binary, 100 of 428k bytes)
     00000000  00 00 00 1a 00 00 0e 0e  00 00 0e 00 00 00 ...
```

### `--paths-from-id` with `--preview-bytes 200`

```
Path from object_id=1661812752 (depth 9 step(s)):
  start  ── id=1661812752 (char[], 5.64 MiB)
        <?xml version="1.0" encoding='utf-8' ...
  hop 1 ── id=1661812736  (via java.lang.String.value)
  hop 2 ── id=364312776  (via java.util.HashMap$Node.value)
  ...
```

### Extended `-l --preview-bytes 200`

```
List of Strings:
... (existing String values) ...

Standalone large arrays (≥ 1024 bytes, sorted by size):
   5.64MiB object_id=1661812752 char[]   <?xml version="1.0"...
 418.64KiB object_id=2595270656 int[]    (binary)
 154.16KiB object_id=1740406800 byte[]   {"home":{"timestamp":...
 ...
```

## 5. Performance + memory

| Mode | Wall time impact | Memory impact |
|------|------------------|---------------|
| Default (no `--preview-bytes`) | None | None |
| `summary --preview-bytes 200` | +20–40 ms on 200 MiB dump | +N × array-count = ~260 MiB on Android dumps |
| `--paths-from-id --preview-bytes 200` | Same as today (the path-walk's existing retain_bodies pass is augmented to also keep primitive bodies — same parser pipeline) | Same as above |
| `-l --preview-bytes 200` | +20–40 ms (sorting + filter) | Same |

The 260 MiB peak is from the Android-canonical dump's ~1.3M primitive
arrays. For most JVM dumps (orders of magnitude fewer arrays) memory is
negligible.

## 6. Testing

### Unit tests

- `parse_gc_primitive_array_dump_full` retains exactly N bytes when
  `preview_bytes_limit = N` and the array is larger than N.
- `parse_gc_primitive_array_dump_full` retains the full body when the
  array is smaller than N.
- `parse_gc_primitive_array_dump_lite` always emits `body: None`.
- `preview::render_preview` against:
  - UTF-8 byte slice → text path with escapes
  - Random bytes → hex path
  - UTF-16 BE char[] holding ASCII XML → text path
  - PNG header bytes → hex path (binary detection)

### Integration tests

- `summary --preview-bytes 200` on `JAVA_PROFILE_1.0.2.hprof` →
  golden-file snapshot of the summary including a non-empty preview
  for the largest `char[]`.
- `summary` (no preview flag) on the same fixture → byte-identical to
  v0.8.0 output (no regression).
- `-l --preview-bytes 200` on the JVM 64-bit fixture → output contains
  at least one "Standalone large arrays" line.

### Fixture coverage

Both canonical fixtures must pass:

- `JAVA_PROFILE_1.0.2.hprof` — JVM, 8-byte ids
- `JAVA_PROFILE_1.0.3.hprof` — Android, 4-byte ids, includes `PrimitiveArrayNoDataDump`

The NoData variant must not panic when `--preview-bytes` is set; render
"(no data — zygote-shared array)" instead of a preview block.

## 7. Rollout

Sequential PRs onto master, single v0.9.0 tag at end:

| PR | Title | Files |
|----|-------|-------|
| 1 | `feat(parser): retain_primitive_bodies mode (B foundation)` | `gc_record.rs`, `record_parser.rs`, `record_stream_parser.rs` |
| 2 | `feat: src/preview.rs — text/binary auto-detect renderer` | `preview.rs` (new), unit tests |
| 3 | `feat(summary): preview lines under "Largest array instances"` | `result_recorder.rs`, `rendered_result.rs`, `slurp.rs`, `args.rs`, `main.rs` |
| 4 | `feat(paths): preview block on primitive-array hops` | `paths.rs` |
| 5 | `feat(referrers): preview when target is a primitive array` | `referrer.rs` |
| 6 | `feat(list-strings): standalone large array listing` | `result_recorder.rs` |
| 7 | `chore: bump 0.8.0 -> 0.9.0; document v0.9.0; tag v0.9.0` | `Cargo.toml`, README, USERGUIDE, SKILL.md, plugin.json, marketplace.json |

Each PR is independently reviewable. PR 1 lands the parser change with
no user-visible effect. PRs 2–6 each add a render surface. PR 7 is the
tag.

## 8. Risk register

| Risk | Mitigation |
|------|------------|
| 260 MiB peak on huge Android dumps surprises users | Default `--preview-bytes 0` (off); doc explicitly states the cost; recommend N=200 not N=10000. |
| `--list-arrays-min-bytes` default (1024) is wrong for some dumps | Configurable via flag; can document tuning examples. |
| Auto-detect misclassifies a partial UTF-8 sequence at the truncation boundary | Decode lossy with `String::from_utf8_lossy`; replacement chars `\u{fffd}` aren't escaped but signal "boundary cut here". Acceptable. |
| `PrimitiveArrayNoDataDump` (Android 1.0.3) has no body to preview | Render "(no data — zygote-shared array)" placeholder; explicit test against `JAVA_PROFILE_1.0.3.hprof`. |
| Memory pressure on dumps where 90% of arrays are tiny + 10% are huge | Truncation cap is the same N regardless of array size — small arrays cost ≤ N bytes; huge ones cost exactly N. Predictable. |

## 9. Out of scope (explicit)

- Retaining full primitive array bodies (deferred to "two-pass on-demand
  fetch" if anyone ever asks).
- Class-name regex / glob filtering of which arrays get previews
  (single global flag suffices; use `--find-referrers --target-glob`
  upstream if you want narrower scope).
- Preview output in `--diff-from`/`--diff-to` (the diff is about counts
  and sizes; previews would clutter the table).
- Anything from feature E (dominator tree). That ships in v1.0.0 with
  its own spec.

## 10. Roadmap context

| Release | Status |
|---------|--------|
| v0.8.0 | shipped 2026-05-09: A (thread/frame), C (alloc sites), D (Object[] index), F (glob) |
| **v0.9.0** | **this spec — feature B (content preview)** |
| v1.0.0 | feature E (Lengauer–Tarjan dominator tree); separate spec |

## 11. References

- v0.8.0 spec: `docs/superpowers/specs/2026-05-09-heaptrail-v0.8-design.md`
- Engineering pain point that motivated B: original feedback transcript
  2026-05-09 (the 72 MiB SharedPreferences XML char[]).
- HPROF format reference: OpenJDK `heapDumper.cpp` for `HPROF_GC_PRIM_ARRAY_DUMP`.
- Android extension: `art/runtime/hprof/hprof.cc` for
  `HPROF_PRIMITIVE_ARRAY_NODATA_DUMP` (0xC3).
