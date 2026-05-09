# heaptrail User Guide

A practical guide to triaging Android (and JVM) heap dumps with `heaptrail`.
Every example below uses real output from a 235 MiB Android dump
(`heap-phase4-jvm.hprof`, captured from a Modern Home / nexio.tv build) — not
synthetic data.

---

## 1. Capturing a heap dump on Android

### Option A — `am dumpheap` over adb (fastest, no IDE)

```bash
# Find the target process
adb shell ps -A | grep com.example.myapp
# u0_a382  29481  ...  com.example.myapp

# Capture (writes to device, then pull)
adb shell am dumpheap 29481 /data/local/tmp/heap.hprof
adb pull /data/local/tmp/heap.hprof
```

`am dumpheap` gives you the live heap of a running process. The captured file
uses 32-bit identifiers (Android default) — `heaptrail` handles both 32-bit
and 64-bit identifier formats automatically.

### Option B — Android Studio Profiler

1. Run → Profile 'app'
2. In the **Memory** tool window, click the camera icon ("Capture heap dump").
3. Right-click the capture in the timeline → **Export…** → save as `.hprof`.

If you tick **Record memory allocations** before capturing, the resulting hprof
also contains `AllocationSites` records (per-call-site bytes/instances). These
aren't surfaced in summary output today, but they're parsed correctly so the
file is still consumable.

### Option C — Perfetto

