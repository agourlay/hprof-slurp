# heaptrail v0.8.0 — Design Spec

**Date:** 2026-05-09
**Target version:** 0.8.0 (minor bump from 0.7.1)
**Status:** approved (brainstorming complete; ready for implementation plan)

## 1. Scope & Overview

heaptrail v0.8.0 surfaces metadata that's already in the parsed record stream
but currently dropped or unrendered, plus adds one trivial UX win
(glob targeting). Four features ship together; no parser-mode changes; no
breaking changes to existing CLI invocations.

### Features in v0.8.0

| ID | Feature | What ships |
|----|---------|------------|
| **A** | Thread name on `RootJavaFrame` / `RootThreadObject` / `RootJniLocal` / `RootJniMonitor` | `--paths-from-id` chain terminator includes thread name + frame method when terminating at a thread-owned root |
| **C** | Allocation sites | New `--allocation-sites` mode + always-on hint in summary (`AllocationSites: 12,453 records (run with --allocation-sites)` or `not present`) |
| **D** | Object[] index in path hops | `--paths-from-id` array hops show `via java.lang.Object[][12]` instead of `via java.lang.Object[][]` |
| **F** | Glob class-name targeting | New `--target-glob 'com.foo.*'` flag on `--find-referrers`, shell-style glob (`*`, `**`, `?`, `[abc]`) |

### Roadmap to v1.0.0

Phasing is purely about reviewable PR size and shipping incremental value;
every feature A–F is in master by the time v1.0.0 tags.

| Release | Scope | Why split |
|---------|-------|-----------|
| **v0.8.0** (this spec) | A, C, D, F | Render-only; no parser changes |
| **v0.9.0** | B — content preview (`--preview-bytes N`, auto-detect text vs binary) | Needs new `retain_primitive_bodies` parser mode + sanitization |
| **v1.0.0** | E — full dominator tree (Lengauer–Tarjan; MAT-equivalent retained sizes) | Largest algorithmic work; deserves a dedicated review cycle |

### Non-goals of v0.8.0

- No new parser modes; no new parser tags (we already have everything we need).
- No JSON schema breaking changes — only additive fields.
- No backwards-incompat CLI changes — every existing invocation produces
  byte-identical output unless the dump itself contains the relevant metadata.
- No allocation-site sort options beyond default (`bytes_allocated`); covered
  in a possible v0.8.1.
- Thread names appear only in `--paths-from-id`, not in `summary`. The summary
  layer doesn't need them; paths is where the diagnosis happens.

## 2. Architecture

**One-line:** v0.8.0 enriches the existing `Pass1Index` and `ResultRecorder`
with three already-parsed-but-discarded record types
(`StartThread`, `StackTrace`+`StackFrame`, `AllocationSites`), then surfaces
them in two existing renderers (paths, summary) and one new renderer
(allocation-sites mode).

### Record types we already parse but ignore

```
Record::StartThread { thread_serial_number, thread_object_id, thread_name_id, ... }
Record::StackTrace(StackTraceData { serial_number, thread_serial_number, stack_frame_ids[] })
Record::StackFrame(StackFrameData { stack_frame_id, method_name_id, source_file_name_id, line_number, ... })
Record::AllocationSites { allocation_sites: Box<Vec<AllocationSite>>, ... }
```

These already flow through the streaming pipeline; the recorder drops them.
v0.8.0 gives each a home.

### Component changes

All in existing files except for one new module (`allocation_sites.rs`).

| File | Change |
|------|--------|
| `src/referrer.rs` (`Pass1Index`) | Add four maps: `thread_name_by_serial`, `stack_trace_by_serial`, `stack_frame_by_id`, `root_thread_meta_by_id`. Populate from `StartThread`/`StackTrace`/`StackFrame`/`RootJavaFrame`+`RootJniLocal`+`RootJniMonitor`+`RootThreadObject` records during pass-1 streaming. Existing `gc_root_kind_by_id` unchanged. |
| `src/paths.rs` | When walker terminates at a thread-owned root, look up thread name + top frame's method/file/line via the new maps; render in chain terminator. (A) Capture matched element index when an array hop fires; render `Object[][N]` instead of `Object[]`. (D) |
| `src/result_recorder.rs` | Capture `AllocationSites` records into a `Vec<AllocationSite>`; track `StackTrace`/`StackFrame` into the existing index. Add `AllocationSites:` line to `RenderedResult.summary`. (C summary hint) |
| `src/args.rs` | Add `--allocation-sites` flag (sets `Mode::AllocationSites`). Add `--target-glob <PATTERN>` flag (sets `Mode::FindReferrers` with a glob target). Update `resolve()` mutual-exclusion check. |
| `src/main.rs` | Dispatch `Mode::AllocationSites` to a new `run_allocation_sites()` handler. |
| `src/allocation_sites.rs` (new, ~150 LOC) | Per-class top-N allocation sites with stack traces resolved to method/file/line. Output: text table + JSON sidecar. (C full mode) |
| `src/referrer.rs` (target resolution) | When `--target-glob` is set, resolve to a *set* of class IDs whose dotted FQ-name matches the glob. Existing pass-2 logic unchanged — it already supports a target set; we just feed it more entries. (F) |

