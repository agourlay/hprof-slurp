# heaptrail User Guide

A practical guide to triaging Android (and JVM) heap dumps with `heaptrail`.
Every example below uses real output from a 235 MiB Android dump
(`heap-phase4-jvm.hprof`, captured from a Modern Home / nexio.tv build) — not
synthetic data.

> **Need v1.3.0+ for playback/debugging timelines.** v1.3.0 adds
> `--diff-series`, `--group-holders`, root metadata fallback, and
> `--native-context`. v1.2.0 adds `--mapping` and `--auto-mapping` for
> R8/ProGuard-obfuscated Android heaps.
> v1.1.0 panicked with
> `class id must have a class definition` on modern Android dumps that
> reference elided boot-classpath / zygote-shared class ids (common on
> recent ART builds — `am dumpheap` on Android 14+). v1.1.1 logs a
> single warning and continues. If you hit the panic on a 1.1.0 install,
> upgrade with `cargo install heaptrail --force`.

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

### Option A2 — heaptrail `android-capture` helper

```bash
heaptrail android-capture \
  --serial 192.168.50.98:5555 \
  --package com.example.myapp \
  --out artifacts/heap-captures \
  --foreground
```

The helper wraps the manual ADB flow when a repeatable capture transcript is
more useful than hand-entered commands. It resolves the package PID with
`pidof`, optionally brings the app foreground with `monkey -p <package> 1`,
records focused-window evidence from `dumpsys window`, runs `am dumpheap`,
pulls the resulting `.hprof`, rejects 0-byte local pulls, and writes a
transcript beside the dump.

Use `--allocation-sites` to attempt allocation tracking setup before the dump:

```bash
heaptrail android-capture \
  --package com.example.myapp \
  --out artifacts/heap-captures \
  --allocation-sites
```

After pull, heaptrail runs a cheap summary pass and records whether
`AllocationSites` data is present in the transcript. The helper does not delete
the device-side `/data/local/tmp/*.hprof` by default, so failed or partial
captures remain available for manual inspection.

### Deobfuscating release-build heap reports

Release Android heaps often contain names such as `d1.q2`, `zh.l1`, or `ai.m`.
Use the R8 mapping generated for the exact installed build:

```bash
heaptrail -i after.hprof \
  --mapping app/build/outputs/mapping/universalRelease/mapping.txt \
  --leak-suspects --exclude-soft-weak --preview-bytes 200
```

For local Gradle builds, heaptrail can select the mapping by querying the device
version and matching Gradle APK metadata:

```bash
heaptrail -i after.hprof \
  --auto-mapping \
  --package com.nexio.tv \
  --serial 192.168.50.98:5555 \
  --leak-suspects
```

Run this from the Android project root, or pass `--project-root ~/Scripts/nexio`
from another directory. If multiple local outputs match, heaptrail prefers a
single `universal` variant; otherwise pass `--mapping` explicitly. The mapping
must come from the same build as the installed app; using a stale mapping can
produce plausible but wrong class names.

Mapping applies to summary, diff, referrers, paths, merged paths, leak suspects,
and allocation-site reports. Summary and diff JSON include
`obfuscated_class_name` whenever a class was renamed, so machine-readable
reports retain a link back to the raw HPROF symbol.

### Option B — Android Studio Profiler

1. Run → Profile 'app'
2. In the **Memory** tool window, click the camera icon ("Capture heap dump").
3. Right-click the capture in the timeline → **Export…** → save as `.hprof`.

If you tick **Record memory allocations** before capturing, the resulting hprof
also contains `AllocationSites` records (per-call-site bytes/instances). These
can be summarized with `heaptrail -i heap.hprof --allocation-sites`.

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
| Compare a playback/state timeline | `heaptrail --diff-series launch.hprof home.hprof play.hprof stop.hprof` |
| Pipe to `jq` / dashboards | append `--json` to any of the above |
| **Show what a giant `char[]`/`byte[]` actually contains** | append `--preview-bytes 200` to summary, paths, find-referrers, or `-l` |
| **Retained-size triage** ("is 35K instances actually 35 MB or 350 MB?") | append `--retained-size` to summary, paths-from-id, or find-referrers |

Common flags:

- `-t N` / `--top N` — top-N rows shown (default 20).
- `--hops 1\|2\|3` — referrer chain depth (default 2).
- `--include-statics` — include class statics as candidate referrers (default true).
- `--group-holders` — group referrer rows by owner family, holder class, and field label.
- `--max-depth N` — bail on path walk after N hops (default 12).
- `--diff-by count\|bytes` — sort diff by Δinstance-count (default) or Δshallow-bytes.
- `--native-context PATH` — attach Android `dumpsys meminfo` totals to `--diff-series`.
- `-l` / `--listStrings` — dump every UTF-8 string in summary mode.
- `--preview-bytes N` — opt-in content preview for primitive arrays (v0.9.0).
- `--list-arrays-min-bytes N` — threshold for the `-l --preview-bytes`
  standalone-large-arrays section (default 1024).
- `-d` / `--debug` — verbose record-tag tracing.
- `--json` — also write a structured JSON sidecar file.

Mutually exclusive: pick **one** of `--find-referrers`, `--target-glob`,
`--paths-from-id`, `--diff-from`/`--diff-to`, or `--diff-series`. With none of
those, you get the summary.

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

### Playback timeline workflow with `--diff-series`

For media apps, leaks often attach to state transitions rather than a single
before/after pair. Capture ordered snapshots around explicit states:

1. launch
2. home loaded
3. playback started
4. playback stopped
5. soak

Then run:

```bash
heaptrail --diff-series launch.hprof home.hprof play.hprof stop.hprof soak.hprof \
  --diff-by bytes --top 30 --json --json-out reports/playback-series.json
```

The report prints adjacent step deltas, first-to-last totals, and monotonic
growth candidates. Use monotonic growth candidates as the shortlist for
referrer and path probes.

For Media3/decoder ownership, collapse the holder table:

```bash
heaptrail -i play.hprof --target-glob 'androidx.media3.**' --hops 2 --group-holders
```

When native pressure may matter, capture `adb shell dumpsys meminfo <package>`
beside the HProf series and attach it:

```bash
heaptrail --diff-series launch.hprof play.hprof soak.hprof \
  --native-context meminfo.txt \
  --json --json-out reports/playback-series.json
```

`--native-context` does not change Java heap calculations; it only adds a
bounded Java Heap / Native Heap / Graphics / GL / TOTAL PSS block to text and
JSON for correlation.

---

## `--preview-bytes` — content preview

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

heaptrail told us *who* held it. `--preview-bytes` answers *what* it is.

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

### Content labels

Preview output includes a concise `content:` label before the snippet or hex
preview. The label is heuristic and intentionally conservative: known file
signatures such as PNG, JPEG, GIF, WebP, gzip, and ZIP win over generic binary
labels; text previews are labeled as JSON, XML, UTF-8 text, or UTF-16 text when
the prefix is clear.

Example:

```text
content: JSON
{"items":[{"id":42,"title":"Example"}]}...
```

For opaque buffers, heaptrail reports `unknown binary`, `protobuf-like binary`,
or `binary/repeated-fill` rather than guessing an application-specific format.

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
  doesn't identify the content — the canonical SharedPreferences-XML /
  cached-JSON / image-buffer disambiguation case. `--preview-bytes 200`
  plus a re-run of `summary` adds inline content snippets to the
  largest-array list.
- During `--paths-from-id` walks where a hop lands on a primitive array
  (e.g. `StringBuilder.value` → giant `char[]` of unknown content).
- For ad-hoc inspection: `--find-referrers id:<u64> --preview-bytes 200`
  shows the array's contents as a header on the referrer report.
- For exploratory listing of all big text-like arrays:
  `-l --preview-bytes 200` adds a "Standalone large arrays" section
  after the `List of Strings` block — useful when the leak is in raw
  `byte[]` / `char[]` allocations, not `java.lang.String` instances.

### Worked example — duplicate-content + cache-blob detection

First production triage with `--preview-bytes` on an Android dump
(`heap-iter-fix.hprof`, 135 MiB) revealed two leak patterns in one
command that the holder chain alone couldn't disambiguate:

```bash
heaptrail -i heap-iter-fix.hprof -l --preview-bytes 65536
```