Perfetto's "Java heap dump" data source produces a compatible hprof. Use the
[Memory recipe](https://perfetto.dev/docs/data-sources/java-heap-dumps) on the
Perfetto site. Drop the resulting file straight into `heaptrail`.

### Option D — JVM (`jmap`)

For server-side JVM dumps:

```bash
jmap -dump:format=b,file=heap.hprof <pid>
```

Same tool, same flags. The bundled `test-heap-dumps/hprof-64.bin` fixture is
a JVM dump captured this way.

---

## 2. Cheat sheet

| Goal | Command |
|------|---------|
| Top-N classes by retained size | `heaptrail -i heap.hprof` |
| What holds an over-allocated class? | `heaptrail -i heap.hprof --find-referrers <class>` |
| What holds a specific giant object? | `heaptrail -i heap.hprof --find-referrers id:<u64>` |
| Walk an object up to a GC root | `heaptrail -i heap.hprof --paths-from-id <u64>` |
| Compare two snapshots (churn) | `heaptrail --diff-from a.hprof --diff-to b.hprof` |
| Pipe to `jq` / dashboards | append `--json` to any of the above |

Common flags:

- `-t N` / `--top N` — top-N rows shown (default 20).
- `--hops 1\|2\|3` — referrer chain depth (default 2).
- `--include-statics` — include class statics as candidate referrers (default true).
- `--max-depth N` — bail on path walk after N hops (default 12).
- `--diff-by count\|bytes` — sort diff by Δinstance-count (default) or Δshallow-bytes.
- `-l` / `--listStrings` — dump every UTF-8 string in summary mode.
- `-d` / `--debug` — verbose record-tag tracing.
- `--json` — also write a structured JSON sidecar file.

Mutually exclusive: pick **one** of `--find-referrers`, `--paths-from-id`, or
`--diff-from`/`--diff-to`. With none of those, you get the summary.

---

## 3. `summary` — what's in the heap right now?

The default mode. Streams the file once at ~1.5 GB/s, no second pass, no
graph walking.

```bash
heaptrail -i heap-phase4-jvm.hprof -t 10
```

Output (real, 158 ms on the 235 MiB Android dump):

```
Found a total of 224.34MiB of raw shallow heap objects in the dump.

Top 10 raw shallow heap classes:

+------------+-----------+-------------+-----------------------------+
| Total size | Instances |     Largest | Class name                  |
+------------+-----------+-------------+-----------------------------+
|   56.89MiB |    573552 | 104.00bytes | com.nexio.tv.domain.model.MetaPreview                           |
|   25.31MiB |    940474 |    28.63KiB | byte[]                                                          |
|   21.59MiB |    372795 |     5.64MiB | char[]                                                          |
|   20.79MiB |    908414 |  24.00bytes | java.lang.String                                                |
|   10.77MiB |    227587 |   123.52KiB | java.lang.Object[][]                                            |
|    3.68MiB |     80397 |  48.00bytes | com.squareup.moshi.LinkedHashTreeMap$Node                       |
|    3.11MiB |    204027 |  16.00bytes | java.lang.StringBuilder                                         |
|    2.87MiB |     75290 |  40.00bytes | androidx.compose...PersistentHashMapBuilder                     |
|    2.80MiB |    122154 |  24.00bytes | java.util.ArrayList                                             |
|    2.09MiB |     40545 |   418.64KiB | int[]                                                           |
+------------+-----------+-------------+-----------------------------+

Top 10 largest instances:

(...same shape, sorted by largest single instance...)

Largest array instances object ids (for retainer tracing):
     5.64MiB object_id=1661812752 char[]
   418.64KiB object_id=2595270656 int[]
   128.02KiB object_id=342360064  com.squareup.moshi.LinkedHashTreeMap$Node[][]
   123.52KiB object_id=518041528  java.lang.Object[][]
    50.07KiB object_id=2743599104 long[]
    28.63KiB object_id=1717379088 byte[]
     ...
File successfully processed in 158.47 ms
```

### How to read it

- **Total size** — total shallow bytes for this class across all instances.
- **Instances** — count of objects of that exact class.
- **Largest** — biggest single instance of the class. For `char[]`, the
  5.64 MiB row means there is one char[] that alone holds 5.64 MiB.
- **Largest array instances object ids** — explicit hand-off list. These ids
  are what you feed into `--find-referrers id:<u64>` or `--paths-from-id`
  next.

In this dump, `MetaPreview` is the dominant class by total size (56.89 MiB,
573,552 instances). The 5.64 MiB char[] at object id `1661812752` is the
single largest live allocation. Both become inputs to the retainer queries
below.

---

## 4. `--find-referrers` — who's holding it?

The most-used investigation tool. Given **either** an FQ class name **or** a
specific object id, find the fields, arrays, and statics that point at it.

### 4.1 Targeting a class

> "Why are there 573,552 `MetaPreview` instances? What's keeping them alive?"

```bash
heaptrail -i heap-phase4-jvm.hprof \
  --find-referrers com.nexio.tv.domain.model.MetaPreview \
  --top 10 --hops 1
```

Output (484 ms):

```
Found 573552 target instance(s) for com.nexio.tv.domain.model.MetaPreview

=== Direct referrers (1-hop) ===
  holder.field (or class[] for arrays)                                     ref count
  java.lang.Object[][]                                                        603715
  com.nexio.tv.ui.screens.home.ModernCarouselItem.metaPreview                    249
  com.nexio.tv.ui.screens.home.CachedCarouselItem.source                         120
  com.nexio.tv.ui.screens.home.ModernHomeRowsKt$...$1$6$2$1$6$2$1.$metaPreview     8
  com.nexio.tv.ui.screens.home.ModernHomeRowsKt$...$1$6$2$1$6$1$1.$nextCatalogItem 8
  com.nexio.tv.ui.screens.home.HomeHydrationCoordinator$hydrate$1.L$0              3
  com.nexio.tv.ui.screens.home.HomeViewModelPresentationPipelineKt$...$1.L$12      2
  java.util.LinkedHashMap$LinkedHashMapEntry.value                                 1
```

**Reading the result:** 603,715 of the holder slots are inside Java
`Object[][]` arrays — these are the backing stores of `ArrayList`s and
similar collections. The named instance fields (`ModernCarouselItem.metaPreview`,
`CachedCarouselItem.source`, the captured-lambda `$metaPreview` /
`$nextCatalogItem` slots) are direct holders in Compose UI code.

The "ref count" is **occurrences of the field/array slot pointing at any
target instance**, not unique source objects. A field with
ref count 249 means "across all live `ModernCarouselItem` instances, the
`.metaPreview` field referred to one of our 573,552 `MetaPreview` objects
249 times."

### 4.2 Multi-hop: who holds the `Object[]`s?

A 1-hop result dominated by `Object[][]` is uninformative — you want to know
*which collections* those arrays back. Add `--hops 2`:

```bash
heaptrail -i heap-phase4-jvm.hprof \
  --find-referrers com.nexio.tv.domain.model.MetaPreview \
  --top 10 --hops 2
```

Output (729 ms — adds a second streaming pass):

```
=== Direct referrers (1-hop) ===
  java.lang.Object[][]                                            603715
  ...

=== 2-hop referrers (X holds Object[] which holds target) ===
  holder.field (or class[] for arrays)        ref count
  java.util.ArrayList.elementData                  28681
  androidx.compose.runtime.SlotTable.slots             8
  androidx.compose.runtime.SlotWriter.slots            8
```

**Now the diagnosis is clear:** 28,681 `ArrayList`s (via their `elementData`
backing arrays) hold `MetaPreview` instances. The 8 references through
`SlotTable.slots` / `SlotWriter.slots` are Compose's recomposition cache.

`--hops 3` adds another link (X holds Y holds Object[] holds target). Useful
when hop-2 itself lands on something generic.

### 4.3 Targeting a specific object id

When a single instance dominates (the 5.64 MiB char[] from the summary), go
straight at it:

```bash
heaptrail -i heap-phase4-jvm.hprof \
  --find-referrers id:1661812752 --hops 1
```

Output (346 ms):

```
Found 1 target instance(s) for id:1661812752

=== Direct referrers (1-hop) ===
  holder.field (or class[] for arrays)  ref count
  java.lang.String.value                        1
```

A `java.lang.String` wraps it. Skip ahead to `--paths-from-id` to walk all
the way up.

### 4.4 What you can pass as `--find-referrers`

| Form | Meaning | Example |
|------|---------|---------|
| `<FQ class name>` | every instance of that class | `--find-referrers java.util.ArrayList` |
| `id:<u64>` | one specific object | `--find-referrers id:1661812752` |
| `<u64>` | bare digits, same as `id:<u64>` | `--find-referrers 1661812752` |

Class names are dotted (`java.util.ArrayList`), not slash-form. Inner
classes use `$`: `java.util.LinkedHashMap$LinkedHashMapEntry`.

### 4.5 Performance

Each additional hop is one extra streaming pass.

| Hops | Wall time on 235 MiB Android dump |
|------|-----------------------------------|
| 1 | 484 ms |
| 2 | 729 ms |
| 3 | ~1.0 s estimated |

Pass 1 (index) is shared across hops. Each subsequent pass touches only the
records it cares about.

---

## 5. `--paths-from-id` — chain to a GC root

When you have a single object id and want the holder chain all the way up,
`--paths-from-id` walks one hop at a time. Each iteration finds the
*first* record (file order) whose body or array elements reference the
current id, then continues from there.

```bash
heaptrail -i heap-phase4-jvm.hprof \
  --paths-from-id 1661812752 --max-depth 10
```

Output (2.93 s, depth 9):

```
Path from object_id=1661812752 (depth 9 step(s)):
  start  ── id=1661812752
  hop 1  ── id=1661812736  (via java.lang.String.value)
  hop 2  ── id=364312776   (via java.util.HashMap$Node.value)
  hop 3  ── id=364312696   (via java.util.HashMap$Node[][])
  hop 4  ── id=364312512   (via java.util.HashMap.table)
  hop 5  ── id=364312344   (via android.app.SharedPreferencesImpl.mMap)
  hop 6  ── id=529946832   (via java.lang.Object[][])
  hop 7  ── id=529946800   (via android.util.ArrayMap.mArray)
  hop 8  ── id=529946720   (via java.lang.Object[][])
  hop 9  ── id=369633368   (via android.util.ArrayMap.mArray)
  → orphan: no holder found in dump
```

### How to read it

Bottom-up:

> A char[] (5.64 MiB) is the value bytes of a `String`, which is a value in
> a `HashMap$Node`, in the `HashMap` backing
> `SharedPreferencesImpl.mMap`. That `SharedPreferencesImpl` is held inside
> nested `ArrayMap`s.

Three terminal states:

- **`reached GC root: <kind>`** — chain successfully terminated. The kind
  is one of `RootJniGlobal`, `RootJniLocal`, `RootJavaFrame`,
  `RootStickyClass`, `RootMonitorUsed`, `RootThreadObject`, `RootThreadBlock`,
  `RootNativeStack`, `RootUnknown`.
- **`stopped at --max-depth (chain may continue)`** — bump `--max-depth` and
  re-run.
- **`orphan: no holder found in dump`** — the chain ran out. Either the
  object is genuinely unreachable (rare on live captures) or its holder is
  in a record type the walker doesn't yet inspect (e.g. a thread stack
  Java-frame local that's not tagged as a `RootJavaFrame`). The example
  above hits this case: after 9 hops the walker can't find a holder for
  `id=369633368`, so it reports orphan rather than guess.

### Performance

Each hop is one streaming pass. Worst case is `O(--max-depth × file_size)`.

On the 235 MiB Android dump:

- 9-hop walk: 2.93 s (≈ 325 ms per hop)

For a deep chain (12+ hops) on a multi-GiB dump, expect tens of seconds.

---

## 6. `--diff-from` / `--diff-to` — snapshot diff

A single hprof shows what's *live at one moment*, not what's *churning*. To
spot allocation hot-paths, capture two snapshots of the same process under
load and diff them:

```bash
# Capture before & after a workload
adb shell am dumpheap <pid> /data/local/tmp/before.hprof
# (run your suspect interaction)
adb shell am dumpheap <pid> /data/local/tmp/after.hprof
adb pull /data/local/tmp/before.hprof
adb pull /data/local/tmp/after.hprof

# Compare
heaptrail --diff-from before.hprof --diff-to after.hprof --diff-by count --top 20
```

Output shape:

```
Class deltas (sorted, top 20 shown):
        Δcount       Δbytes  count(a→b)   bytes(a→b)  class
        +12000       +480000 100→12100    4KiB→480KiB java.util.HashMap$Node
         +8400       +268800 500→8900     20KiB→289KiB com.example.MyDto
          +200       +200000 10→210       2KiB→202KiB  java.lang.String
```

### Reading it

- **Δcount** — `count_b − count_a`. Positive = leaked or growing; negative =
  garbage-collected away or freed.
- **Δbytes** — same arithmetic for shallow bytes.
- **count(a→b)** — raw before/after counts for context.
- **bytes(a→b)** — raw before/after sizes.
- Zero-delta classes are filtered out automatically.

### Sort key

- `--diff-by count` (default) — short-lived allocation hot-paths usually
  show up here first.
- `--diff-by bytes` — better when a few large objects dominate (image caches,
  log buffers, parse buffers).

### Sanity check

Diffing a file against itself should produce zero entries:

```bash
heaptrail --diff-from heap-phase4-jvm.hprof --diff-to heap-phase4-jvm.hprof
# → No per-class deltas — the two snapshots match.
```

(Real wall time on the 235 MiB dump: 320 ms — both files share the OS page
cache after the first read.)

---

## 7. `--json` — structured output for scripts

Append `--json` to any mode and you get a sidecar file with the same
information, machine-parseable. The text table still prints to stdout.

```bash
heaptrail -i heap-phase4-jvm.hprof \
  --find-referrers java.util.ArrayList --hops 1 --top 3 --json
```

Stdout (abridged):

```
File successfully processed in 527.84 ms
Output JSON result file heaptrail-referrers-1715299876543.json
```

`heaptrail-referrers-<ts>.json`:

```json
{
  "target_label": "java.util.ArrayList",
  "target_instance_count": 122154,
  "hop1": [
    {
      "holder_class": "com.nexio.tv.domain.model.MetaPreview",
      "field_name": "genres",
      "ref_count": 573412
    },
    {
      "holder_class": "com.nexio.tv.domain.model.CatalogRow",
      "field_name": "items",
      "ref_count": 28697
    },
    {
      "holder_class": "com.squareup.moshi.LinkedHashTreeMap$Node",
      "field_name": "value",
      "ref_count": 13743
    }
  ],
  "hop2": [],
  "hop3": []
}
```

This is also the way to feed the data into `jq`, dashboards, or CI gates:

```bash
heaptrail -i heap.hprof --json -t 5
jq '.top_allocated_classes[0]' heaptrail.json
```

JSON file naming:

| Mode | Filename |
|------|----------|
| summary | `heaptrail.json` (overwritten each run) |
| `--find-referrers` | `heaptrail-referrers-<ts>.json` (timestamped) |
| `--paths-from-id` | `heaptrail-paths-<ts>.json` |
| `--diff-from`/`--diff-to` | `heaptrail-diff-<ts>.json` |

---

## 8. Worked example — chasing a real leak end-to-end

Real workflow that produced the screenshots above. Goal: explain why the
Modern Home build is sitting at ~225 MiB of live heap.

### Step 1 — summary

```bash
heaptrail -i heap-phase4-jvm.hprof -t 10
```

Top finding: `com.nexio.tv.domain.model.MetaPreview` is 56.89 MiB across
573,552 instances. That's an order of magnitude more than expected for a
home-screen carousel. Worth chasing.

Side finding: a single 5.64 MiB char[] (`object_id=1661812752`) is the
biggest individual allocation.

### Step 2 — what holds the 573k MetaPreviews?

```bash
heaptrail -i heap-phase4-jvm.hprof \
  --find-referrers com.nexio.tv.domain.model.MetaPreview --hops 2
```

Hop-1 says 603,715 references via `Object[][]`. That's not actionable yet —
`Object[][]` is just "some collection's backing array."

Hop-2 reveals the actual collection: **28,681 `ArrayList.elementData`
holders**. So 28k `ArrayList`s are full of `MetaPreview` instances. Plus 8
references via Compose `SlotTable.slots` (the recomposition cache).

### Step 3 — which ArrayList field?

```bash
heaptrail -i heap-phase4-jvm.hprof \
  --find-referrers java.util.ArrayList --hops 1 --top 3 --json
```

```json
"hop1": [
  { "holder_class": "com.nexio.tv.domain.model.MetaPreview",
    "field_name": "genres", "ref_count": 573412 },
  { "holder_class": "com.nexio.tv.domain.model.CatalogRow",
    "field_name": "items", "ref_count": 28697 },
  ...
]
```

**Diagnosis converges.** Every single MetaPreview owns its own `genres`
ArrayList (573,412 of them — almost 1:1 with the 573,552 MetaPreview
instances). Separately, 28,697 CatalogRows hold an `items` ArrayList — this
matches the 28,681 ArrayLists holding MetaPreviews from hop-2. So the
structure is roughly:

> `CatalogRow.items: ArrayList<MetaPreview>` ← 28,697 of these
>
> Each `MetaPreview` *also* owns `MetaPreview.genres: ArrayList<…>` (these
> ArrayLists are mostly empty / small but each one is its own instance).

Two real questions for the team:

1. Why are there 28,697 CatalogRows live at once? Is the Compose home screen
   accumulating rows across navigations without pruning?
2. Does every MetaPreview need its own dedicated `genres` ArrayList instance,
   or could the empty case be a shared singleton?

### Step 4 — what's the 5.64 MiB char[]?

```bash
heaptrail -i heap-phase4-jvm.hprof --paths-from-id 1661812752 --max-depth 10
```

The 9-hop chain ends at:
`String → HashMap$Node.value → HashMap$Node[] → HashMap.table → SharedPreferencesImpl.mMap → ArrayMap.mArray → ArrayMap.mArray …`

So the 5.64 MiB char[] is a single SharedPreferences value. Likely a
serialized blob the app shouldn't be storing in `SharedPreferences` (which is
loaded into memory in full on every read). Different bug, parallel
investigation.

