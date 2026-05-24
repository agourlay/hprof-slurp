---
name: analysing-heap-dumps
description: Use when investigating .hprof files, Android or JVM heap dumps, obfuscated Android release heap reports, memory leaks, retained size, OutOfMemoryError, GC churn, playback memory growth, or questions like 'what holds class X / object id Y'. Triggers include `am dumpheap`, `jmap -dump`, Perfetto Java heap dumps, `.hprof`, 'heap is huge', and 'find what is holding this object'.
---

# Analyzing heap dumps with heaptrail

## Overview

`heaptrail` is a streaming CLI for triaging Java/Android `.hprof` heap
dumps. It is the right first reach for: **histogram, retainer chains,
path-to-root, content preview, retained leak suspects, and snapshot diff**.
It supports both 4-byte (Android) and 8-byte (JVM) identifier formats, and
processes dumps **larger than RAM** at ~1.5 GB/s.

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

## Non-negotiable defaults

Use these defaults unless the user explicitly asks for a raw/minimal command:

1. **Always deobfuscate Android release dumps.** If `--mapping <PATH>` is
   known, include it on every heaptrail command. If no mapping path is known
   but an Android project/device is available, use
   `--auto-mapping --project-root <DIR> --package <PACKAGE> [--serial <SERIAL>]`.
   If neither is possible, state that raw class names may be obfuscated before
   interpreting output. Do not silently analyze release dumps without mapping.
2. **Use `--preview-bytes 200` on first-pass triage.** Primitive arrays are
   often the memory story. Running summary without preview only gives
   `byte[]`/`char[]`; preview identifies JSON, XML, protobuf-like binary,
   image signatures, and log buffers immediately. On the Nexio 113 MiB dump,
   mapped summary+preview took ~0.42 s and exposed a 117 KiB `char[]` as JSON.
3. **For Android vague-leak triage, prefer retained suspects first.**
   Run `--leak-suspects --exclude-soft-weak --preview-bytes 200 --top 5`
   with mapping. It costs more than summary but gives retained dominators plus
   paths. On the Nexio dump it took ~4.1 s and surfaced `FfmpegVideoDecoder`,
   `DurableArtworkDecisionCache`, and Media3 renderer queues immediately.
4. **For 3+ ordered captures, use `--diff-series`, not repeated pair diffs.**
   It shows per-step deltas, first-to-last totals, and monotonic growth in one
   report. Use `--diff-by bytes` for memory-growth investigations.
5. **For Media3/playback/cache questions, use grouped holders immediately.**
   Prefer `--target-glob 'androidx.media3.**' --hops 2 --group-holders` with
   mapping. On the Nexio dump it took ~0.52 s and collapsed 2,869 Media3
   targets into actionable owner/field rows such as
   `LoadControl$Parameters.timeline`, `playerId`, and `mediaPeriodId`.
6. **For new Android captures, prefer `android-capture`.** Use it instead of
   hand-written `adb` commands when capturing from a live device. It records
   PID, focused-window evidence, exact commands, local file size,
   AllocationSites presence, and mapping metadata when `--auto-mapping` is
   enabled. That transcript is often as valuable as the dump when another
   agent must reason about capture quality later.

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

**Mapping guardrail:** If output includes short names such as `x7.k`, `a3.t`,
`p1.j`, `d1.q2`, `zh.l1`, or similar, stop and rerun with `--mapping` or
`--auto-mapping`. On the Nexio dump, raw summary showed `x7.k`, `a3.t`, and
`p1.j`; the mapped command resolved them to
`com.google.gson.internal.LinkedTreeMap$Node`,
`androidx.compose.ui.semantics.SemanticsNode`, and
`androidx.compose.runtime.snapshots.SnapshotIdSet`.

### 1. `summary` (default) — what's in the heap?

```bash
heaptrail -i heap.hprof --mapping mapping.txt --preview-bytes 200 -t 20
```

**What it tells you:** Top-N classes by total shallow size, instance count
per class, largest single instance per class, and a list of object ids
for the largest array instances (these are the inputs you feed into
`--paths-from-id` next).

**Wall time:** ~150 ms on a 235 MB Android dump; mapped preview summary was
~0.42 s on the 113 MiB Nexio Android dump.

**Use this first** when the user asks for heap composition or you need object
ids for follow-up commands. Keep `--preview-bytes 200` on by default so
primitive arrays identify their content in the same run.

### 2. `--find-referrers <target>` — who's holding it?