Trimmed output of the "Standalone large arrays" section:

```
   234.01KiB  object_id=…  char[]   {"schemaVersion":5,…"traktGroups":[…]}
   205.00KiB  object_id=…  char[]   {"airsDays":["monday"],"aliases":[…
   205.00KiB  object_id=…  char[]   {"airsDays":["monday"],"aliases":[…
   205.00KiB  object_id=…  char[]   {"airsDays":["monday"],"aliases":[…
   114.00KiB  object_id=…  char[]   {"personalLists":[{"isPersonal":…
    64.00KiB  object_id=…  char[]   <string name="alias::en::policy:1::tv:tvdb:…
    53.00KiB  object_id=…  char[]   {"catalogRow":{"addonBaseUrl":"https://…
    32.00KiB  object_id=…  byte[]   <string name="alias::en::policy:1::tv:tvdb:…
```

What each preview line answered (without leaving heaptrail):

- **Three identical 205 KiB `char[]`s of the same `{"airsDays":...}` JSON** —
  600 KiB of redundant in-memory copies. A subsequent
  `--find-referrers id:<one of the three>` walk pinned the holders to
  `StringReader.str` mid-`gson.fromJson(String, …)` — i.e. concurrent
  parses materializing the whole cache string at once. Read-side
  equivalent of a streaming-write rule; the fix is
  `gson.fromJson(JsonReader, type)` off a `BufferedReader`, which would
  never allocate the 205 KiB `char[]`.
- **234 KiB `char[]` of `{"schemaVersion":5,…"traktGroups":[…]}`** held
  by `SharedPreferencesImpl.mMap` (the `trakt_discovery_snapshot`
  entry) — 47 KB on disk inflated to 234 KB resident as a `String`
  during XML deserialization. Without the preview, the chain stopped
  at `SharedPreferencesImpl.mMap` and the actual blob's identity was
  invisible.
- **64 KiB `char[]` + 32 KiB `byte[]` of `<string name="alias::…">`** —
  the `hydrated_home_overlay_v1.xml` SharedPreferences file
  materialized in the heap. The exact file the design spec described.
- **53 KiB `{"catalogRow":{"addonBaseUrl":"https://…"}` and 114 KiB
  `{"personalLists":…}`** — Trakt list cache and catalog disk cache,
  identifiable from the JSON root-key alone.

Without `--preview-bytes`, all of these were anonymous "large `char[]`
held by `SharedPreferencesImpl.mMap` / `StringReader.str`" entries.
With it, root-cause and remediation strategy fell out in one pass:
both classes of finding (duplicate concurrent parses + SharedPreferences
XML inflation) point to the same fix — replace
`gson.fromJson(String, type)` with streaming reads.

---

## `--retained-size` — dominator-tree retained sizes

### Why this exists

`summary` ranks classes by **shallow** size — bytes occupied by the
instance header and own fields. For wrapper objects that hold deep
subgraphs, shallow drastically under-represents the cost of allowing
the class to live.

The engineering session that drove this feature: a
`ResolvedDisplayItem` was 88 bytes shallow, but each instance held a
12-element `ResolvedDisplayFieldSlots` (each slot pointing at typed
content) and an `ArtworkBundle` (multiple image references). Shallow
size flagged the class at 3 MB across 35K instances. The actual
question — "is this 35K-instance retention 35 MB, or is it 350 MB?"
— could only be answered by following references manually for one
instance and multiplying out, or by reaching for Eclipse MAT.

heaptrail v1.0.0 brings retained size — the metric MAT computes via
dominator-tree analysis — to the CLI. Same diagnostic shape as
`--preview-bytes` for v0.9.0: the data was always there, but
heaptrail wasn't surfacing it; now it does.

### How to use it

```bash
# Re-sort the class table by retained bytes; add "Largest retained
# instances" hot list of (object_id, class, retained_bytes).
heaptrail -i my.hprof --retained-size -t 20

# Annotate each path-from-id hop with that hop object's retained size.
heaptrail -i my.hprof --paths-from-id <u64> --retained-size

# Add a `class retained` column to each holder row.
heaptrail -i my.hprof --find-referrers <class-or-id> --retained-size
```

Output example (re-sorted summary table):