### Data flow

```
prefetcher → record_stream_parser → recorder
                                      ├─ ResultRecorder      (summary/diff)  ← C summary hint
                                      ├─ Pass1Index           (referrer/paths) ← A thread index, D array index
                                      └─ AllocSiteRecorder    (new mode)       ← C full mode
```

Each recorder is an independent consumer of the same record stream. No new
threads, no new channels.

### Memory budget impact

- Thread/stack-frame maps: ~16 bytes per StackFrame, ~thread-name length per
  thread. On the 235 MiB Android dump: ~20K stack frames + 50 threads ≈
  320 KB extra. Negligible.
- `AllocationSites` vec (when present): one entry per call site, ~32 bytes
  each. Typical dump has 0; alloc-tracked dumps have a few thousand. ≤ 100 KB.

### Backwards compatibility

- Every existing CLI invocation produces byte-identical output unless the
  dump contains the relevant metadata.
- Existing JSON output gains optional fields (`thread_name`, `frame`,
  `array_index`); no fields removed; existing field types unchanged.

## 3. Per-Feature Detail

### A — Thread name on thread-owned roots (`--paths-from-id`)

#### Pass 1 enrichment

```rust
// new fields on Pass1Index
pub thread_name_by_serial: AHashMap<u32, Box<str>>,            // thread_serial -> name
pub stack_trace_by_serial: AHashMap<u32, Vec<u64>>,            // serial -> [frame_id, ...]
pub stack_frame_by_id: AHashMap<u64, ResolvedFrame>,           // frame_id -> resolved
pub root_thread_meta_by_id: AHashMap<u64, ThreadFrameRef>,     // root obj_id -> {thread_serial, frame_idx}
```

`ResolvedFrame { method_name: String, source_file: Option<String>, line_number: i32 }`
— utf8-resolved at first reference (not pre-resolved on every frame).

`ThreadFrameRef { thread_serial: u32, frame_idx: Option<u32> }` is captured
when the indexer sees `RootJavaFrame { object_id, thread_serial_number,
frame_number_in_stack_trace }`. `RootJniLocal` and `RootJniMonitor` carry
`thread_serial_number` but no frame index. `RootThreadObject` carries
`thread_object_id` (resolved separately to its serial via the index).

#### Render in `paths::render_text`

Before:

```
  → reached GC root: RootJavaFrame
```

After:

```
  → reached GC root: RootJavaFrame
        thread "pool-7-thread-2" (serial=23)
        at com.example.SharedPreferencesImpl$EditorImpl.commitToMemory(SharedPreferencesImpl.java:478)
```

When the thread serial resolves but the frame doesn't (common for
`RootThreadObject`):

```
  → reached GC root: RootThreadObject
        thread "main" (serial=1)
```

When neither resolves:

```
  → reached GC root: RootJavaFrame
        (thread metadata not in dump)
```

JSON output adds `root_thread_name: Option<String>` and
`root_frame: Option<{method, file, line}>` to `PathResult`.

### C — Allocation sites

#### Always-on summary hint

One line in summary, after the "Heap dumps containing in total … segments"
block:

```
AllocationSites: 12,453 records (run with --allocation-sites for stack traces)
```

Or:

```
AllocationSites: not present (capture with `am profile start <pid>`)
```

The line lands inside the existing `RenderedResult.summary` string so all
existing callers (text + JSON) get it for free.

#### New mode (`--allocation-sites`)

```bash
heaptrail -i heap.hprof --allocation-sites --top 20
```

Output:

```
Top 20 allocation sites by bytes_allocated:

  ─ 1.21 GiB  /  4,812,000 instances  com.nexio.tv.domain.model.MetaPreview#<init>
        at com.squareup.moshi.adapters.ClassJsonAdapter.fromJson(ClassJsonAdapter.java:128)
        at com.squareup.moshi.JsonAdapter$1.fromJson(JsonAdapter.java:194)
        at com.nexio.tv.network.HomeRepository.fetchCatalog(HomeRepository.kt:87)
        ...

  ─ 421.0 MiB  /  1,880,000 instances  java.lang.String#<init>
        at java.util.Arrays.copyOfRange(Arrays.java:3664)
        ...
```

- Class name resolved via `class_serial_number` → `LoadClassData` →
  `class_name_id` → utf8.
- Stack frames resolved via `stack_trace_serial_number` →
  `StackTraceData.stack_frame_ids` → `StackFrameData.method_name_id` etc.
- Sort key: `bytes_allocated` (default; only sort key in v0.8.0).

JSON sidecar: `heaptrail-allocation-sites-<ts>.json` with

```json
[
  {
    "class_name": "com.nexio.tv.domain.model.MetaPreview",
    "bytes_allocated": 1300000000,
    "instances_allocated": 4812000,
    "stack_trace": [
      {"method": "com.squareup.moshi.adapters.ClassJsonAdapter.fromJson",
       "file": "ClassJsonAdapter.java",
       "line": 128},
      ...
    ]
  },
  ...
]
```

If `--allocation-sites` is invoked on a dump without alloc data, exits with:

```
error: no AllocationSites records in this dump (capture with `am profile start <pid>`)
```

### D — Object[] index in path hops

`paths::find_first_holder` for the `ObjectArrayDump` arm: instead of just
recording "this array holds the target", record *which slot*:

```rust
let idx = elems.iter().position(|&rid| rid == target).unwrap();
// store idx alongside the existing PathStep
```

`PathStep` gains `array_index: Option<u32>` (None for instance-field hops,
`Some` for array hops).

Render — before:

```
  hop 5  ── id=518041528  (via java.lang.Object[])
```

After:

```
  hop 5  ── id=518041528  (via java.lang.Object[][12])
```

JSON output: `array_index` field added to `PathStep` schema.

### F — Glob class-name targeting

#### CLI

`--target-glob <pattern>` is a new top-level flag. It activates
`Mode::FindReferrers` the same way the existing `--find-referrers <target>`
flag does, but interprets the value as a glob pattern instead of an exact
class FQ-name.

Mutually exclusive with `--find-referrers` — passing both is a CLI error
("--target-glob cannot be used with --find-referrers"). Enforced via
`clap`'s `conflicts_with`.

Examples:
```
heaptrail -i heap.hprof --find-referrers java.util.ArrayList     # exact match (existing)
heaptrail -i heap.hprof --target-glob 'com.nexio.tv.domain.model.*'   # new
heaptrail -i heap.hprof --target-glob '**$Itr' --hops 2          # all iterator inner classes
```

#### Glob syntax (matched against the dotted FQ name)

| Pattern | Meaning |
|---------|---------|
| `*` | any sequence of characters except `.` |
| `**` | any sequence including `.` |
| `?` | one character |
| `[abc]` | character class |

So `com.foo.*` matches `com.foo.Bar` but not `com.foo.bar.Baz`;
`com.foo.**` matches both.

Implementation: pull in `globset` crate (~1.5 MB compiled, no transitive
heavyweights). If at PR time `globset` looks too heavy, fall back to a
~30 LOC hand-rolled matcher covering exactly these four operators.

#### Resolution flow in `referrer::resolve_target_ids`

1. Compile glob pattern.
2. Walk `class_name_id_by_class_id`, resolve each entry to dotted FQ name,
   test against pattern.
3. For every class that matches: collect its instance ids via the existing
   pass-1B sweep.
4. Combine into the same `target_ids: AHashSet<u64>` the rest of the
   pipeline expects.

#### Output header

```
Found 4 classes matching glob 'com.nexio.tv.domain.model.*':
  - com.nexio.tv.domain.model.MetaPreview         (123,382 instances)
  - com.nexio.tv.domain.model.CatalogRow          (28,697 instances)
  - com.nexio.tv.domain.model.ResolvedDisplayItem (35,011 instances)
  - com.nexio.tv.domain.model.ArtworkBundle       (18,442 instances)

Found 205,532 target instance(s) for glob 'com.nexio.tv.domain.model.*'

=== Direct referrers (1-hop) ===
  ...
```