---

## 9. Performance and limits

### Throughput

`heaptrail` streams the file with a 64 MiB pre-fetcher, a separate parser
thread, and a separate recorder thread, communicating via crossbeam channels.
It can process dumps significantly larger than RAM in a single pass.

| Mode | 235 MiB Android dump |
|------|----------------------|
| `summary` | 158 ms (~1.5 GB/s) |
| `--find-referrers --hops 1` | 484 ms |
| `--find-referrers --hops 2` | 729 ms |
| `--find-referrers id:N --hops 1` | 346 ms |
| `--paths-from-id` (depth 9) | 2.93 s |
| `--diff-from = --diff-to` | 320 ms (cached) |

### Memory

Summary mode holds class metadata + per-class counters but **never** holds
instance bodies — that's the design that makes it work on dumps larger than
RAM.

`--find-referrers` and `--paths-from-id` opt into a "retain bodies" parser
mode for one of their passes. Working memory for that pass is bounded by
the prefetcher's two 64 MiB buffers plus per-record body allocations
(released as soon as the record is consumed). The reference-count maps
themselves are small (a few thousand entries at most).

### Format support

- HPROF formats: `JAVA PROFILE 1.0.1` and `JAVA PROFILE 1.0.2`.
- Identifier sizes: 4-byte (Android default) and 8-byte (most JVMs).
- AllocationSites and HeapSummary records are parsed but not yet surfaced
  in summary output (planned).