```
+----------+-----------+-----------+-------------+---------------------------------------+
|  Shallow | Instances |  Retained |     Largest | Class name                            |
+----------+-----------+-----------+-------------+---------------------------------------+
|  2.80MiB |    122154 | 126.84MiB |  24.00bytes | java.util.ArrayList                   |
|  1.53MiB |     28697 |  58.58MiB |  56.00bytes | com.nexio.tv.domain.model.CatalogRow  |
| 56.89MiB |    573552 |  57.37MiB | 104.00bytes | com.nexio.tv.domain.model.MetaPreview |
|  8.25KiB |       352 |  15.18MiB |  24.00bytes | kotlin.SynchronizedLazyImpl           |
+----------+-----------+-----------+-------------+---------------------------------------+
```

The table makes the wrapper-vs-subgraph effect immediately visible:
`SynchronizedLazyImpl` is 8 KiB shallow but anchors **15 MiB** of
retained subgraph — a small wrapper class that's individually trivial
but, in aggregate, owns a serious chunk of the heap. Shallow size
would have placed it well below the visible cutoff.

### How it's computed

heaptrail builds an in-memory CSR object-reference graph from the
hprof, computes immediate dominators using Lengauer–Tarjan
(O(N α(N))), then walks the dominator tree post-order to sum
retained bytes per node. Class-level totals and the top-N
largest-retained instance ids fall out of the same pass.

### Reference strength: weak / soft / phantom edges are included

heaptrail v1.0.0's dominator tree treats every reference as equal —
including `WeakReference`, `SoftReference`, and `PhantomReference`
edges. That's the strict graph-theoretic dominator definition.

Eclipse MAT's default leak-hunting workflow excludes those edges, so
a side-by-side comparison will show MAT's retained smaller than
heaptrail's for any object reachable only via a weak/soft/phantom
holder. **This is by design, not a bug.** If the difference matters
for your investigation, use `--exclude-soft-weak` to rebuild the graph dropping
outgoing edges from `java.lang.ref.{Soft,Weak,Phantom}Reference` subclasses.

### Memory and wall time

Adds roughly 200 MiB working memory and 1–3 s wall time on a
200 MiB Android dump (~3M objects, ~15M edges). Negligible on
typical JVM dumps. Default off.

### When to use

- After `summary` shows a class with high instance count but low
  shallow size — retained tells you whether each instance silently
  anchors a deep subgraph.
- When prioritizing leak-hunting work: a class with 35K instances at
  88 bytes shallow looks small; if its retained is 350 MB, that's
  the right starting point even if the shallow-size ranking buried
  it. The "is this 35K-instance retention 35 MB or 350 MB?"
  triage question.
- During a `--paths-from-id` walk where you want to know how much
  weight each hop carries (an upper bound on what freeing that
  reference would reclaim).
- To rank holder classes from `--find-referrers` by retained rather
  than ref-count alone — sometimes 1 reference holds 8 MiB.

---

## `--exclude-soft-weak` — drop weak/soft/phantom holders (v1.1.0)

### Why this exists

Real Android dump pain: a `--paths-from-id` walk for a leaked
`MainActivity` shows the holder chain as

```
  hop 1 ── id=...  (via leakcanary.KeyedWeakReference.referent)
  hop 2 ── id=...  (via leakcanary.KeyedWeakReferenceWatcher.watchedRefs)
  hop 3 ── id=...  (via java.lang.ref.Reference.discovered)
  ... 12 more hops of WeakReference / Reference internals ...
```

The actual *strong* holder — the EventBus subscriber, the static
cache, the lambda capturing `this` — is buried underneath that.
LeakCanary's watchers, the framework's `WeakReference<Activity>`,
and `Reference.discovered` chains are all by-design weak; including
them in path walks is correct graph-theoretically but useless for
leak hunting.

MAT solves this with a default "Exclude phantom/weak/soft references"
toggle in its leak-hunting workflow. heaptrail v1.1.0 does the same
via `--exclude-soft-weak`.

### How to use it

`--exclude-soft-weak` is a *modifier* on existing modes:

