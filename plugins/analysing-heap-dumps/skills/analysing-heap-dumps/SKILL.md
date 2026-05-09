---
name: analysing-heap-dumps
description: Use when investigating .hprof files (Android or JVM heap dumps), diagnosing memory leaks, asking 'what holds class X / object id Y', measuring GC churn between two snapshots, or chasing OutOfMemoryError. Triggers include `am dumpheap`, `jmap -dump`, 'memory leak', 'retained size', 'heap is huge', 'find what is holding this object'. heaptrail is the recommended CLI; do not reach for Eclipse MAT first on large dumps.
---

# Analyzing heap dumps with heaptrail

## Overview

`heaptrail` is a streaming CLI for triaging Java/Android `.hprof` heap
dumps. It is the right first reach for: **histogram, retainer chains,
path-to-root, and snapshot diff**. It supports both 4-byte (Android) and
8-byte (JVM) identifier formats, and processes dumps **larger than RAM** at
~1.5 GB/s.

**Source:** https://github.com/johnneerdael/heaptrail (master, version 0.9.0+).

## When to use

- A `.hprof` file appears in the conversation or working tree.
- The user mentions Android heap dump, `am dumpheap`, JVM heap, `jmap`,
  Perfetto Java heap dump, OOM, OutOfMemoryError.
- The user asks "what's using all the memory", "what's holding X alive",
  "why is this class allocating so much", "is this a leak".
- The user has two heap dumps and wants the delta.

**Do NOT default to Eclipse MAT.** MAT is a GUI tool that requires loading
the entire dump into RAM and is slow on multi-hundred-MB dumps.
`heaptrail` answers ~95% of triage questions in seconds at the CLI.
Reach for MAT only when you need a full dominator tree or retained-size
calculation, after `heaptrail` has narrowed the question.

**Do NOT run `hprof-conv`** on modern Android hprof files. Modern Android
dumps from `am dumpheap` are already the standard format `heaptrail`
expects — `hprof-conv` is only needed for legacy Dalvik dumps from
pre-ART devices.

## Step 0: Ensure heaptrail is installed

Before running any heaptrail command, check it's on PATH:

```bash
command -v heaptrail >/dev/null 2>&1 && heaptrail --version
```

If missing, install (requires Rust toolchain):

```bash
# Check Rust is present
command -v cargo >/dev/null 2>&1 || {
  echo "Rust not installed. Run: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  exit 1
}

# Install latest from johnneerdael's fork (has --find-referrers, --paths-from-id, --diff-from)
cargo install --git https://github.com/johnneerdael/heaptrail

# Verify
heaptrail --version    # should report 0.7.0 or newer
```

If `heaptrail` is found but version is `0.6.3` or older, **upgrade**:
```bash
cargo install --git https://github.com/johnneerdael/heaptrail --force
```
The 0.6.3 build (from crates.io) is summary-only and lacks the referrer
tracing and diff modes you need for retainer queries.

If `~/.cargo/bin` is not on PATH after install, instruct the user to add
it (bash/zsh: `export PATH="$HOME/.cargo/bin:$PATH"` in `~/.bashrc` or
`~/.zshrc`; fish: `fish_add_path ~/.cargo/bin`).

## The five operating modes

`heaptrail` has one default mode (summary) and three opt-in modes
selected by mutually-exclusive flags. Pick exactly one of:
`--find-referrers`, `--paths-from-id`, or `--diff-from`/`--diff-to`.

### 1. `summary` (default) — what's in the heap?

```bash
heaptrail -i heap.hprof -t 20
```

**What it tells you:** Top-N classes by total shallow size, instance count
per class, largest single instance per class, and a list of object ids
for the largest array instances (these are the inputs you feed into
`--paths-from-id` next).

**Wall time:** ~150 ms on a 235 MB Android dump.

**Use this first** to identify the dominant class(es) and any single huge
allocation worth chasing.

### 2. `--find-referrers <target>` — who's holding it?

```bash
# Targeting a class FQ-name (every instance of that class)
heaptrail -i heap.hprof --find-referrers java.util.ArrayList --hops 2 --top 30

# Targeting a specific object id (for one giant instance)
heaptrail -i heap.hprof --find-referrers id:1661812752 --hops 1
```

**What it tells you:** Direct + multi-hop holders (instance fields, array
slots, class statics) that point at any of the target instances.

**Feature F (v0.8.0 — glob targeting):** add `--target-glob '<pattern>'`
to find referrers of every class matching a shell-style glob in one
pass:

```bash
heaptrail -i heap.hprof --target-glob 'com.example.**' --hops 2
heaptrail -i heap.hprof --target-glob '**$Itr'           # all iterator inner classes
```