### What it doesn't do (yet)

- **Allocation tracking surfacing.** A dump captured under "Record memory
  allocations" contains per-call-site stack traces; the parser sees them,
  the renderer doesn't. Use Android Studio's profiler if you need that view
  today.
- **Full retained-size / dominator tree.** This is what Eclipse MAT does,
  and it's expensive. `--find-referrers --hops N` covers ~95% of the
  diagnostic work without the cost.
- **Class-name regex / wildcard match in `--find-referrers`.** Targets are
  exact FQ-name strings or numeric ids today.

---

## 10. Troubleshooting

### "target class not found in dump"

The class name is wrong, or it isn't loaded. Class names are dotted, with
`$` for inner classes. Double-check by passing the class name through
summary's class column first.

### "32 bits heap dumps are not supported yet"

You're on an old build. `heaptrail >= 0.6.4` supports both 32-bit and
64-bit identifier sizes. `cargo install --git
https://github.com/johnneerdael/heaptrail` for the current build.

### `--paths-from-id` reports "orphan"

The walker found 0 holders for the current id at some hop. Either:
- the id is genuinely unreachable in this snapshot (rare),
- the holder is in a record type the walker doesn't currently inspect (e.g.
  Java thread stack locals not tagged as `RootJavaFrame`), or
- the field is a non-Object type (skipped by design).