```bash
heaptrail -i my.hprof --paths-from-id <id> --exclude-soft-weak
heaptrail -i my.hprof --retained-size --exclude-soft-weak
heaptrail -i my.hprof --find-referrers <class> --exclude-soft-weak
heaptrail -i my.hprof --leak-suspects --exclude-soft-weak
```

When set, every outgoing edge from any
`java.lang.ref.{Soft,Weak,Phantom}Reference` subclass is dropped.
This applies symmetrically to:

- **Graph build (with `--retained-size`):** Reference-subclass
  source nodes have 0 outgoing edges, so retained sums no longer
  credit their referents. An object reachable *only* through a
  `WeakReference` becomes unreachable in the filtered graph
  (retained == 0). Matches MAT's leak-hunting default.
- **Path walks (`--paths-from-id`, `--find-referrers`,
  `--merge-paths`):** when a hop lands on a Reference subclass
  holder, the walker terminates with the annotation
  `[soft/weak/phantom — excluded]; re-run without --exclude-soft-weak
  to see the weak chain`. The chain is truncated; the user can re-run
  to see the suppressed weak holders if needed.

### When to use

- **Always, for Android leak hunting.** LeakCanary itself relies on
  WeakReference; without this flag, every leaked Activity's path
  starts with LeakCanary's own watcher.
- **For "is this object reachable at all?"** combined with
  `--retained-size --exclude-soft-weak`: an object whose retained
  drops to 0 with the flag is held only weakly — it'll be reclaimed
  on the next GC cycle.

### Caveat

heaptrail's retained sums under `--exclude-soft-weak` will sometimes
differ from MAT's by a small margin: MAT also has a separate
"Exclude finalizer references" toggle (for
`java.lang.ref.FinalReference` / `Finalizer` chains). v1.1.0 doesn't
expose that as a separate flag; usually inconsequential.

---

## `--leak-suspects` — automated suspect identification (v1.1.0)

### Why this exists

`summary --retained-size` ranks classes by retained heap, but
*finding the leak* still requires you to know what class to look for.
On an unfamiliar dump — first time triaging an OOM from a service
you didn't write — this is exactly the friction point: you can see
`java.util.HashMap$Node` retained 47 MiB, but is that 47 MiB
distributed across the codebase normally, or is it one runaway
`HashMap` somewhere?

MAT solves this with its "Leak Suspects" report: it auto-ranks
dominators by retained share, picks the dominant *accumulating
class* in each suspect's subtree, and writes a narrative paragraph
per suspect. heaptrail v1.1.0 does the same — terminal-readable,
content-aware via `--preview-bytes`, and content-aware out of the
box (the answer to MAT's narrative is *also* the answer to "what's
in the giant array").

### How to use it

```bash
# Default 5% retained-share threshold; recommended for first triage:
heaptrail -i my.hprof --leak-suspects --exclude-soft-weak --preview-bytes 200

# Tighter threshold (10%) when you want only the biggest:
heaptrail -i my.hprof --leak-suspects=0.10 --exclude-soft-weak

# JSON sidecar for CI gates:
heaptrail -i my.hprof --leak-suspects --json
```

### Output anatomy

```
Heap: 234.18 MiB total, 187.42 MiB retained-reachable.
Threshold: 5.0 % retained share. Showing 3 suspect(s) (1 above threshold).

Suspect 1 — 47.32 MiB (25.2 % of heap)
  dominator: com.example.app.AppCache (object_id=4097812752)
  accumulating: 234 instances of java.lang.String, total 42.10 MiB
  preview: {"airsDays":["Mon","Wed","Fri"],"seasonNumber":3, ...
  path to GC root:
    [id=4097812752] com.example.app.AppCache
      ↑ static field INSTANCE in AppCache
        (root: System Class)
```

Each suspect has:
- **dominator** — the object that, if collected, would free this
  retained subtree.
- **accumulating** — the most-common class inside the dominated
  subtree, with instance count and shallow-byte total. Tells you
  *what* is accumulating (e.g. "234 Strings of total 42 MiB" → JSON
  cache; "12 large `Bitmap`s of total 16 MiB" → image leak).
- **preview** — first ~120 chars of the dominator's content (when
  `--preview-bytes` is set and the dominator is a primitive array).
- **path to GC root** — full chain via `--paths-from-id`'s walker.