Edge cases:

- Glob matches zero classes → `error: glob 'X' matched no classes; check available classes with: heaptrail -i x.hprof -t 1000`
- Glob matches all classes (`**`) → warn but proceed; useful for "find anything held by X".
- One target instance covered by multiple matched classes — counted once
  per holder hit (the natural behavior of the union'd `target_ids` set).

JSON output: `target_label` becomes `"glob:com.nexio.tv.domain.model.*"`,
plus a new `matched_classes: [{name, instance_count}, ...]` array.

## 4. Testing Strategy

### Unit tests (`#[cfg(test)]`)

**A — thread/frame resolution:**
- Pass1 indexer: synthetic record stream `[StartThread{serial=1, name_id=10}, Utf8{id=10, str="main"}, RootJavaFrame{obj=42, thread_serial=1, frame_idx=0}, StackTrace{serial=99, ...}]` → assert `pass1.thread_name_by_serial[1] == "main"`, `pass1.root_thread_meta_by_id[42].thread_serial == 1`.
- Frame resolution: `StackFrame{method_name_id=20, source_file_name_id=21, line_number=478}` + utf8 lookups → `ResolvedFrame { method: "commitToMemory", source_file: "SharedPreferencesImpl.java", line: 478 }`.
- Render fallback: thread-meta absent → "(thread metadata not in dump)" line.

**C — allocation sites:**
- Recorder captures `AllocationSites { allocation_sites: [...] }` into the recorder's `Vec<AllocationSite>`.
- Render: synthetic dump with one `AllocationSite{class_serial=5, bytes_allocated=1<<20, instances_allocated=2, stack_trace_serial=99}` plus class/utf8/trace records → output contains `"1.00 MiB  /  2 instances"` and the resolved method name.
- Empty case: zero AllocationSites → summary hint says "not present"; `--allocation-sites` mode exits with the informative error.

**D — array index:**
- `paths::find_first_holder` against synthetic `ObjectArrayDump{elements: [10, 11, 12, 13]}` looking for `target=12` → `PathStep.array_index == Some(2)`.
- Render: `Object[][2]` appears in the output line.

**F — glob matching:**
- Compile `com.foo.*` and assert it matches `com.foo.Bar` but not `com.foo.bar.Baz` (the `**` distinction).
- Compile `**$Itr` and assert it matches `java.util.ArrayList$Itr` but not `java.util.HashMap$KeyIterator`.
- Zero-match: `nonexistent.*` → resolve_target_ids returns the dedicated "matched no classes" error.
- Multi-match: `com.example.*` → `Pass1Resolved.matched_classes` lists the matched classes with instance counts.

### Integration tests (fixtures)

**Existing fixtures** stay green:
- `test-heap-dumps/hprof-64.bin` (3 MB JVM dump): all v0.7.x golden tests still pass byte-for-byte for `summary`, `--find-referrers`, `--paths-from-id`, `--diff-from`/`--diff-to`. Add new gold lines for the summary AllocationSites hint (`"AllocationSites: not present"` is expected here).
- `test-heap-dumps/hprof-32.bin` (Android 32-bit ID dump): same.

**New fixture: alloc-tracked dump.** Need a dump captured with
`am profile start` so we can integration-test `--allocation-sites`
end-to-end. Two options:
1. Capture from an Android emulator/device, commit a small (<5 MB) dump.
2. Synthesize one — write a tiny Rust helper that emits a hand-rolled HPROF
   stream with known AllocationSites records, run heaptrail against the
   in-memory bytes.

Recommend (1) for realism, (2) as fallback if no emulator is convenient.
This is the main fixture-acquisition task for v0.8.0; everything else uses
existing fixtures.

**End-to-end smoke** on `/tmp/heap-snapshot-fix.hprof` (1.0.3, 186 MiB):
- `--paths-from-id <large-char[]> --max-depth 12` → terminator includes
  thread name when chain hits a Java frame.
- `--find-referrers --target-glob 'com.nexio.tv.domain.model.*' --hops 1`
  → outputs the matched-classes header listing 4+ classes, hop-1 counts
  attributed correctly.
- `--allocation-sites` (probably hits the "not present" path on this dump
  unless re-captured under tracing).

### CI

`cargo clippy --workspace --all-targets --all-features -- -D warnings`
already gates lint cleanliness — anything we add must pass on rustc 1.95+.
No new CI steps needed.

### Performance regression check

Manual smoke (recorded in PR description): summary on the 235 MiB dump
should stay under 200 ms. Pass-1 enrichment for thread/frame maps should
add ≤ 10 ms (the records are sparse).

## 5. Rollout & Versioning

### Branch strategy

Sequential PRs onto `master`, single v0.8.0 tag at the end:

| Order | PR | Files touched |
|-------|-----|---------------|
| 1 | `feat: thread name + stack frame resolution (A)` | `referrer.rs`, `paths.rs` |
| 2 | `feat: Object[] index in paths (D)` | `paths.rs` |
| 3 | `feat: --target-glob for find-referrers (F)` | `args.rs`, `referrer.rs`, `Cargo.toml` |
| 4 | `feat: --allocation-sites mode + summary hint (C)` | `args.rs`, `main.rs`, `result_recorder.rs`, new `allocation_sites.rs` |
| 5 | `chore: bump to 0.8.0; update README + USERGUIDE + plugin SKILL.md` | docs only |

Order chosen so each PR is independently testable and reviewable;
A → D → F → C goes from smallest blast radius to largest.

### Version bump

`Cargo.toml` `0.7.1 → 0.8.0` in PR #5. Minor bump per semver: new flags,
no breaking changes.

### Dependencies

- New: `globset` (~1.5 MB compiled, no heavy transitives). Added in PR #3.
- No other new deps.

### Documentation surface

Each feature lands its docs in the same PR that introduces it:
- `README.md` — short reference + USERGUIDE pointer for the new flag/mode.
- `USERGUIDE.md` — full section per feature with worked example output.
- `plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md` —
  operating-mode entry per feature, integrated into the standard triage
  workflow.

Plugin/marketplace JSONs don't change. Skill description doesn't need new
triggers — `--allocation-sites`, `--target-glob`, etc., are flags on
existing entry points the skill already covers conceptually.

### Release flow

After PR #5 merges:

```bash
cd ~/Scripts/heaptrail
git tag -a v0.8.0 -m "v0.8.0 — thread/stack metadata + allocation sites + glob targeting + path indices"
git push fork v0.8.0
gh release create v0.8.0 --repo johnneerdael/heaptrail \
  --title "heaptrail v0.8.0" -F /tmp/release-notes-080.md
```

The release workflow (just-fixed in v0.7.1) handles binary builds + crates.io publish.

## 6. Risk Register

| Risk | Mitigation |
|------|------------|
| `globset` pulls a heavier transitive than expected | Audit at PR #3; if size grows the binary by >100 KB, fall back to a hand-rolled glob matcher (~30 LOC for `*`/`?`/`**`/character classes). |
| Allocation-tracked fixture not capturable on this machine | Synthesize a minimal hprof byte stream in-test (option 2 from §4); integration coverage moves to unit-test depth. |
| Thread/stack-frame maps blow memory on a pathological dump (e.g. 1M unique stack frames) | Cap at first 100K frames with a warning; rare in practice (Android dumps have ≤ 50K). |
| `--target-glob` + `--target` both passed | Compile-time `clap` mutual-exclusion via `conflicts_with`. |
| Order-of-records: `RootJavaFrame` seen before `StartThread` | Already two-pass — Pass 1A indexes everything before Pass 2 resolves; no ordering issue. |

## 7. Out of Scope (Explicit)

- Sort options on `--allocation-sites` beyond default (`bytes_allocated`).
  v0.8.1 candidate: `--sort instances|live-bytes|live-instances`.
- Filtering allocation sites by class glob. Combinable later with F's
  pattern infrastructure.
- Showing thread names in `summary` mode (only `paths` for v0.8.0).
- Fancy globset features: `{a,b}` braces, `!negation`. Stick to
  `*`/`**`/`?`/`[abc]`.
- Anything from feature B (content preview) or E (dominator tree). Those
  ship in v0.9.0 and v1.0.0 respectively, with their own specs.

## 8. References

- Source feedback document: in-conversation transcript 2026-05-09.
- Validated hotfix predecessor: `docs/superpowers/specs/`-equivalent of
  v0.7.1 (Android HPROF 1.0.3 extension tags) — implemented in commit
  `504c6d0`.
- Existing v0.7.0 architecture: `docs/superpowers/plans/2026-05-09-merge-hprof-analyze-into-slurp.md`.
- HPROF format reference: `art/runtime/hprof/hprof.cc` in AOSP for the
  Android extensions; OpenJDK `heapDumper.cpp` for the standard format.