```bash
# Targeting a class FQ-name (every instance of that class)
heaptrail -i heap.hprof --mapping mapping.txt --find-referrers java.util.ArrayList --hops 2 --top 30

# Targeting a specific object id (for one giant instance)
heaptrail -i heap.hprof --mapping mapping.txt --find-referrers id:1661812752 --hops 1 --preview-bytes 200
```

**What it tells you:** Direct + multi-hop holders (instance fields, array
slots, class statics) that point at any of the target instances.

**Feature F (v0.8.0 — glob targeting):** add `--target-glob '<pattern>'`
to find referrers of every class matching a shell-style glob in one
pass:

```bash
heaptrail -i heap.hprof --mapping mapping.txt --target-glob 'com.example.**' --hops 2 --group-holders
heaptrail -i heap.hprof --mapping mapping.txt --target-glob '**$Itr' --hops 2 --group-holders
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

**Group noisy tables by default:** add `--group-holders` when targeting a
family (`--target-glob`) or playback/media/cache classes. It groups by owner
family, holder class, and field, which is more useful to agents than scanning
hundreds of individual holder rows.

**Target syntax:** dotted FQ class names (`java.util.ArrayList`,
`java.util.LinkedHashMap$LinkedHashMapEntry`); inner classes use `$`.
Object ids are passed as `id:<u64>` or bare `<u64>`.

### 3. `--paths-from-id <u64>` — chain to a GC root

```bash
heaptrail -i heap.hprof --mapping mapping.txt --paths-from-id 1661812752 --max-depth 12 --preview-bytes 200
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

**When prioritizing impact**, add `--retained-size` so the start id and hops
show retained bytes. On the Nexio dump,
`--paths-from-id 364390216 --retained-size --preview-bytes 200 --mapping ...`
took ~0.87 s and showed the `FfmpegVideoDecoder` root retained 11.71 MiB.

### 4. `--diff-from <a> --diff-to <b>` — snapshot diff (churn signal)

```bash
heaptrail --diff-from before.hprof --diff-to after.hprof --mapping mapping.txt --diff-by count --top 20
heaptrail --diff-from before.hprof --diff-to after.hprof --mapping mapping.txt --diff-by bytes
```

**What it tells you:** Per-class delta in instance count and shallow
bytes between two snapshots. The strongest GC-churn signal a pair of
static dumps can give: classes whose instance count grew most are
allocation hot-paths. Sort by `count` (default) for short-lived
allocations or `bytes` for size growth. Zero-delta classes are filtered.

### 4b. `--diff-series <a> <b> <c>...` — ordered playback/state timeline (v1.3.0)

```bash
heaptrail --diff-series launch.hprof home.hprof play.hprof stop.hprof soak.hprof \
  --mapping mapping.txt --diff-by bytes --top 30 \
  --json --json-out reports/playback-series.json
```

**What it tells you:** adjacent step deltas, first-to-last totals, and
monotonic growth candidates across 3+ snapshots. This is the right default
when the user has ordered captures from playback, navigation, start/stop, or
soak states. On the Nexio before/before/after validation series it took ~0.56 s
and identified monotonic growth in `char[]`, `int[]`, `java.lang.Long`,
`androidx.compose.runtime.snapshots.SnapshotIdSet`, and related Compose rows.

Use `--native-context meminfo.txt` when the user also captured
`adb shell dumpsys meminfo <package>`; it annotates text and JSON with Java
Heap, Native Heap, Graphics, GL, and TOTAL PSS. It does not change Java heap
diff calculations.

### 5. `--allocation-sites` — per-class allocation stack traces (v0.8.0)