### Threshold + top-3 fallback

`--leak-suspects[=THRESHOLD]` defaults to 0.05 (5%). When no
dominator clears the threshold, the report falls back to **top-3
suspects flagged "(below threshold)"** so you always get something
useful. Useful when the heap is dominated by a single huge cache:
without the fallback the report would show one entry and stop.

### When to use

- **First-pass triage on an unfamiliar dump.** The
  "open MAT, click Leak Suspects" workflow, run from a CLI in seconds.
- **Pair with `--exclude-soft-weak`** unless you specifically want
  weak holders to count. The recommended default is
  `--leak-suspects --exclude-soft-weak --preview-bytes 200`.

---

## `--merge-paths` — fold parallel paths-to-root (v1.1.0)

### Why this exists

When 47 leaked `MainActivity` instances all share the same holder
chain, the *common prefix* is what tells you the leak is the
EventBus. But `--paths-from-id` walks one instance at a time —
running it 47 times gives you 47 nearly-identical chains and forces
you to spot the shared structure by eye.

`--merge-paths` resolves all instances of the start id's class in a
single pass and folds their paths into a single tree with branch
counts. The `[47×]` annotation on each hop tells you "this hop is
shared by all 47 instances" — the EventBus jumps out immediately.

### How to use it

```bash
heaptrail -i my.hprof --paths-from-id <any-instance-id> --merge-paths --retained-size
```

Output:

```
Target: com.example.app.MainActivity — 37 instance(s) merged.
(merge verified via dominator convergence)

  ↑ field handler in MainActivity                              [37×]
    ↑ field this$0 in EventBus$SubscriberHolder                 [37×]
      ↑ key in ConcurrentHashMap$Node (subscribers)             [37×]
        ↑ static EventBus.INSTANCE                              [37×]
```

When paths fork, branches render as their own subtrees:

```
  ↑ field handler in MainActivity                              [25×]
    ↑ ... continues ...
  ↑ field activityRef in NavHostController                     [12×]
    ↑ ... continues ...
```

### Graph-verified vs textual merge

Without `--retained-size`, the merge is a **textual prefix match** —
two paths sharing a hop class+field name are folded together even
if they don't graph-converge at the same dominator. Almost always
correct in practice; the renderer emits a banner so you know the
difference. Pair with `--retained-size` for **dominator-verified
convergence** when the answer matters.

### Limitation

The target must be a class instance, not a primitive array — "fold
all paths to all instances of class X" is the design goal, and
primitive arrays aren't a class. For primitive arrays, use plain
`--paths-from-id`.

---

## `--bitmaps` — Bitmap pixel-byte accounting (v1.1.0)

### Why this exists

Bitmaps dominate Android heaps. A 12 MiB `byte[]` in
`summary --retained-size`'s class table is just "another big
primitive array" until you see it's a 4096×4096 ARGB_8888 bitmap
held by a `RecyclerView.ViewHolder` that should have recycled.

`--bitmaps` walks every `android.graphics.Bitmap` instance, reads
the cached field offsets (resolved at index time), and computes
pixel bytes from `width × height × config`. Top-N output ranks them
by pixel size with location (Java-heap or native).

### How to use it

```bash
heaptrail -i my.hprof --bitmaps -t 20
```

If `android.graphics.Bitmap` is not loaded in the dump, heaptrail exits with an
actionable message instead of implying the dump is not Android. This can happen
on Android screens that have not used Bitmap-backed images.

Output:

```
Top 20 Bitmap instances by pixel bytes:
  pixel_bytes   dimensions    config        location  object_id
    64.00 MiB   4096×4096     ARGB_8888     native    4097812752
    12.00 MiB   1024×3072     ARGB_8888     java      2097446928
     4.00 MiB   1024×1024     ARGB_8888     java      1723142144
    ...

Total bitmap pixel bytes: 96.50 MiB across 247 instances.
```

### Java-heap vs native

- **Pre-O Android** (API ≤ 25) stores pixel data in a Java `byte[]`
  pointed to by `Bitmap.mBuffer`. heaptrail's `location` column
  shows `java`. The Java-heap pixel array is in the dump and counted
  toward heap totals.
