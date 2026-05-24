---
name: analysing-heap-dumps
description: Use when investigating .hprof files (Android or JVM heap dumps), deobfuscating Android release heap reports, diagnosing memory leaks, asking 'what holds class X / object id Y', measuring GC churn between two snapshots, or chasing OutOfMemoryError. Triggers include `am dumpheap`, `jmap -dump`, 'memory leak', 'retained size', 'heap is huge', 'find what is holding this object'. heaptrail is the recommended CLI; do not reach for Eclipse MAT first on large dumps.
---

# Analyzing heap dumps with heaptrail

## Overview

`heaptrail` is a streaming CLI for triaging Java/Android `.hprof` heap
dumps. It is the right first reach for: **histogram, retainer chains,
path-to-root, and snapshot diff**. It supports both 4-byte (Android) and
8-byte (JVM) identifier formats, and processes dumps **larger than RAM** at
~1.5 GB/s.

**Source:** https://github.com/johnneerdael/heaptrail (master, version 1.3.0+).

> **v1.3.0 (playback debugging foundation):** use `--diff-series` for 3+
> ordered snapshots, `--group-holders` to collapse noisy Media3/cache holder
> tables, `--native-context` to attach `dumpsys meminfo`, and
> `--paths-from-id` root metadata fallback when thread names/frames are absent.

> **v1.2.0 (Android release-build deobfuscation):** use `--mapping` with the
> exact R8/ProGuard mapping file for the installed build, or `--auto-mapping`
> from the Android project root to query the device package version and select
> the matching local Gradle mapping. Summary/diff JSON keeps
> `obfuscated_class_name` for traceability.

> **v1.1.1 (modern Android dump fix):** v1.1.0 panicked with
> `class id must have a class definition` on dumps that reference
> elided boot-classpath / zygote-shared class ids (common on recent
> ART builds — e.g. `am dumpheap` on Android 14+). v1.1.1 degrades
> gracefully: unknown classes get bare object-header size and an
> `<unknown class #id>` label, plus a single end-of-run stderr
> warning. If a user reports that panic message, upgrade them with
> `cargo install heaptrail --force`.

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

# Install latest published release
cargo install heaptrail

# Verify
heaptrail --version    # should report 1.3.0 or newer
```

If `heaptrail` is found but version is older than `1.3.0`, **upgrade**:
```bash
cargo install heaptrail --force
```
Versions older than 1.3.0 lack playback diff-series and grouped holder
summaries. Versions older than 1.2.0 lack release-build deobfuscation. Versions
0.7–1.1.0 also have the modern-Android
`class id must have a class definition` panic fixed in 1.1.1 — recommend
an upgrade whenever a user reports that crash.

If `~/.cargo/bin` is not on PATH after install, instruct the user to add
it (bash/zsh: `export PATH="$HOME/.cargo/bin:$PATH"` in `~/.bashrc` or
`~/.zshrc`; fish: `fish_add_path ~/.cargo/bin`).

## Operating modes

`heaptrail` has one default mode (summary) and several opt-in modes selected
by mutually-exclusive flags. Pick exactly one of: `--find-referrers`,
`--target-glob`, `--paths-from-id`, `--diff-from`/`--diff-to`,
`--diff-series`, `--allocation-sites`, `--leak-suspects`, or `--bitmaps`.

### Android release-build deobfuscation

```bash
# Explicit mapping file from the exact installed build
heaptrail -i heap.hprof --mapping app/build/outputs/mapping/universalRelease/mapping.txt --leak-suspects