```bash
heaptrail -i heap.hprof --mapping mapping.txt --allocation-sites --top 20
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

**Use only when present:** first-pass summary prints `AllocationSites:`. On the
Nexio dumps it was `not present`, so running `--allocation-sites` is lower value
than leak suspects, diff-series, grouped holders, or paths.

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
heaptrail -i heap.hprof --mapping mapping.txt -t 20 --preview-bytes 200
heaptrail -i heap.hprof --mapping mapping.txt --paths-from-id <u64> --preview-bytes 200
heaptrail -i heap.hprof --mapping mapping.txt --find-referrers id:<u64> --preview-bytes 200
heaptrail -i heap.hprof --mapping mapping.txt -l --preview-bytes 200
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
Default 0 (off) for the CLI, but agents should usually opt in with 200 bytes
on first-pass Android leak triage because it prevents a second command when
large `byte[]`/`char[]` rows dominate.

### 7. `--retained-size` — dominator-tree retained sizes (v1.0.0, feature E)

Global flag. When set, summary's class table re-sorts by retained
heap and adds a `retained` column; a "Largest retained instances"
hot list of `(object_id, class, retained_bytes)` follows;
`--paths-from-id` annotates each hop with `(retained=<size>)`;
`--find-referrers` adds a `class retained` column to holder rows.

```bash
heaptrail -i heap.hprof --mapping mapping.txt --retained-size --preview-bytes 200 -t 20
heaptrail -i heap.hprof --mapping mapping.txt --paths-from-id <u64> --retained-size --preview-bytes 200
heaptrail -i heap.hprof --mapping mapping.txt --find-referrers <class-or-id> --retained-size
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
heaptrail -i heap.hprof --mapping mapping.txt --paths-from-id <id> --exclude-soft-weak --preview-bytes 200
heaptrail -i heap.hprof --mapping mapping.txt --leak-suspects --exclude-soft-weak --preview-bytes 200
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
heaptrail -i heap.hprof --mapping mapping.txt --leak-suspects --exclude-soft-weak --preview-bytes 200 --top 5
heaptrail -i heap.hprof --mapping mapping.txt --leak-suspects=0.10 --exclude-soft-weak --preview-bytes 200
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
heaptrail -i heap.hprof --mapping mapping.txt --paths-from-id <any-instance-of-target> \
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
heaptrail -i heap.hprof --mapping mapping.txt --bitmaps -t 20
```

*Engineering use case:* bitmaps dominate Android heaps but are
invisible to the class-name view. A 12 MiB `byte[]` is just
"another big primitive array" until you see it's a 4096×4096
ARGB_8888 bitmap held by a `RecyclerView.ViewHolder`. Handles both
pre-O (Java-heap pixel data via `mBuffer`) and O+ (native pixel
data sized via `width × height × bpp`).

**Wall time / memory:** single instance scan filtered by Bitmap class id.
Negligible cost; ~400 KiB working memory for ~5K bitmaps. Use when summary,
UI context, or the user points at bitmap/image pressure. On dumps where
`android.graphics.Bitmap` was never loaded, it exits quickly with an actionable
message; on the Nexio playback dumps this made it lower value than Media3,
leak-suspects, and diff-series probes.

### `--json` for any mode

Append `--json` to any of the above. Writes `heaptrail-<mode>-<ts>.json`
alongside the text output. Use this when piping to `jq`, dashboards, or
CI gates.

## The standard triage workflow

Given a fresh heap dump and a vague "memory looks bad" report:

0. **Resolve mapping first.** Use explicit `--mapping` if known; otherwise
   try `--auto-mapping`. If neither is available, say the report may contain
   obfuscated names and treat conclusions as provisional.
1. **Android vague leak / OOM:** start with
   `--leak-suspects --exclude-soft-weak --preview-bytes 200 --top 5`.
   This answers "what dominates retained heap and what holds it?" in one
   command. Then use `--paths-from-id <suspect-id> --retained-size` only when
   a suspect path needs deeper confirmation.
2. **Need composition / IDs:** run mapped
   `summary --preview-bytes 200 -t 20`. Use this to collect dominant classes,
   large primitive-array object ids, and the `AllocationSites:` presence hint.
   Do not run a no-preview summary first; it usually forces a second command.
3. **3+ ordered captures:** run mapped `--diff-series ... --diff-by bytes`.
   Use monotonic growth candidates as the shortlist. If there are exactly two
   captures, use `--diff-from/--diff-to --diff-by bytes`.
4. **Playback / Media3 / cache ownership:** run mapped
   `--target-glob 'androidx.media3.**' --hops 2 --group-holders`. For another
   subsystem, replace the glob with that package family. Grouped rows are the
   report agents should summarize first.
5. **Dominant class holder question:** run mapped
   `--find-referrers <class> --hops 2 --group-holders` for class families or
   collection-heavy output. Hop 2 is the default because hop 1 often only says
   `Object[]`.
6. **Single giant primitive array:** run mapped
   `--find-referrers id:<id> --hops 1 --preview-bytes 200`, then
   `--paths-from-id <id> --preview-bytes 200 --retained-size` if the holder
   path matters.
7. **AllocationSites present:** run mapped `--allocation-sites --top 20` as a
   replacement for holder/path guessing; it points at allocation stack traces.
   If summary says `AllocationSites: not present`, skip it.
8. **Many leaked instances of one class:** use
   `--paths-from-id <any-instance> --merge-paths --retained-size` to fold shared
   paths into one tree.
9. **Bitmap/image pressure:** use `--bitmaps` only when the dump loaded
   `android.graphics.Bitmap` or the user/context points at image memory.

## Capturing an Android heap dump

Prefer the helper for new Android captures:

```bash
heaptrail android-capture \
  --serial <adb-serial> \
  --package com.example.myapp \
  --out artifacts/heap-captures \
  --foreground \
  --auto-mapping \
  --project-root <android-project-root>