- **O+ Android** (API ≥ 26) moved pixel data to native heap; only
  `mNativeBitmap` (a long handle) remains in the Java heap.
  heaptrail's `location` column shows `native`. **The pixel bytes
  reported are estimated from `width × height × bpp`, not from the
  dump itself** — native heap isn't captured in `am dumpheap`. The
  number is approximate (a small over-estimate is possible if the
  config has stride padding).

### When to use

- **Triaging Android OOM where bitmaps are suspect.** Image
  carousels, large posters, RecyclerView misuse.
- **Combined with `--paths-from-id`:** find a giant bitmap, then
  walk its holder chain to identify the misbehaving view holder /
  fragment / cache.

### Limitations in v1.1.0

- `config` defaults to `ARGB_8888` (4 bpp). Resolving the actual
  `Bitmap.Config` enum constant requires an extra pass to read each
  enum's `name` field; deferred to v1.2 since ARGB_8888 is by far
  the most common case.
- `holder_summary` is `None` (would require a path-to-root walk per
  bitmap; expensive on dumps with hundreds of bitmaps). Use
  `--paths-from-id <object_id>` directly to chase a specific bitmap.

---

## `--target-glob` — pattern targeting

### Why this exists

You see 35K instances spread across `*$Itr`, `*$KeyIterator`, `*$EntryIterator`
in summary and want to know "what's producing all these iterators?" — that's
six exact-match runs of `--find-referrers`. Or you want to chase every
`com.nexio.tv.domain.model.*` class in one shot to confirm a domain layer
isn't leaking into a UI layer. Per-class targeting forces you into a query
loop; a glob does it in one pass.

### How to use it

`--find-referrers` accepts an exact FQ class name. When you want to
target a *family* of classes — every model class in a package, every
inner iterator class — use `--target-glob` instead.

```bash
# All MetaPreview-related model classes
heaptrail -i heap.hprof --target-glob 'com.nexio.tv.domain.model.*' --hops 2

# Every Iterator inner class anywhere
heaptrail -i heap.hprof --target-glob '**$Itr'

# Match a single character
heaptrail -i heap.hprof --target-glob 'com.example.User?'
```

Glob syntax matches dotted FQ class names:

| Pattern | Meaning |
|---------|---------|
| `*`     | one package level (no `.`) |
| `**`    | zero or more levels (crosses `.`) |
| `?`     | exactly one character |
| `[abc]` | one of the listed characters |

Output prepends a "matched classes" header listing each class with its
live instance count, sorted by count desc:

```
Found 70 classes matching glob:com.nexio.tv.domain.model.*:
  - com.nexio.tv.domain.model.MetaPreview                 (123382 instances)
  - com.nexio.tv.domain.model.ProviderIds                 (18084 instances)
  - com.nexio.tv.domain.model.CatalogRow                  (6185 instances)
  ...
```

