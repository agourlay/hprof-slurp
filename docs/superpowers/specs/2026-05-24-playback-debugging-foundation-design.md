# Playback Debugging Foundation Design

Date: 2026-05-24

## Context

`HEAPTRAIL_FEATURE_GUIDE.md` came from a real Nexio Android TV heap
investigation using multiple `com.nexio.tv` dumps. Since that guide was
written, heaptrail has gained several of its listed improvements:

- stable JSON output paths with `--json-out`
- clearer unavailable-feature diagnostics
- `android-capture`
- R8/ProGuard `--mapping` and `--auto-mapping`
- content classifiers and zero-byte HProf validation

The remaining highest-value work for Nexio playback debugging is the ability to
explain memory over time, reduce noisy ownership tables, and preserve whatever
root/thread context the HProf actually contains.

## Goals

Build reusable CLI primitives that improve Android playback investigations
without hard-coding Nexio-specific behavior:

1. Compare 3+ snapshots in sequence and highlight monotonic growth.
2. Group noisy referrer output into meaningful owner summaries.
3. Improve path/root output when thread stack metadata is missing.
4. Optionally attach small native-memory context from `dumpsys meminfo`.

These primitives should later compose into a higher-level `playback-report`
workflow, but this design does not add that wrapper yet.

## Non-Goals

- Do not build a full Android profiler.
- Do not parse Perfetto traces.
- Do not infer thread names or native ownership when the input data cannot
  prove them.
- Do not store object-level data for every snapshot in diff-series mode.
- Do not replace Eclipse MAT's interactive graph exploration.

## Feature 1: Diff Series

Add a new mode:

```bash
heaptrail --diff-series launch.hprof home.hprof play.hprof stop.hprof soak.hprof \
  --diff-by bytes \
  --top 30 \
  --mapping app/build/outputs/mapping/universalRelease/mapping.txt \
  --json \
  --json-out reports/playback-series.json
```

### Behavior

`--diff-series` accepts three or more HProf files in the order supplied by the
user. It computes class-level counts and shallow bytes for each snapshot, then
derives:

- per-step deltas such as `launch -> home`, `home -> play`
- total delta from first to last snapshot
- monotonic growth candidates, where count or bytes never decrease across the
  series and final value is greater than the first value

`--diff-by count|bytes` controls sorting of step deltas and monotonic growth.
`--top` limits text output per section. JSON output keeps the complete computed
series, not only the top text rows.

### Text Output

The report should include:

- snapshot list in input order
- per-snapshot total shallow heap
- top deltas for each adjacent step
- top total deltas from first to last
- monotonic growth candidates

Mapped class names should render the same way as summary and diff modes.

### JSON Output

JSON should include:

- `snapshots[]`: path, index, total shallow bytes, class count
- `steps[]`: from index, to index, top or complete class deltas
- `classes[]`: per-class counts and bytes for every snapshot
- `monotonic_growth[]`: class-level monotonic candidates
- `obfuscated_class_name` when mapping deobfuscates a class

## Feature 2: Common Holder Summaries

Add `--group-holders` as a modifier for `--find-referrers` and
`--target-glob`:

```bash
heaptrail -i play.hprof \
  --target-glob 'androidx.media3.**' \
  --hops 2 \
  --group-holders \
  --mapping mapping.txt
```

### Behavior

The existing referrer scanner remains the source of truth. A new grouping layer
consumes the existing referrer result rows and produces aggregate owner
summaries.

Initial grouping dimensions:

- package family, such as `androidx.media3`, `com.nexio.tv`, `java.util`
- holder class
- holder field or hop chain label

When `--retained-size` is present, grouped rows should include retained context
when the underlying referrer data already has it. Grouping must not trigger a
new retained-size computation by itself.

### Text Output

Text output should preserve existing raw referrer information and add a grouped
holder section near it. The grouped section should show:

- owner family
- representative holder class
- field or field-chain label
- total ref count
- retained context if available

### JSON Output

Existing hop arrays remain unchanged for compatibility. Add:

- `grouped_holders[]`

Each grouped holder row should include grouping keys, ref count, and optional
retained fields.

## Feature 3: Root And Thread Fallback

Improve path rendering where Android HProfs end at roots such as
`RootJavaFrame` but lack stack frame or thread-name metadata.

### Behavior

Path and leak-suspect rendering should expose concrete root metadata that the
parser already has or can safely index, such as:

- root kind
- root object id where present
- thread serial number where present
- stack trace serial number where present
- thread object id if it can be proven

If thread name or Java frame metadata is absent, output should say which data is
absent and show the available ids. It must not guess names or frames.

### JSON Output

Path-like JSON structures should gain optional root metadata fields while
preserving current fields.

## Feature 4: Native Context

Add optional `--native-context <PATH>` support where useful, starting with
`--diff-series`.

```bash
heaptrail --diff-series launch.hprof play.hprof stop.hprof \
  --native-context meminfo-play.txt
```

### Behavior

Parse a bounded subset of Android `dumpsys meminfo` text:

- Java heap
- native heap
- graphics
- GL
- total PSS

The parser is best-effort. Missing sections should produce a warning and an
incomplete native-context block, not fail HProf analysis.

Multiple native-context files are not required in the first implementation. A
single file can annotate the report with the process-level native snapshot
available at that point in the investigation.

## Architecture

### `series_diff.rs`

New module. It should reuse class rollup primitives from summary/diff paths and
store only class-level statistics per snapshot. It must not retain object-level
records after each snapshot is processed.

### `holder_grouping.rs`

New module. It should consume existing referrer result structures and return
grouped rows. Keep grouping separate from `src/referrer.rs`, which is already a
large module.

### Root Metadata Changes

Use small changes in root metadata indexing and rendering. Prefer extending
existing path result structures over creating a separate path mode.

### `native_context.rs`

New module. It should parse simple text fixtures and return a compact
best-effort struct. It should not depend on ADB or attempt live device queries.

## Error Handling

- `--diff-series` with fewer than three files should fail with a clear CLI
  error.
- Missing or unreadable HProf inputs should use existing input-file errors.
- Mapping errors should behave like other mapped modes.
- `--group-holders` without `--find-referrers` or `--target-glob` should fail
  during CLI validation.
- Invalid native-context files should warn and continue unless the file cannot
  be read at all. An unreadable explicitly supplied file should fail.

## Testing

Unit tests:

- monotonic and non-monotonic diff-series detection
- diff-series JSON shape
- class mapping in diff-series rows
- holder grouping by package family, class, and field label
- root fallback rendering with missing thread metadata
- native-context parser with synthetic Android-like `dumpsys meminfo`

Integration or smoke tests:

- run diff-series against the two Nexio real dumps plus a copied snapshot to
  exercise 3+ input validation
- run grouped holders against a known class or glob in the Nexio dump
- verify existing path output is improved for a `RootJavaFrame` case without
  inventing unavailable thread metadata

## Implementation Slices

1. Diff-series class rollups, text output, and tests.
2. Diff-series JSON and mapping support.
3. Holder grouping module and `--group-holders` CLI modifier.
4. Root/thread fallback metadata and rendering improvements.
5. Native-context parser and optional report block.
6. Documentation and real Nexio HProf validation.

## Decisions

- `--diff-series --json` emits all computed class rows because scripts need
  complete data. `--top` limits text output only.
- Native context supports one file in the first implementation. Per-snapshot
  native context can be added later if the parser proves useful.