`*` stays within a package level; `**` crosses levels. Mutually
exclusive with `--find-referrers`. Output prepends a list of matched
classes with live instance counts.

*Engineering use case:* you see 35K instances spread across `*$Itr` /
`*$KeyIterator` / `*$EntryIterator` and want to chase "what's
producing all these iterators?" — that's six exact-match
`--find-referrers` runs vs one `--target-glob '**$Itr'`.

**Hops:**
- `--hops 1` — direct holders only.
- `--hops 2` (default) — also chain through `Object[]` arrays. **This is
  usually where the real diagnosis lives** — hop 1 will report
  `java.lang.Object[][]` as the dominant holder; hop 2 reveals the
  `ArrayList.elementData` (or similar) that backs those arrays.
- `--hops 3` — three-link chain.

**Wall time:** ~480 ms (hops 1) / ~730 ms (hops 2) on a 235 MB dump.

**Target syntax:** dotted FQ class names (`java.util.ArrayList`,
`java.util.LinkedHashMap$LinkedHashMapEntry`); inner classes use `$`.
Object ids are passed as `id:<u64>` or bare `<u64>`.

### 3. `--paths-from-id <u64>` — chain to a GC root

```bash
heaptrail -i heap.hprof --paths-from-id 1661812752 --max-depth 12
```

**What it tells you:** A single chain of holders walking from `<id>` up
toward a GC root. Each hop is the *first* record (file order) whose body
or array elements reference the current id. Terminates with one of:

- `→ reached GC root: <kind>` — chain successfully terminated. Kinds:
  `RootJniGlobal`, `RootJniLocal`, `RootJavaFrame`, `RootStickyClass`,
  `RootMonitorUsed`, `RootThreadObject`, `RootThreadBlock`,
  `RootNativeStack`, `RootUnknown`.
- `→ stopped at --max-depth (chain may continue)` — bump `--max-depth`.
- `→ orphan: no holder found in dump` — chain exhausted.

**Feature A (v0.8.0 — thread/frame surfacing):** when the chain
terminates at a thread-owned root (`RootJavaFrame`, `RootThreadObject`,
`RootJniLocal`, `RootJniMonitor`), the output now includes the thread
name and (for `RootJavaFrame`) the top frame's method/file/line:

```
  → reached GC root: RootJavaFrame
        thread "pool-7-thread-2"
        at android.app.SharedPreferencesImpl$EditorImpl.commitToMemory(SharedPreferencesImpl.java:478)
```

*Engineering use case (the bug that motivated this):* a 72 MiB `char[]`
held by a `StringBuilder.value` rooted at `RootJavaFrame` — the chain
told us the holder but not which thread or method was running. Without
this, the next step was source-grepping `gson.toJson|moshi.*toJson` for
candidates. With it, the line "thread pool-7-thread-2" pinpoints the
SharedPreferences flusher immediately.

**Feature D (v0.8.0 — Object[] element index):** array hops now show
the matched slot (e.g. `via java.lang.Object[][12]` instead of the
generic `via java.lang.Object[][]`). Lets you correlate the slot back
to a known position in a paged result, sparse cache, or backing
`ArrayList.elementData`.

**Wall time:** ~325 ms per hop (each hop is one streaming pass).

### 4. `--diff-from <a> --diff-to <b>` — snapshot diff (churn signal)

```bash
heaptrail --diff-from before.hprof --diff-to after.hprof --diff-by count --top 20
heaptrail --diff-from before.hprof --diff-to after.hprof --diff-by bytes
```

**What it tells you:** Per-class delta in instance count and shallow
bytes between two snapshots. The strongest GC-churn signal a pair of
static dumps can give: classes whose instance count grew most are
allocation hot-paths. Sort by `count` (default) for short-lived
allocations or `bytes` for size growth. Zero-delta classes are filtered.

### 5. `--allocation-sites` — per-class allocation stack traces (v0.8.0, feature C)

```bash
heaptrail -i heap.hprof --allocation-sites --top 20
```

**What it tells you:** For dumps captured with allocation tracking
(`am profile start <pid>` before `am dumpheap`), prints the top-N
allocation sites with their resolved Java stack traces — the most
direct path from "this class is huge" to "this is the line that
allocated it."

**Summary always reports presence/absence** so you know whether
re-capturing under tracking is worth it:

```
AllocationSites: 12,453 sites across 287 records (run with --allocation-sites for stack traces)
AllocationSites: not present (capture with `am profile start <pid>`)
```

When the dump has no alloc-tracking data, `--allocation-sites` exits
with the same hint as an error.