Mutually exclusive with `--find-referrers <name>`; passing both is a
CLI error. Implementation uses the [`globset`](https://crates.io/crates/globset)
crate with `literal_separator=true`.

---

## `--allocation-sites` — per-class stack traces

### Why this exists

Real session: `summary` shows a 72 MiB `char[]`. `--paths-from-id` walks
to a `StringBuilder.value` rooted at a `RootJavaFrame`. heaptrail told us
*who held it* but not *which method allocated it*. The fix was a 20-minute
source-grep through `gson.toJson` / `moshi.*toJson` candidates to guess
the call site.

If the dump had been captured under allocation tracking, every allocation
site would already be in the hprof — but heaptrail used to parse those
records and discard them. `--allocation-sites` turns that data into a
ranked list of "this class was allocated 4.8M times totalling 1.21 GiB,
here's the exact stack trace at the call site." Skips the source-grep
entirely; works as the first thing you reach for after `summary`.

The summary always reports presence/absence so you know whether re-capturing
under tracking would help **before** trying to switch tools.

### Capturing an alloc-tracked dump

```bash
adb shell am profile start <pid>          # turn on alloc tracking
# (run the suspect interaction)
adb shell am dumpheap <pid> /sdcard/heap.hprof
adb shell am profile stop <pid>            # turn off
adb pull /sdcard/heap.hprof
```

### Running the report

```bash
heaptrail -i heap.hprof --allocation-sites --top 20
```

Output:

```
Top 20 allocation sites by bytes_allocated (of 12,453 total):

  ─ 1.21 GiB  /  4,812,000 instances  com.nexio.tv.domain.model.MetaPreview#<init>
        at com.squareup.moshi.adapters.ClassJsonAdapter.fromJson(ClassJsonAdapter.java:128)
        at com.squareup.moshi.JsonAdapter$1.fromJson(JsonAdapter.java:194)
        at com.nexio.tv.network.HomeRepository.fetchCatalog(HomeRepository.kt:87)
        ...
```

### When the dump has no alloc data

`heaptrail summary` reports it explicitly:

```
AllocationSites: not present (capture with `am profile start <pid>`)
```

Running `--allocation-sites` on a non-tracked dump exits with the same
hint as an error, so scripts can detect the case and fall back.

---

## Object[] indices in `--paths-from-id`

### Why this exists

A path through `ArrayList.elementData` used to render as a generic
`via java.lang.Object[][]` — you could see *that* an array held it, but
not *where in the array*. With paged results (e.g. row 13 of a catalogue)
or sparse caches, the slot index is a load-bearing piece of information
for reproducing the path in code. Surfacing it costs essentially nothing
because the walker already iterated to find the matching element.

### What it looks like

When a path hop passes through an `Object[]`, the output now includes
the matched element index:

```
  hop 5  ── id=518041528  (via java.lang.Object[][12])
```

Useful when an `ArrayList.elementData` sits between you and the
target — you can correlate index 12 back to a known position in the
collection (e.g., a paged result's 13th entry).

---

## Thread name + top frame on thread-owned roots

### Why this exists

Same real session that motivated `--allocation-sites`: `--paths-from-id` ended at
`→ reached GC root: RootJavaFrame`. We had the chain *up to* a Java
frame but not *which thread* or *what method* was running on it.
Source-grepping `gson.toJson|moshi.*toJson` would have ended faster if
heaptrail had said `thread "pool-7-thread-2"` (instantly identifying the
SharedPreferences flusher) — the HPROF spec includes the thread serial
and frame index on `RootJavaFrame`, the data was there, the renderer
just wasn't using it.

Thread name + top frame is now standard output for any `--paths-from-id`
walk that terminates at a thread-owned root.

### What it looks like

When `--paths-from-id` terminates at one of:

- `RootJavaFrame` — a Java stack frame holds the object
- `RootThreadObject` — the chain reached the `Thread` itself
- `RootJniLocal` / `RootJniMonitor` — JNI references

heaptrail prints the thread name and (for `RootJavaFrame`) the top
frame's method/file/line:

```
  → reached GC root: RootJavaFrame
        thread "pool-7-thread-2"
        at android.app.SharedPreferencesImpl$EditorImpl.commitToMemory(SharedPreferencesImpl.java:478)
```

When the dump's `StartThread` / `StackTrace` records are missing, the
gap is reported explicitly:

```
  → reached GC root: RootJavaFrame
        (thread metadata not in dump)
```

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
jq '.top_allocated_classes[0]' heaptrail-<ts>.json
```

For stable automation paths, pass `--json-out <path>` with `--json`:

```bash
heaptrail -i heap.hprof --leak-suspects --exclude-soft-weak --json --json-out reports/leaks.json
heaptrail --diff-from before.hprof --diff-to after.hprof --json --json-out reports/diff.json
```

`--json-out` requires `--json`. Parent directories must already exist; heaptrail
reports the normal file-creation error if they do not.

JSON file naming:

| Mode | Filename |
|------|----------|
| summary | `heaptrail-<ts>.json` |
| `--find-referrers` | `heaptrail-referrers-<ts>.json` (timestamped) |
| `--paths-from-id` | `heaptrail-paths-<ts>.json` |
| `--diff-from`/`--diff-to` | `heaptrail-diff-<ts>.json` |
| `--diff-series` | `heaptrail-diff-series-<ts>.json` |

When `--json-out <path>` is present, all modes write exactly that path instead
of the generated filename.

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
64-bit identifier sizes. Run `cargo install heaptrail --force` for the current
published build.

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