# Auto-select mapping from local Gradle outputs by installed package version
heaptrail -i heap.hprof --auto-mapping --package com.example.app --serial <device>
```

Run `--auto-mapping` from the Android project root, or add
`--project-root <dir>`. It maps class and holder-field names in text
reports; summary and diff JSON also include `obfuscated_class_name` when
a row was renamed.

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

### 5. `--allocation-sites` — per-class allocation stack traces (v0.8.0)

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

### 6. `--preview-bytes N` — content preview (v0.9.0)

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

### 7. `--retained-size` — dominator-tree retained sizes (v1.0.0, feature E)

Global flag. When set, summary's class table re-sorts by retained
heap and adds a `retained` column; a "Largest retained instances"
hot list of `(object_id, class, retained_bytes)` follows;
`--paths-from-id` annotates each hop with `(retained=<size>)`;
`--find-referrers` adds a `class retained` column to holder rows.

```bash
heaptrail -i heap.hprof --retained-size -t 20
heaptrail -i heap.hprof --paths-from-id <u64> --retained-size
heaptrail -i heap.hprof --find-referrers <class-or-id> --retained-size
```

**What it tells you:** the bytes that would actually be reclaimed if
every instance of a class disappeared — the metric Eclipse MAT calls
"retained heap." Closes heaptrail's last gap with MAT for single-shot
triage. Computed via Lengauer–Tarjan dominators on the in-memory
object-reference graph.

*Engineering use case:* the canonical wrapper-vs-subgraph question.
A `ResolvedDisplayItem` is 88 bytes shallow but holds a 12-element
`ResolvedDisplayFieldSlots` and an `ArtworkBundle`. For 35K
instances, shallow size says 3 MB and ranks the class low; retained
size answers whether the *real* cost is 35 MB or 350 MB. Same
diagnostic shape as `--preview-bytes` for v0.9.0 — the data was
always in the dump, but heaptrail wasn't surfacing it. Now it does.

**Reference strengths:** v1.0.0 includes weak / soft / phantom-reference
edges in the graph (strict graph-theoretic dominator definition).
Eclipse MAT's default leak-hunting view excludes those, so MAT and
heaptrail will sometimes report different retained sizes — by design.
Selective exclusion ships in v1.1+ as `--exclude-soft-weak`.

**Wall time / memory:** opt-in, adds ~200 MiB working memory and
~1–3 s wall time on a 200 MiB Android dump. Default off; existing
output unchanged when the flag is unset.

### 8. `--exclude-soft-weak` — drop weak/soft/phantom holders (v1.1.0, feature G)

Modifier flag. Drops outgoing edges from
`java.lang.ref.{Soft,Weak,Phantom}Reference` subclasses across path
walks and the retained-size graph build. Compatible with
`--paths-from-id`, `--find-referrers`, `--retained-size`, and
`--leak-suspects`.

```bash
heaptrail -i heap.hprof --paths-from-id <id> --exclude-soft-weak
heaptrail -i heap.hprof --leak-suspects --exclude-soft-weak
```

*Engineering use case:* on Android, LeakCanary's
`KeyedWeakReference` watchers, the framework's
`WeakReference<Activity>`, and `Reference.discovered` chains all
appear as holders in path walks — the actual *strong* holder is
buried 12 hops underneath. MAT's default leak-hunting view excludes
these automatically; this flag matches that behavior. **The
recommended Android leak-hunting workflow always pairs
`--leak-suspects --exclude-soft-weak`.**

**Wall time / memory:** rebuilds the reference graph minus
Reference-subclass edges. Same cost as `--retained-size` (~1.5 s on
200 MiB Android); typically slightly faster (5–15 % fewer edges).

### 9. `--leak-suspects[=THRESHOLD]` — automated suspect identification (v1.1.0, feature H)

Auto-rank dominators by retained share against a threshold (default
0.05 = 5 %). Per-suspect output: dominator class + object_id,
accumulating-class summary (most-common class in dominated
subtree), content preview (when `--preview-bytes` is set), full
path-to-root via `--paths-from-id`'s walker.

```bash
heaptrail -i heap.hprof --leak-suspects --exclude-soft-weak --preview-bytes 200
heaptrail -i heap.hprof --leak-suspects=0.10  # tighter threshold
```

*Engineering use case:* the canonical "what's wrong with this
dump?" question. `summary --retained-size` requires you to already
know what class to investigate; `--leak-suspects` doesn't. Same
diagnostic shape as MAT's "Leak Suspects" report — terminal-readable,
content-aware (the preview line tells you the suspect *content*
inline, which MAT's Leak Suspects doesn't surface).

**Always shows top-3 even if all below threshold** (flagged
"below threshold"). Useful when one massive cache dominates the
heap and would otherwise be the only entry.

**Wall time / memory:** runs the v1.0.0 dominator pipeline
(~1.5 s) + `dom_children` derivation (~12 MiB / 200 ms) + N path
walks (~5–20 ms each, where N = `--top` capped at threshold count).

### 10. `--merge-paths` — fold paths-to-root for all instances of a class (v1.1.0, feature I)

Modifier on `--paths-from-id`. Resolves all instances of the start
id's class and folds their paths-to-root into a single tree with
`[Nx]` branch counts.

```bash
heaptrail -i heap.hprof --paths-from-id <any-instance-of-target> \
    --merge-paths --retained-size