*Engineering use case:* a 72 MiB `char[]` whose holder chain
terminated at a Gson serializer. `--allocation-sites` would have shown
the exact `Moshi.fromJson` / `JsonAdapter` frame that allocated it,
skipping the source-grep through `gson.toJson|moshi.*toJson` candidates.
First thing to reach for after `summary` when alloc-tracking is on.

**Wall time:** ~150 ms on a 235 MiB dump (data is loaded by the same
slurp pass; the resolution overhead is dominated by class+frame map
lookups, not parsing).

### 6. `--preview-bytes N` — content preview (v0.9.0, feature B)

Global flag (not its own mode — applies to summary, `--paths-from-id`,
`--find-referrers id:N`, and `-l`). When set, primitive arrays
(`char[]`, `byte[]`, etc.) are previewed inline:

```bash
heaptrail -i heap.hprof -t 5 --preview-bytes 200
heaptrail -i heap.hprof --paths-from-id <u64> --preview-bytes 200
heaptrail -i heap.hprof --find-referrers id:<u64> --preview-bytes 200
heaptrail -i heap.hprof -l --preview-bytes 200
```

**What it tells you:** UTF-8 / UTF-16 BE / hex auto-detect of the
first N bytes per primitive array, surfaced under each "Largest array
instances" entry, primitive-array path hops, find-referrers targets,
and (with `-l`) a standalone-large-arrays listing keyed off
`--list-arrays-min-bytes` (default 1024 bytes).

*Engineering use case:* a 72 MiB `char[]` whose holder chain ended at
a Gson `StringBuilder` — heaptrail told us *who* held it but not
*what* it contained. Identifying the content as a SharedPreferences
XML blob required `adb shell` to find a 7.86 MB file on disk plus a
source-grep for `gson\.toJson|moshi.*toJson`. With
`--preview-bytes 200`, the inline
`<?xml version="1.0" encoding='utf-8' standalone='yes' ?>...home_catalog_snapshot...`
would have identified it instantly. This is the canonical
"big primitive array of unknown content" disambiguation pattern —
SharedPreferences XML, cached JSON, decoded log buffers, and image
magic bytes all read at a glance.

**Wall time / memory:** opt-in parser pass retains at most N bytes
per primitive array. Memory bound: N × array-count. ~260 MiB peak on
a 200 MiB Android dump with N=200; negligible on typical JVM dumps.
Default 0 (off) — every existing CLI invocation produces byte-identical
output unless `--preview-bytes` is set.

### `--json` for any mode

Append `--json` to any of the above. Writes `heaptrail-<mode>-<ts>.json`
alongside the text output. Use this when piping to `jq`, dashboards, or
CI gates.

## The standard triage workflow

Given a fresh heap dump and a vague "memory looks bad" report:

1. **`summary`** → identify the dominant class. Note any large
   `largest_object_id` entries. Also note the `AllocationSites:` hint
   line — if the dump *was* captured under allocation tracking, jump to
   step 6 first; it short-circuits the rest.
2. **`--find-referrers <dominant-class> --hops 2`** → find the
   collection / field that retains it. Hop 2 usually pinpoints the actual
   holder (e.g. `ArrayList.elementData` in 28k ArrayLists). For families
   of related classes (`*$Itr`, `com.example.model.*`), use
   `--target-glob` instead — one pass instead of one per class.
3. **`--find-referrers <holder-class> --hops 1 --json`** → pivot to
   identify which field of the holder owns the over-allocation. JSON
   output makes this easy to script.
4. **`--paths-from-id <largest-object-id>`** → for any single giant
   allocation, walk to its GC root to confirm whether it's leaked or
   bounded. The terminator includes thread name + top frame for
   thread-owned roots; array hops show the matched element index.
5. (Optional) **`--diff-from before --diff-to after`** between two
   captures of the same process to confirm whether the class is actively
   growing under load.
6. (When alloc-tracking was on) **`--allocation-sites --top 20`** →
   jump straight from "this class is huge" to "this is the exact line
   that allocated it." Most direct shortcut available; skips the
   source-grep step entirely. Use as a *replacement* for steps 2–4
   when the data is present.
7. (Optional) **`--preview-bytes 200`** → append to any of the above
   surfaces inline content for `char[]` / `byte[]` arrays. Use when a
   chain leads to a giant primitive array and the holder identity
   alone doesn't say what it contains (the canonical "is this big
   `char[]` a SharedPreferences XML, a cached JSON blob, a log buffer,
   or a decoded image?" disambiguation).

## Capturing an Android heap dump