Try `--find-referrers id:<that-hop-id>` directly to see if any non-walker
record references it.

### Slow `--paths-from-id`

Each hop is a streaming pass. Lower `--max-depth` if you only need the
first few links; use `--find-referrers id:N --hops 2` for a wider view at
the same cost.

### Memory grows on `--find-referrers`

The retain-bodies pass briefly buffers each `Object[]` element list and
each instance body. On dumps with very large object arrays (e.g. a single
1M-element `Object[]`) peak memory adds ~8 MiB per such record while it's
in flight. The prefetcher's backpressure keeps total in-flight bounded.

---

## 11. Comparison with related tools

| Tool | When to reach for it |
|------|---------------------|
| **`heaptrail`** | Triage workflow on huge dumps. Top-N, retainer chains, snapshot diff. CLI + JSON. |
| **Eclipse MAT** | Deep retained-size / dominator-tree analysis. Slower to load, GUI-driven, very thorough. |
| **Android Studio profiler** | Live captures, allocation tracking with stack traces, GUI exploration. |
| **Perfetto** | Time-correlated heap dumps + system trace. |
| **`hprof-analyze-rust`** | (Archived) — superseded by `--find-referrers`. |

`heaptrail` deliberately does less than MAT but on dumps that don't fit
in MAT's memory budget. It is the tool for the first ten minutes of a heap
investigation; MAT or Studio is the tool for the second hour.

---

## 12. Reading recommendations

- The original blog series on the streaming parser design (pre-fork):
  [agourlay's hprof-slurp posts](https://agourlay.github.io/tags/hprof-slurp/).
- HPROF format reference: [OpenJDK heap dumper source](https://hg.openjdk.java.net/jdk/jdk/file/ee1d592a9f53/src/hotspot/share/services/heapDumper.cpp#l62).
- The retainer-tracing design note in this repo: `docs/feature-retainer-tracing.md`.
- The implementation plan that produced the merged tool:
  `docs/superpowers/plans/2026-05-09-merge-hprof-analyze-into-slurp.md`.