```

*Engineering use case:* when 47 leaked `MainActivity` instances
share the same holder chain, the *common prefix* tells you "it's
the EventBus" — but `--paths-from-id` walks one instance at a time.
Running it 47 times would force the user to spot the shared
structure by eye. `--merge-paths` collapses the 47 paths into one
tree showing `[47×]` on each shared hop; the EventBus jumps out.

**With `--retained-size`:** branches are dominator-verified
(graph-converged at the same idom). **Without:** textual prefix
match only — usually correct, but the renderer emits a banner
("re-run with --retained-size for graph-verified convergence").

**Wall time / memory:** N × per-instance path cost. ~1 s for 50
leaked Activities on a 200 MiB Android dump.

### 11. `--bitmaps` — Bitmap pixel-byte accounting (v1.1.0, feature J)

Independent of the dominator pipeline. Walks every
`android.graphics.Bitmap` instance, reads
`mWidth`/`mHeight`/`mConfig`/`mBuffer` from each, computes pixel
bytes, and emits a top-N report.

```bash
heaptrail -i heap.hprof --bitmaps -t 20
```

*Engineering use case:* bitmaps dominate Android heaps but are
invisible to the class-name view. A 12 MiB `byte[]` is just
"another big primitive array" until you see it's a 4096×4096
ARGB_8888 bitmap held by a `RecyclerView.ViewHolder`. Handles both
pre-O (Java-heap pixel data via `mBuffer`) and O+ (native pixel
data sized via `width × height × bpp`).

**Wall time / memory:** single instance scan filtered by Bitmap
class id. Negligible cost; ~400 KiB working memory for ~5K
bitmaps. Returns "android.graphics.Bitmap not loaded" error on
non-Android dumps.

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
8. (Optional) **`--retained-size`** → append to summary,
   `--paths-from-id`, or `--find-referrers`. Re-sorts summary's class
   table by dominator-tree retained bytes and adds a "Largest retained
   instances" hot list; annotates path hops with `(retained=<size>)`;
   adds a `class retained` column to find-referrers holder rows. Use
   when a class with high instance count has low shallow size — wrapper
   objects can anchor deep subgraphs whose retained cost is orders of
   magnitude larger. The "is this 35K-instance retention 35 MB or
   350 MB?" prioritization question.
9. (v1.1.0, **recommended for Android first-pass**)
   **`--leak-suspects --exclude-soft-weak --preview-bytes 200`** →
   the "what's wrong with this dump?" entry point. Auto-ranks
   dominators by retained share, picks the accumulating class per
   suspect, walks the path to GC root, and emits a content snippet.
   `--exclude-soft-weak` strips LeakCanary watchers and framework
   `WeakReference` chains so the *strong* holder surfaces.
10. (Optional) **`--merge-paths`** → modifier on `--paths-from-id`.
    When N leaked instances of the same class share a holder chain,
    folds the N paths into one tree with `[N×]` branch counts.
    "47 MainActivity instances all share an EventBus holder" — one
    command instead of 47.
11. (Android-only) **`--bitmaps`** → top-N
    `android.graphics.Bitmap` instances by pixel-byte size. Surfaces
    bitmap leaks invisible to the class-name view (a 12 MiB `byte[]`
    becomes "4096×4096 ARGB_8888 bitmap").

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
| Retained-size triage (wrapper-vs-subgraph) | append `--retained-size` to summary, paths, or find-referrers |
| Filter weak/soft/phantom holders (Android default) | append `--exclude-soft-weak` to any retained-size or path-walk mode |
| Automated leak-suspect identification | `heaptrail -i heap.hprof --leak-suspects --exclude-soft-weak --preview-bytes 200` |
| Deobfuscate Android release heap | append `--mapping mapping.txt` or `--auto-mapping --package <app>` |
| Fold N paths-to-root into one tree | append `--merge-paths` to `--paths-from-id <any-instance>` |
| Bitmap pixel-byte accounting (Android) | `heaptrail -i heap.hprof --bitmaps -t 20` |
| JSON sidecar | append `--json` to any of the above |
| List all UTF-8 strings | `heaptrail -i heap.hprof -l` |

## Common mistakes

| Mistake | Reality |
|---------|---------|
| Reaching for Eclipse MAT first | MAT loads the dump into RAM and is slow on multi-hundred-MB dumps. Use `heaptrail` for triage, MAT only if you need a full dominator tree. |
| Running `hprof-conv` | Modern Android hprof from `am dumpheap` is already the standard format. `hprof-conv` is only for legacy pre-ART Dalvik dumps. |
| Stopping at `--hops 1` | Hop 1 reports `Object[][]` for any class held in a collection — uninformative. **Always run with `--hops 2`** for class targets. |
| Using stale heaptrail | Install or upgrade with `cargo install heaptrail --force`; use 1.3.0+ for playback diff-series and 1.2.0+ for Android release-build deobfuscation. |
| Forgetting the `id:` prefix | `--find-referrers 1661812752` and `--find-referrers id:1661812752` both work; `--find-referrers <class-name>` is FQ-name targeting. Bare digits are always treated as ids. |
| Combining `--find-referrers` with `--diff-from` | Modes are mutually exclusive. Run them as separate commands. |
| Using slash form for class names | Names are dotted (`java.util.ArrayList`), not slash-form (`java/util/ArrayList`). The HPROF stores slash form internally; `heaptrail` accepts and displays dotted. Inner classes: `Outer$Inner`. |
| Seeing `class id must have a class definition` panic | Pre-1.1.1 bug on modern Android dumps where `InstanceDump` references an elided boot-classpath / zygote-shared class id. Upgrade with `cargo install heaptrail --force` to get 1.3.0+, which logs a single warning and continues instead. |

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