```bash
# Find the target process
adb shell ps -A | grep com.example.myapp

# Capture (writes to device, then pull)
adb shell am dumpheap <pid> /data/local/tmp/heap.hprof
adb pull /data/local/tmp/heap.hprof
```

For two-snapshot diff:
```bash
adb shell am dumpheap <pid> /data/local/tmp/before.hprof
# (run the suspect interaction)
adb shell am dumpheap <pid> /data/local/tmp/after.hprof
adb pull /data/local/tmp/before.hprof
adb pull /data/local/tmp/after.hprof
heaptrail --diff-from before.hprof --diff-to after.hprof
```

For JVM (server) dumps: `jmap -dump:format=b,file=heap.hprof <pid>`.

## Quick reference

| Goal | Command |
|------|---------|
| Top-N classes | `heaptrail -i heap.hprof -t 20` |
| Direct holders of a class | `heaptrail -i heap.hprof --find-referrers <class> --hops 1` |
| Holders through Object[] | `heaptrail -i heap.hprof --find-referrers <class> --hops 2` |
| Holders of one specific object | `heaptrail -i heap.hprof --find-referrers id:<u64>` |
| Chain to a GC root | `heaptrail -i heap.hprof --paths-from-id <u64>` |
| Compare two snapshots | `heaptrail --diff-from a.hprof --diff-to b.hprof` |
| Glob targeting (family of classes) | `heaptrail -i heap.hprof --target-glob 'com.foo.**'` |
| Allocation sites (when alloc-tracked) | `heaptrail -i heap.hprof --allocation-sites --top 20` |
| Inline content preview for `char[]`/`byte[]` | append `--preview-bytes 200` to summary, paths, find-referrers, or `-l` |
| JSON sidecar | append `--json` to any of the above |
| List all UTF-8 strings | `heaptrail -i heap.hprof -l` |

## Common mistakes

| Mistake | Reality |
|---------|---------|
| Reaching for Eclipse MAT first | MAT loads the dump into RAM and is slow on multi-hundred-MB dumps. Use `heaptrail` for triage, MAT only if you need a full dominator tree. |
| Running `hprof-conv` | Modern Android hprof from `am dumpheap` is already the standard format. `hprof-conv` is only for legacy pre-ART Dalvik dumps. |
| Stopping at `--hops 1` | Hop 1 reports `Object[][]` for any class held in a collection — uninformative. **Always run with `--hops 2`** for class targets. |
| Trying to install via `cargo install heaptrail` | The crates.io build is 0.6.3 — summary-only, no referrer tracing. Use `cargo install --git https://github.com/johnneerdael/heaptrail`. |
| Forgetting the `id:` prefix | `--find-referrers 1661812752` and `--find-referrers id:1661812752` both work; `--find-referrers <class-name>` is FQ-name targeting. Bare digits are always treated as ids. |
| Combining `--find-referrers` with `--diff-from` | Modes are mutually exclusive. Run them as separate commands. |
| Using slash form for class names | Names are dotted (`java.util.ArrayList`), not slash-form (`java/util/ArrayList`). The HPROF stores slash form internally; `heaptrail` accepts and displays dotted. Inner classes: `Outer$Inner`. |

## Performance reference

Numbers from a 235 MB Android dump (32-bit ids):

| Mode | Wall time |
|------|-----------|
| `summary` | 158 ms |
| `--find-referrers --hops 1` | 484 ms |
| `--find-referrers --hops 2` | 729 ms |
| `--find-referrers id:N` | 346 ms |
| `--paths-from-id` (depth 9) | 2.93 s |
| `--diff-from = --diff-to` | 320 ms (page-cache warm) |

Wall cost scales linearly with file size and number of streaming passes.
`--paths-from-id` is the only mode where wall time grows with depth
(`O(depth × file_size)`); cap with `--max-depth` if needed.

## What heaptrail doesn't do (yet)

- **Allocation-site stack traces** from "Record memory allocations"
  captures are parsed but not surfaced. Fall back to Android Studio's
  profiler GUI for that view.
- **Full retained-size / dominator tree** (Eclipse MAT's specialty).
  `--find-referrers --hops N` covers most of the diagnostic value
  without the cost.
- **Class-name regex / wildcard** in `--find-referrers`. Targets are
  exact FQ-name strings or numeric ids.

## Further reading

- `USERGUIDE.md` in the repo: hands-on guide with worked Android-dump
  examples (https://github.com/johnneerdael/heaptrail/blob/master/USERGUIDE.md).
- `README.md` for cheat-sheet and install instructions.
- `docs/feature-retainer-tracing.md` for the design rationale of the
  multi-pass referrer engine.