```

Use `--foreground` for app-state-sensitive leaks so the transcript proves the
focused package/window at capture time. Use `--auto-mapping` during capture
when possible so the transcript records the selected mapping path, source, and
hash. The resulting local `.hprof` path is the input for follow-up `heaptrail`
analysis commands.

Use `--allocation-sites` only when the user can recapture the workload and
needs "where was this allocated?" stack traces:

```bash
heaptrail android-capture \
  --package com.example.myapp \
  --out artifacts/heap-captures \
  --foreground \
  --auto-mapping \
  --project-root <android-project-root> \
  --allocation-sites
```

AllocationSites cannot be recovered from an ordinary already-captured
`am dumpheap` file. If summary says `AllocationSites: not present`, skip
`--allocation-sites` analysis and use holders/paths/diffs instead. Allocation
tracking can perturb runtime behavior, so use it for targeted reproductions
rather than every baseline capture.

There is no special bitmap capture flag. Capture the app after navigating to
the image-heavy screen and waiting for images to load, then run mapped
`--bitmaps` on the resulting HProf. Java HProf records live
`android.graphics.Bitmap` objects; native pixel buffers are not directly
contained in the dump, so pair bitmap output with `dumpsys meminfo` and
`--native-context` when native/graphics pressure matters.

Manual fallback when `android-capture` cannot be used:

```bash
# Find the target process
adb shell ps -A | grep com.example.myapp

# Capture (writes to device, then pull)
adb shell am dumpheap <pid> /data/local/tmp/heap.hprof
adb pull /data/local/tmp/heap.hprof
```

Manual AllocationSites fallback:

```bash
adb shell am profile start <pid> /data/local/tmp/heaptrail-alloc.trace
adb shell am dumpheap <pid> /data/local/tmp/heap.hprof
adb shell am profile stop <pid>
adb pull /data/local/tmp/heap.hprof
heaptrail -i heap.hprof --mapping mapping.txt --allocation-sites --top 20
```

For two-snapshot diff:
```bash
adb shell am dumpheap <pid> /data/local/tmp/before.hprof
# (run the suspect interaction)
adb shell am dumpheap <pid> /data/local/tmp/after.hprof
adb pull /data/local/tmp/before.hprof
adb pull /data/local/tmp/after.hprof
heaptrail --diff-from before.hprof --diff-to after.hprof --mapping mapping.txt --diff-by bytes
```

For ordered playback/state captures, prefer a series:

```bash
heaptrail --diff-series launch.hprof home.hprof play.hprof stop.hprof soak.hprof \
  --mapping mapping.txt --diff-by bytes --json --json-out playback-series.json
