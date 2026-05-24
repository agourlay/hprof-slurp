# heaptrail

[![Build status](https://github.com/johnneerdael/heaptrail/actions/workflows/ci.yml/badge.svg)](https://github.com/johnneerdael/heaptrail/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/heaptrail.svg)](https://crates.io/crates/heaptrail)

`heaptrail` is a JVM/Android heap dump analyzer with referrer tracing,
path-to-root walking, and snapshot diff. Forked from
[agourlay/hprof-slurp](https://github.com/agourlay/hprof-slurp), which
contributes the streaming parser foundation; this fork adds the
investigation modes documented below.

The hprof format itself is the one used by the [JDK](https://hg.openjdk.java.net/jdk/jdk/file/ee1d592a9f53/src/hotspot/share/services/heapDumper.cpp#l62)
to encode heap dumps.

The design of the underlying streaming parser is described in detail in
[Arnaud Gourlay's blog series](https://agourlay.github.io/tags/hprof-slurp/).

## Motivation

`heaptrail` is a CLI for fast, detailed post-mortem analysis of JVM and Android heap dumps. Each investigation mode answers a specific question in a single command — top classes, snapshot diff, referrer chains, paths to GC roots with thread name and top Java frame at thread-owned terminators, allocation-site attribution, inline content previews (v0.9.0) so a 234 KiB `char[]` identifies itself as a `SharedPreferences` XML blob or an inflated Gson string, dominator-tree retained sizes (v1.0.0) for the wrapper-vs-subgraph question MAT answers, automated leak-suspect clustering with reference-strength filtering and bitmap-aware reporting (v1.1.0), and R8/ProGuard mapping deobfuscation (v1.2.0) for Android release-build dumps. v1.1.1 hardens the summary parser so modern Android dumps that reference unloaded boot-classpath / zygote-shared class ids no longer panic with `class id must have a class definition`. Output is structured for terminal reading and CI logs, not interactive exploration.

The parser reads sequentially. Summary and diff modes complete in a single pass; the investigation modes (`--find-referrers`, `--paths-from-id`, `--allocation-sites`) do a lightweight first pass to build a metadata index — classes, threads, stack frames, GC roots — before a targeted second scan. None of those modes hold a full object graph in memory, so multi-gigabyte dumps run comfortably on a laptop. The opt-in `--retained-size` mode (v1.0.0+) is the exception: it builds a full reference graph and dominator tree in memory (~210 MiB extra on a 200 MiB Android dump) — the cost of MAT-grade retained-bytes accounting.

### When to use `heaptrail`

- **Agentic / LLM-driven investigation.** Structured terminal output (with `--json` for machine consumers) lets an agent run heaptrail, read the result, and decide on the next probe. A GUI tool can't sit inside that loop.
- **Headless / CI workflows.** Single static binary, no JVM dependency, deterministic output that diffs cleanly between runs. Fits scheduled jobs, regression-detection pipelines, post-incident automation.
- **Dumps larger than host RAM.** Default and investigation modes don't hold a full object graph in memory; multi-gigabyte captures run on a laptop. (`--retained-size` is the exception — dominator analysis requires a full graph.)
- **Content-aware diagnosis.** Inline previews of large primitive arrays in summary, paths, and referrer output identify the *kind* of bug — a `SharedPreferences` XML blob or an inflated Gson string — which MAT's narrative output doesn't surface (you can click into a String in MAT, but Leak Suspects doesn't preview content; heaptrail's `--leak-suspects` includes a content snippet inline per suspect).
- **Retained-size triage.** Lengauer–Tarjan dominator-tree retained sizes augment `summary`, `--paths-from-id`, and `--find-referrers` — the wrapper-vs-subgraph question MAT answers, at the CLI in seconds rather than a GUI session.
- **Automated suspect detection.** `--leak-suspects` ranks dominators by retained share, clusters by accumulating class, and emits a narrative report with path-to-root and content previews per suspect — the "what's wrong with this dump?" entry point that doesn't require knowing the suspect class up front. Pairs with `--exclude-soft-weak` to filter out `WeakReference`/`SoftReference`/`PhantomReference` noise from LeakCanary and the framework.
- **Android release-build deobfuscation.** `--mapping` applies an explicit R8/ProGuard mapping file; `--auto-mapping` queries the installed package version via ADB and selects the matching local Gradle mapping. Text reports show real class and field names, while JSON summary/diff rows keep `obfuscated_class_name` for traceability.
- **Narrow, repeatable questions.** "Who holds class X?", "What changed between these two dumps?", "What dominates the heap by retained size?", "What are the top allocation sites?" — single-command answers in seconds, no load-and-explore session.

### When to use [Eclipse MAT](https://www.eclipse.org/mat/) or [VisualVM](https://visualvm.github.io/)

- **Interactive graph exploration.** Clicking through inbound/outbound references and pivoting on the fly is a UI capability and stays in MAT's column.
- **OQL** for ad-hoc querying — heaptrail is a fixed-flag CLI by design.
- **HTML reports for non-engineering audiences.** MAT's report exporter is well-suited for sharing with people who won't open a CLI.

The two tools complement each other: `heaptrail` is the cheaper, scriptable, agent-friendly first pass; MAT remains the right tool when the question demands an interactive graph session.

## Documentation

📖 **[USERGUIDE.md](USERGUIDE.md)** — hands-on guide with worked examples
from a real 235 MiB Android dump: how to capture an hprof on Android, every
flag and what it does, and an end-to-end leak-investigation walkthrough.

The sections below are a quick orientation; the user guide has the complete
reference and worked examples.

## Features

- displays top `n` raw shallow heap classes found in the dump.
- displays number of instances per class.
- displays largest instance size per class.
- displays threads stack traces.
- lists all `Strings` found.
- output results as JSON possible
- **referrer tracing** (`--find-referrers`) — find what holds an over-allocated
  class or specific object id, with multi-hop chain support.
- **path-to-root** (`--paths-from-id`) — walk holder chain from one object id
  toward a GC root, with thread name + top frame at thread-owned terminators
  and Object[] element indices on array hops.
- **snapshot diff** (`--diff-from` / `--diff-to`) — per-class delta in instance
  count and shallow bytes between two captures (the strongest churn signal a
  pair of static dumps can give you).
- **glob targeting** (`--target-glob`) — find referrers of every class
  matching a shell-style pattern in one pass.
- **allocation sites** (`--allocation-sites`) — when the dump was captured
  under allocation tracking, print the top-N call sites with resolved Java
  stack traces.
- **content preview** (`--preview-bytes`) — show the first N bytes/chars
  of `char[]` / `byte[]` arrays inline (UTF-8 / UTF-16 / hex auto-detect).
  Identifies SharedPreferences XML, JSON caches, log buffers, and
  image-magic-byte signatures without leaving heaptrail. Closes the gap
  between *who* holds an over-allocated array and *what* is in it.
- **retained size** (`--retained-size`) — Lengauer–Tarjan dominator-tree
  retained sizes augmenting `summary`, `--paths-from-id`, and
  `--find-referrers`. Answers the wrapper-vs-subgraph question:
  `ResolvedDisplayItem` is 88 bytes shallow but holds a
  `ResolvedDisplayFieldSlots` + `ArtworkBundle` — for 35K instances,
  shallow says 3 MB; retained tells you whether the *real* cost is
  35 MB or 350 MB.
- **leak suspects** (`--leak-suspects`) — auto-rank top dominators by
  retained share, cluster dominated objects by class, emit narrative +
  path-to-root + content-preview snippet per suspect. The "what's
  wrong with this dump?" entry point — no need to know what class to
  grep for first. Pair with `--exclude-soft-weak` for the recommended
  leak-hunting workflow.
- **reference-strength filter** (`--exclude-soft-weak`) — drop outgoing
  edges from `java.lang.ref.{Soft,Weak,Phantom}Reference` subclasses
  across path walks and the retained-size graph build. The single most-
  used MAT workflow on Android: cuts LeakCanary watchers and framework
  `WeakReference` noise out of paths so the actual strong holder
  surfaces.
- **merged shortest paths** (`--merge-paths`) — fold paths-to-root for
  every instance of a target class into one tree with branch counts.
  When 47 leaked `MainActivity` instances share the same holder chain,
  the common prefix tells you "it's the EventBus." Pairs with
  `--target-glob` and `--retained-size` (graph-verified convergence
  via dominator tree).
- **bitmap-aware reporting** (`--bitmaps`) — top-N Bitmap instances by
  pixel-byte size, with width × height × config and holder summary.
  Handles both pre-O (Java-heap pixel data via `mBuffer`) and O+
  (native pixel data sized via width × height × bpp). Bitmaps dominate
  Android heaps; this surfaces them without leaving heaptrail.

## Usage

```
./heaptrail --help
JVM/Android heap dump (hprof) analyzer

Usage: heaptrail [OPTIONS] [COMMAND]

Options:
  -i, --inputFile <INPUT_FILE>  Binary hprof input file
  -t, --top <TOP>               The top N results to display [default: 20]
      --json                    Additional JSON output
      --json-out <PATH>         Write JSON output to this exact path
      --mapping <PATH>          R8/ProGuard mapping file for deobfuscation
      --auto-mapping [<MODE>]   Discover mapping from local Gradle output
      --project-root <DIR>      Android project root for --auto-mapping
      --package <PACKAGE>       Android package/application id
      --serial <SERIAL>         adb serial/device id
      --find-referrers <TARGET> Find direct + N-hop referrers
      --paths-from-id <ID>      Trace holder chain toward a GC root
      --diff-from <PATH>        Baseline hprof for diff
      --diff-to <PATH>          Comparison hprof for diff
      --leak-suspects [<THRESHOLD>]
      --retained-size
      --exclude-soft-weak
      --bitmaps
  -h, --help                    Print help
  -V, --version                 Print version
```

### `android-capture` — capture and validate Android dumps

```bash
heaptrail android-capture --serial 192.168.50.98:5555 --package com.example.app --out artifacts/run
heaptrail android-capture --package com.example.app --out artifacts/run --foreground --allocation-sites
```

Runs the ADB capture path, pulls the `.hprof`, validates that the local file is
nonzero, runs a cheap summary pass to record AllocationSites availability, and
writes a transcript with PID, foreground evidence, commands, dump size, and
artifact paths. The helper leaves device files in `/data/local/tmp` so failed
or partial captures remain inspectable.

### Mapping files for obfuscated Android builds

```bash
heaptrail -i after.hprof --mapping app/build/outputs/mapping/universalRelease/mapping.txt --leak-suspects
heaptrail -i after.hprof --auto-mapping --package com.nexio.tv --serial 192.168.50.98:5555 --leak-suspects
```

`--mapping` applies an explicit R8/ProGuard mapping file. `--auto-mapping`
matches the package version installed on the selected ADB device to local
Gradle `output-metadata.json`, then uses the corresponding
`app/build/outputs/mapping/<variant>/mapping.txt`. Run it from the Android
project root, or pass `--project-root` from another directory.

### Example table

```bash
./heaptrail -i "test-heap-dumps/hprof-64.bin"
```

```
Found a total of 2.51MiB of raw shallow heap objects in the dump.

Top 20 raw shallow heap classes:

+------------+-----------+-------------+---------------------------------------------+
| Total size | Instances |     Largest | Class name                                  |
+------------+-----------+-------------+---------------------------------------------+
|    1.99MiB |       436 |   634.78KiB | int[]                                       |
|  194.89KiB |      1991 |    16.02KiB | char[]                                      |
|   85.23KiB |       443 |     8.02KiB | byte[]                                      |
|   47.38KiB |      1516 |  32.00bytes | java.lang.String                            |
|   45.42KiB |       560 |     8.02KiB | java.lang.Object[]                          |
|   14.77KiB |       378 |  40.00bytes | java.util.LinkedList$Node                   |
|   14.77KiB |       126 | 120.00bytes | java.lang.reflect.Field                     |
|    9.94KiB |       212 |  48.00bytes | java.util.HashMap$Node                      |
|    8.91KiB |       190 |  48.00bytes | java.util.LinkedList                        |
|    6.05KiB |       258 |  24.00bytes | java.lang.Integer                           |
|    5.91KiB |        18 |     2.02KiB | java.util.HashMap$Node[]                    |
|    5.44KiB |       116 |  48.00bytes | java.util.Hashtable$Entry                   |
|    5.36KiB |        98 |  56.00bytes | java.lang.ref.SoftReference                 |
|    4.69KiB |       150 |  32.00bytes | java.lang.StringBuilder                     |
|    3.50KiB |        32 | 112.00bytes | java.net.URL                                |
|    3.42KiB |        73 |  48.00bytes | java.io.File                                |
|    3.17KiB |        12 | 776.00bytes | java.util.Hashtable$Entry[]                 |
|    3.13KiB |        56 | 144.00bytes | java.lang.String[]                          |
|    2.95KiB |        63 |  48.00bytes | java.util.concurrent.ConcurrentHashMap$Node |
|    2.50KiB |        40 |  64.00bytes | java.lang.ref.Finalizer                     |
+------------+-----------+-------------+---------------------------------------------+
```

### Example JSON

```bash
./heaptrail -i "test-heap-dumps/hprof-64.bin" --top 3 --json
```

```bash
less heaptrail.json | grep jq .
```

```JSON
{
  "top_allocated_classes": [
    {
      "class_name": "int[]",
      "instance_count": 436,
      "largest_allocation_bytes": 650016,
      "allocation_size_bytes": 2089368
    },
    {
      "class_name": "char[]",
      "instance_count": 1991,
      "largest_allocation_bytes": 16400,
      "allocation_size_bytes": 199568
    },
    {
      "class_name": "byte[]",
      "instance_count": 443,
      "largest_allocation_bytes": 8208,
      "allocation_size_bytes": 87272
    }
  ],
  "top_largest_instances": [..]
}
```

## Beyond the summary

The summary above is the default mode. Three additional modes drive the
deeper investigation workflow — see [USERGUIDE.md](USERGUIDE.md) for the
full reference, flag-by-flag walkthrough, and worked Android-leak example.

### `--find-referrers` — who's holding it?

```bash
heaptrail -i my.hprof --find-referrers java.util.ArrayList --hops 2
heaptrail -i my.hprof --find-referrers id:1661812752    # specific object
```

Direct + multi-hop holders for an FQ class name or specific object id.
Hop 2 is usually where the real diagnosis lives — it goes through
`Object[]`-mediated holders like `ArrayList.elementData`. Details in
[USERGUIDE §4](USERGUIDE.md#4---find-referrers--whos-holding-it).

### `--paths-from-id` — chain to a GC root

```bash
heaptrail -i my.hprof --paths-from-id 1661812752 --max-depth 12
```

Walks holders upward one hop at a time until a GC root is reached or
`--max-depth` is exceeded. When the chain terminates at a thread-owned
root (`RootJavaFrame` / `RootThreadObject` / `RootJniLocal` /
`RootJniMonitor`), the output includes the thread name and (for Java
frames) the top frame's method/file/line. Object[] hops include the
matched element index (e.g. `via java.lang.Object[][12]`). Details in
[USERGUIDE §5](USERGUIDE.md#5---paths-from-id--chain-to-a-gc-root).

### `--diff-from` / `--diff-to` — snapshot diff (churn signal)

```bash
heaptrail --diff-from before.hprof --diff-to after.hprof --diff-by count
```

Per-class delta in instance count and shallow bytes between two captures —
the strongest churn signal a pair of static dumps can give you. Details in
[USERGUIDE §6](USERGUIDE.md#6---diff-from----diff-to--snapshot-diff).

### `--target-glob` — pattern targeting

```bash
heaptrail -i my.hprof --target-glob 'com.example.**' --hops 2
```

Glob-match against dotted FQ class names (`*` within a package level,
`**` across levels, `?` single char, `[abc]` class). Mutually exclusive
with `--find-referrers`. Output prepends a list of matched classes with
live instance counts. Details in
[USERGUIDE — `--target-glob`](USERGUIDE.md#--target-glob--pattern-targeting).

### `--allocation-sites` — per-class stack traces

```bash
heaptrail -i my.hprof --allocation-sites --top 20
```

Requires the dump to have been captured under allocation tracking
(Android: `am profile start <pid>` before `am dumpheap`). Prints the
top-N allocation sites with resolved Java stack traces. Summary always
reports whether the dump has alloc-tracking data. Details in
[USERGUIDE — `--allocation-sites`](USERGUIDE.md#--allocation-sites--per-class-stack-traces).

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
Default 0 (off).

Preview output includes a short `content:` label when heaptrail can identify the
payload shape. Initial labels include `JSON`, `XML`, `UTF-8 text`,
`UTF-16 text`, image signatures (`PNG image`, `JPEG image`, `GIF image`,
`WebP image`), compressed data (`gzip compressed`, `ZIP archive`),
`protobuf-like binary`, `binary/repeated-fill`, and `unknown binary`.

**Engineering motivation:** a 72 MiB `char[]` whose holder chain ended
at a Gson `StringBuilder` told us *who* held it but not *what* it
contained — investigation needed `adb shell` for file size + source-grep
for serialization candidates. With `--preview-bytes 200`, the inline
`<?xml version="1.0"...home_catalog_snapshot...` would have identified
it as the SharedPreferences XML blob in one command. Details in
[USERGUIDE — `--preview-bytes`](USERGUIDE.md#--preview-bytes--content-preview).

### `--retained-size` — dominator-tree retained sizes (v1.0.0)

```bash
heaptrail -i my.hprof --retained-size -t 20
heaptrail -i my.hprof --paths-from-id <id> --retained-size
heaptrail -i my.hprof --find-referrers <class> --retained-size
```

Computes per-instance retained bytes via Lengauer–Tarjan dominators.
Re-sorts the summary class table by retained size, adds a `retained`
column, and appends a "Largest retained instances" hot list of object
ids. `--paths-from-id` annotates each hop with `(retained=<size>)`;
`--find-referrers` adds a `class retained` column to holder rows.
Default off. Adds ~250 MiB working memory and ~1–3 s wall time on a
200 MiB Android dump.

**Engineering motivation:** a class like `ResolvedDisplayItem` shows
88 bytes shallow but holds a 12-element `ResolvedDisplayFieldSlots` +
`ArtworkBundle`. For 35K instances, shallow says 3 MB; retained tells
you whether the *real* cost is 35 MB or 350 MB. Same triage friction
that motivated `--preview-bytes` at the *content* level — `--retained-size`
solves it at the *prioritization* level.

Includes weak/soft/phantom-reference edges by default
(graph-theoretic dominator definition); pair with
`--exclude-soft-weak` (v1.1.0) for MAT-style filtering. Details in
[USERGUIDE — `--retained-size`](USERGUIDE.md#--retained-size--dominator-tree-retained-sizes).

### `--exclude-soft-weak` — filter weak holders (v1.1.0)

```bash
heaptrail -i my.hprof --paths-from-id <id> --exclude-soft-weak
heaptrail -i my.hprof --retained-size --exclude-soft-weak
heaptrail -i my.hprof --leak-suspects --exclude-soft-weak
```

Modifier flag. Drops outgoing edges from
`java.lang.ref.{Soft,Weak,Phantom}Reference` subclasses across path
walks and the retained-size graph. **Engineering motivation:** on
Android, LeakCanary's `KeyedWeakReference` watchers, the framework's
`WeakReference<Activity>`, and `Reference.discovered` chains all
appear as holders in path walks — the real strong holder is buried
underneath. MAT's default leak-hunting view excludes these
automatically; this flag matches that behavior. **Default off; pair
with `--leak-suspects` for the recommended workflow.**

### `--leak-suspects` — automated suspect identification (v1.1.0)

```bash
heaptrail -i my.hprof --leak-suspects --exclude-soft-weak --preview-bytes 200
heaptrail -i my.hprof --leak-suspects=0.10  # 10% threshold
```

Auto-rank dominators by retained share against a threshold (default
5%). Per-suspect output: dominator class + object_id, accumulating-
class summary, content preview (when `--preview-bytes` is set), and
full path-to-root. Always shows top-3 even if all below threshold.
**Engineering motivation:** answers the "what's wrong with this
dump?" question without requiring you to know what class to grep
for first. heaptrail's answer to MAT's Leak Suspects narrative
report — same data, terminal-readable, content-aware via
`--preview-bytes`.

### `--merge-paths` — fold parallel paths-to-root (v1.1.0)

```bash
heaptrail -i my.hprof --paths-from-id <id> --merge-paths --retained-size
```

Modifier on `--paths-from-id`. Resolves all instances of the start
id's class and folds their paths-to-root into a single tree with
branch counts. **Engineering motivation:** when 47 leaked
`MainActivity` instances share the same holder chain, the *common
prefix* tells you "it's the EventBus" — but `--paths-from-id` walks
one instance at a time. `--merge-paths` collapses the 47 paths into
one tree showing `[47×]` on each shared hop. Pair with
`--retained-size` for graph-verified convergence; otherwise textual
prefix matching with a banner.

### `--bitmaps` — Bitmap pixel-byte accounting (v1.1.0)

```bash
heaptrail -i my.hprof --bitmaps -t 20
```

List top-N `android.graphics.Bitmap` instances by pixel-byte size.
Reports `width × height × config`, location (java/native), and one-
line holder summary. **Engineering motivation:** bitmaps dominate
Android heaps but are invisible to the class-name view — a 12 MiB
`byte[]` is just "another big primitive array" until you see it's a
4096×4096 ARGB_8888 bitmap held by a `RecyclerView.ViewHolder`.
Handles both pre-O Java-heap pixel data (via `mBuffer`) and O+
native pixel data (sized via `width × height × bpp`).

If `android.graphics.Bitmap` is not loaded in the dump, heaptrail exits with an
actionable message instead of implying the dump is not Android. This can happen
on Android screens that have not used Bitmap-backed images.

### `--json` — structured output for scripts

Append `--json` to any mode for a machine-parseable sidecar. By default,
heaptrail keeps its existing generated filename behavior. For stable CI or
agent artifacts, add `--json-out <path>`:

```bash
heaptrail -i my.hprof --leak-suspects --exclude-soft-weak --json --json-out reports/leaks.json
heaptrail --diff-from before.hprof --diff-to after.hprof --json --json-out reports/diff.json
```

Details in [USERGUIDE §7](USERGUIDE.md#7---json--structured-output-for-scripts).

## v1.2.0 — Android release-build deobfuscation

v1.2.0 adds first-class R8/ProGuard mapping support for obfuscated Android
release heaps:

- `--mapping <PATH>` loads an explicit mapping file.
- `--auto-mapping` matches the installed package's `versionCode` /
  `versionName` to local Gradle `output-metadata.json`, then loads
  `app/build/outputs/mapping/<variant>/mapping.txt`.
- Summary, diff, referrers, paths, merged paths, leak suspects, and allocation
  site reports render deobfuscated class and holder-field names.
- Summary and diff JSON keep `obfuscated_class_name` when a row was
  deobfuscated, so reports remain traceable back to raw HPROF symbols.
- `android-capture --auto-mapping` records mapping path, source, `pg_map_id`,
  and `pg_map_hash` in the capture transcript.

## v1.1.1 — modern Android dump robustness

v1.1.0 panicked with `class id must have a class definition` when an
`InstanceDump` referenced a class id that was never emitted as a
`ClassDump` / `LoadClass` record. Modern Android dumps (`am dumpheap` on
recent ART builds, app-image / zygote-shared classes elided by the
dumper) routinely do this — the affected dumps were unreadable in
v1.1.0.

v1.1.1 degrades gracefully instead:

- unknown class ids are counted with bare object-header size (no field
  reconstruction possible without the `ClassDump`);
- their class name is rendered as `<unknown class #<id>>`;
- a single end-of-run stderr line reports how many ids were affected,
  so the operator knows the summary is a lower bound when those
  classes happened to dominate the heap.

All other modes (`--retained-size`, `--leak-suspects`,
`--find-referrers`, `--paths-from-id`, `--diff-from`/`--diff-to`) work
unchanged on the affected dumps — they index by object id and don't
depend on per-class field reconstruction. Existing reference dumps
(`JAVA_PROFILE_1.0.2`, `JAVA_PROFILE_1.0.3`) produce byte-identical
output.

## GC churn analysis caveat

A single hprof shows what is **live at one instant**, not allocation rate.
For real churn analysis either:

- capture two snapshots and use `--diff-from`/`--diff-to`, or
- capture under allocation tracking (Android Studio "Record memory
  allocations", `art --allocation-tracking`, Perfetto). Allocation-site
  stack traces are parsed but not yet surfaced in summary output.

## Installation

### Prerequisites — Rust toolchain

`cargo install` needs Rust. If you don't have it yet, install via
[rustup](https://rustup.rs):

```bash
# macOS / Linux
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

```powershell
# Windows (PowerShell)
Invoke-WebRequest -Uri https://win.rustup.rs/x86_64 -OutFile rustup-init.exe
./rustup-init.exe
```

`rustup` installs `cargo` and adds `~/.cargo/bin` (or `%USERPROFILE%\.cargo\bin`
on Windows) to your shell `PATH` automatically for most setups. Open a new
terminal so the change takes effect, then verify:

```bash
cargo --version
```

If `cargo` isn't found, see [Adding cargo bin to PATH](#adding-cargo-bin-to-path) below.

### Latest published build (recommended)

```bash
cargo install heaptrail
```

This downloads the crate, compiles in release mode (~1–2 minutes), and installs
the binary to `~/.cargo/bin/heaptrail` (or
`%USERPROFILE%\.cargo\bin\heaptrail.exe` on Windows). To upgrade later, rerun
with `--force`.

### Build directly from git

```bash
cargo install --git https://github.com/johnneerdael/heaptrail --force
```

Use the git install when testing unreleased changes from `master`.

### Verify the install

```bash
heaptrail --version
heaptrail --help
```

If you see `command not found` (or `'heaptrail' is not recognized` on
Windows), `~/.cargo/bin` is not on your `PATH` — see the next section.

### Adding cargo bin to PATH

`cargo install` writes binaries to `~/.cargo/bin/`. If `rustup` didn't add
it to your shell automatically, add it manually:

**bash** (`~/.bashrc` or `~/.bash_profile`):

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

**zsh** (macOS default since Catalina; `~/.zshrc`):

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

**fish** (`~/.config/fish/config.fish`):

```bash
fish_add_path ~/.cargo/bin
```

**Windows PowerShell** (user-scoped, persists across sessions):

```powershell
[Environment]::SetEnvironmentVariable(
  "PATH",
  "$env:PATH;$env:USERPROFILE\.cargo\bin",
  "User"
)
```

Open a fresh terminal after making the change so the new `PATH` is picked up.

### Pre-built binaries

Each tagged release attaches binaries for six targets, no `cargo` needed:

- Linux: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`
- macOS: `x86_64-apple-darwin`, `aarch64-apple-darwin`
- Windows: `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`

Download from
[johnneerdael/heaptrail/releases](https://github.com/johnneerdael/heaptrail/releases),
extract, and place on your `PATH`. The latest release is
[heaptrail v1.2.0](https://github.com/johnneerdael/heaptrail/releases/latest).

(The legacy summary-only binaries from
[agourlay/hprof-slurp/releases](https://github.com/agourlay/hprof-slurp/releases)
predate the rename to `heaptrail` and lack `--find-referrers`,
`--paths-from-id`, `--diff-from`, `--target-glob`, and
`--allocation-sites`. Use the heaptrail releases above instead.)

## Installing the Claude Code plugin

This repo doubles as a [Claude Code](https://claude.com/claude-code) plugin
marketplace. Installing the plugin gives Claude an `analysing-heap-dumps`
skill that auto-activates whenever you mention `.hprof` files, memory
leaks, retained size, `am dumpheap`, etc. — Claude will then run
`heaptrail` for you, install it via `cargo install heaptrail` if missing,
and walk through the standard triage workflow (summary → find-referrers
→ paths-from-id → diff).

### One-time: add the marketplace

In Claude Code, run:

```
/plugin marketplace add johnneerdael/heaptrail
```

(`gh auth` is only needed if the repo is private.)

### Install the plugin

```
/plugin install analysing-heap-dumps@analysing-heap-dumps
```

Format is `<plugin-name>@<marketplace-name>` — both happen to be
`analysing-heap-dumps` here. Confirm with:

```
/plugin
```

You should see `analysing-heap-dumps` listed as installed.

### Use it

Just talk about your heap dump:

```
I have a 235 MB Android hprof at /tmp/heap.hprof. The app is using way more
memory than expected. What's going on?
```

Claude will load the skill, verify `heaptrail` is on PATH (and install it via
`cargo install heaptrail` if not), then walk you through summary, retainer
tracing, and path-to-root in the right order.

### Updating

```
/plugin marketplace update johnneerdael/heaptrail
/plugin update analysing-heap-dumps@analysing-heap-dumps
```

### Uninstalling

```
/plugin uninstall analysing-heap-dumps@analysing-heap-dumps
/plugin marketplace remove johnneerdael/heaptrail
```

The skill content lives at
[`plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md`](plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md)
if you want to read or fork it without installing.

## Performance

On modern hardware `heaptrail` can process heap dump files at around 2GB/s.

To maximize performance make sure to run on a host with at least 4 cores.

## Format support

- `JAVA PROFILE 1.0.1` (legacy JVM)
- `JAVA PROFILE 1.0.2` (current JVM — `jmap` default)
- `JAVA PROFILE 1.0.3` (modern Android — `am dumpheap` default; includes
  the Android extension tags `RootInternedString` / `RootVmInternal` /
  `RootJniMonitor` / `RootDebugger` / `RootFinalizing` /
  `RootReferenceCleanup` / `Unreachable` / `PrimitiveArrayNoDataDump` /
  `HeapDumpInfo`, all parsed and surfaced)
- 4-byte (Android) and 8-byte (JVM) HPROF identifier sizes

CI validates against bundled JVM 64-bit and JVM 32-bit fixtures. Both
canonical real-world dumps (`JAVA_PROFILE_1.0.2.hprof` and
`JAVA_PROFILE_1.0.3.hprof`) are smoke-tested before every release per
the project's `CLAUDE.md`.

## Known limitations

- No allocation-tracked-dump fixture in CI yet (the
  `--allocation-sites` mode is unit-tested against synthetic records and
  smoke-tested manually; integration coverage will land when a small
  alloc-tracked fixture is captured).
- `--paths-from-id` walks one streaming pass per hop, so deep chains
  (e.g. `--max-depth 12` on a multi-GiB dump) take seconds. Truly deep
  chains rarely add diagnostic value past the first few hops anyway.

## Generate a heap dump

Heap dump files are sometimes generated in case of a JVM crash depending on your runtime configuration.

It can also be done manually by triggering a heap dump using `jmap`.

Example:

`jmap -dump:format=b,file=my-hprof-file.bin <pid>`

## Prior art of HPROF parsing

Several projects have been very useful while researching and implementing this tool.
They have provided guidance and inspiration in moments of uncertainty.

- https://github.com/monoid/hprof_dump_parser
- https://github.com/eaftan/hprof-parser