```

For JVM (server) dumps: `jmap -dump:format=b,file=heap.hprof <pid>`.

## Quick reference

| Goal | Command |
|------|---------|
| Top-N classes and large array content | `heaptrail -i heap.hprof --mapping mapping.txt --preview-bytes 200 -t 20` |
| Best Android vague-leak first pass | `heaptrail -i heap.hprof --mapping mapping.txt --leak-suspects --exclude-soft-weak --preview-bytes 200 --top 5` |
| Repeatable Android capture | `heaptrail android-capture --package <app> --out artifacts/heap-captures --foreground --auto-mapping --project-root <dir>` |
| Allocation-site capture | append `--allocation-sites` to `android-capture`, then analyze with mapped `--allocation-sites` |
| Holders through Object[] | `heaptrail -i heap.hprof --mapping mapping.txt --find-referrers <class> --hops 2 --group-holders` |
| Holders of one specific object | `heaptrail -i heap.hprof --mapping mapping.txt --find-referrers id:<u64> --hops 1 --preview-bytes 200` |
| Chain to a GC root with impact | `heaptrail -i heap.hprof --mapping mapping.txt --paths-from-id <u64> --retained-size --preview-bytes 200` |
| Compare two snapshots | `heaptrail --diff-from a.hprof --diff-to b.hprof --mapping mapping.txt --diff-by bytes` |
| Compare playback/state series | `heaptrail --diff-series a.hprof b.hprof c.hprof --mapping mapping.txt --diff-by bytes` |
| Glob targeting / subsystem family | `heaptrail -i heap.hprof --mapping mapping.txt --target-glob 'com.foo.**' --hops 2 --group-holders` |
| Media3 playback ownership | `heaptrail -i heap.hprof --mapping mapping.txt --target-glob 'androidx.media3.**' --hops 2 --group-holders` |
| Allocation sites (only when present) | `heaptrail -i heap.hprof --mapping mapping.txt --allocation-sites --top 20` |
| Fold N paths-to-root into one tree | append `--merge-paths` to mapped `--paths-from-id <any-instance> --retained-size` |
| Bitmap pixel-byte accounting (Android) | `heaptrail -i heap.hprof --mapping mapping.txt --bitmaps -t 20` |
| JSON sidecar | append `--json` to any of the above |
| List all UTF-8 strings/large arrays | `heaptrail -i heap.hprof --mapping mapping.txt -l --preview-bytes 200` |

## Common mistakes

| Mistake | Reality |
|---------|---------|
| Reaching for Eclipse MAT first | MAT loads the dump into RAM and is slow on multi-hundred-MB dumps. Use `heaptrail` for triage, MAT only if you need a full dominator tree. |
| Running `hprof-conv` | Modern Android hprof from `am dumpheap` is already the standard format. `hprof-conv` is only for legacy pre-ART Dalvik dumps. |
| Stopping at `--hops 1` | Hop 1 reports `Object[][]` for any class held in a collection — uninformative. **Always run with `--hops 2`** for class targets. |
| Running raw commands on release dumps | Obfuscated names hide the diagnosis. Always include `--mapping`/`--auto-mapping`; if output has names like `x7.k` or `p1.j`, rerun mapped. |
| Hand-writing ADB capture when provenance matters | Use `heaptrail android-capture`; it writes a transcript with PID, focus evidence, commands, dump size, AllocationSites presence, and mapping metadata. |
| Expecting AllocationSites from an ordinary dump | Allocation stack traces must be captured up front with allocation tracking, e.g. `android-capture --allocation-sites`. If summary says not present, use holders/paths/diffs. |
| Looking for a bitmap capture mode | `--bitmaps` analyzes an HProf; it does not change capture. Navigate to the image-heavy screen, wait for images to load, capture normally, then run mapped `--bitmaps`. |
| Running summary without preview | It identifies `byte[]`/`char[]` but not content. Use `--preview-bytes 200` by default on first-pass triage. |
| Running pairwise diffs for playback captures | For 3+ ordered snapshots, `--diff-series` gives the whole timeline and monotonic growth candidates in one run. |
| Scanning huge Media3 referrer output manually | Use `--target-glob 'androidx.media3.**' --group-holders` so owner families and holder fields surface first. |
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
| mapped summary + `--preview-bytes 200` on Nexio 113 MiB dump | 0.42 s |
| mapped `--diff-series` before/before/after on Nexio dumps | 0.56 s |
| mapped Media3 `--target-glob ... --group-holders` on Nexio dump | 0.52 s |
| mapped `--leak-suspects --exclude-soft-weak --preview-bytes 200` on Nexio dump | 4.1 s |

Wall cost scales linearly with file size and number of streaming passes.
`--paths-from-id` is the only mode where wall time grows with depth
(`O(depth × file_size)`); cap with `--max-depth` if needed.

## What heaptrail doesn't do (yet)

- **Interactive graph browsing / OQL.** Use Eclipse MAT when the question needs
  ad-hoc graph pivots rather than repeatable CLI probes.
- **Native heap attribution.** `--native-context` records `dumpsys meminfo`
  totals for correlation, but Java HProf cannot explain native allocations by
  stack. Use Perfetto/Android Studio/native profilers for that.
- **Allocation stacks unless captured.** `--allocation-sites` needs a dump
  captured under allocation tracking; ordinary `am dumpheap` files usually say
  `AllocationSites: not present`.

## Further reading

- `USERGUIDE.md` in the repo: hands-on guide with worked Android-dump
  examples (https://github.com/johnneerdael/heaptrail/blob/master/USERGUIDE.md).
- `README.md` for cheat-sheet and install instructions.
- `docs/feature-retainer-tracing.md` for the design rationale of the
  multi-pass referrer engine.
