# hprof-slurp

[![Build status](https://github.com/agourlay/hprof-slurp/actions/workflows/ci.yml/badge.svg)](https://github.com/agourlay/hprof-slurp/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/hprof-slurp.svg)](https://crates.io/crates/hprof-slurp)

`hprof-slurp` is a specialized JVM heap dump analyzer.

It is named after the `hprof` format which is used by the [JDK](https://hg.openjdk.java.net/jdk/jdk/file/ee1d592a9f53/src/hotspot/share/services/heapDumper.cpp#l62) to encode heap dumps.

The design of this tool is described in details in the [following blog articles series](https://agourlay.github.io/tags/hprof-slurp/).

## Motivation

The underlying motivation is to enable the analysis of **huge** heap dumps which are much larger than the amount of RAM available on the host system.

`hprof-slurp` processes dump files in a **streaming fashion in a single pass** without storing intermediary results on the host.

This approach makes it possible to provide an extremely fast overview of dump files without the need to spin up expensive beefy instance.

However, it does not replace tools like [Eclipse Mat](https://www.eclipse.org/mat/) and [VisualVM](https://visualvm.github.io/) which provide more advanced features at a different cost.

## Features

- displays top `n` raw shallow heap classes found in the dump.
- displays number of instances per class.
- displays largest instance size per class.
- displays threads stack traces.
- lists all `Strings` found.
- output results as JSON possible
- **referrer tracing** (`--find-referrers`) — find what holds an over-allocated
  class or specific object id, with multi-hop chain support.

## Usage

```
./hprof-slurp --help
JVM heap dump hprof file analyzer

Usage: hprof-slurp [OPTIONS] --inputFile <inputFile>

Options:
  -i, --inputFile <inputFile>  binary hprof input file
  -t, --top <top>              the top results to display [default: 20]
  -d, --debug                  debug info
  -l, --listStrings            list all Strings found
      --json                   additional JSON output in file
  -h, --help                   Print help
  -V, --version                Print version
```

### Example table

```bash
./hprof-slurp -i "test-heap-dumps/hprof-64.bin"
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
./hprof-slurp -i "test-heap-dumps/hprof-64.bin" --top 3 --json
```

```bash
less hprof-slurp.json | grep jq .
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

## Referrer tracing (`--find-referrers`)

When the summary tells you that `java.lang.String` is using 1.4 GiB of heap,
the next question is *who's keeping all those Strings alive*. `--find-referrers`
answers that by walking the reference graph in reverse.

```bash
# Find what holds every instance of a class
./hprof-slurp -i my.hprof --find-referrers java.util.ArrayList --top 30

# Find what holds a specific object id (e.g. a 54 MiB char[] surfaced
# in summary's "Largest array instances object ids" section)
./hprof-slurp -i my.hprof --find-referrers id:66277392

# Trace 2 hops (covers ArrayList$elementData and similar Object[]-mediated holders)
./hprof-slurp -i my.hprof --find-referrers java.util.ArrayList --hops 2

# 3 hops chains one more link (X holds Y holds Object[] holds target)
./hprof-slurp -i my.hprof --find-referrers java.util.ArrayList --hops 3
```

Example output:

```
Found 378 target instance(s) for java.util.LinkedList$Node

=== Direct referrers (1-hop) ===
  holder.field (or class[] for arrays)  ref count
  java.util.LinkedList.last                   190
  java.util.LinkedList.first                  190
  java.util.LinkedList$Node.next              188
  java.util.LinkedList$Node.prev              188
```

Targets:

| Form | Meaning |
|------|---------|
| `<class fq-name>` | Every instance of a class (e.g. `java.util.ArrayList`). |
| `id:<u64>` | A single object id (e.g. `id:66277392`). |
| `<u64>` | Bare digits — same as `id:<u64>`. |

Flags:

- `--hops 1\|2\|3` (default `2`) — direct, via Object[], or three-link chain.
- `--include-statics` (default `true`) — include class statics as candidate holders.
- `--top N` (default `20`) — top N holders per hop.
- `--json` — also write `hprof-slurp-referrers-<ts>.json`.

### How it works (and why it does multiple passes)

The streaming summary parser drops instance bodies and array element ids by
default — that's how it can process dumps larger than RAM at ~2 GB/s. Referrer
tracing needs those bytes, so it runs in additional passes:

1. **Pass 1A** — index utf8 / class metadata / GC roots (lite parser).
2. **Pass 1B** — when targeting a class FQ-name, collect matching instance ids
   (skipped for `id:N` targets).
3. **Pass 2** — retain-bodies stream; resolve hop-1 holders.
4. **Pass 3 / 4** (when `--hops >= 2 / 3`) — chain another hop.

Wall-cost is roughly `O(hops × file_size)`. On the bundled 3 MB JVM fixture this
is single-digit milliseconds; on a 235 MB Android dump expect a few seconds.

## GC churn analysis caveat

A single hprof shows what is **live at one instant**, not allocation rate. For
real GC churn analysis use one of:

- **Two snapshots, then diff them** (`--diff-from a.hprof --diff-to b.hprof`).
  The class with the largest delta in instance count is the strongest churn signal.
- **A dump captured with allocation tracking enabled** — Android Studio's
  "Record memory allocations", `art --allocation-tracking`, or Perfetto produce
  hprof files containing `AllocationSites` records and a populated `HeapSummary`
  with cumulative `total_bytes_allocated` since process start. Surfacing those
  in the summary view is on the roadmap; today they're parsed but unused.

For "what holds the over-allocated thing I just saw in the summary?", use
`--find-referrers` on the class FQ-name or `--find-referrers id:N` for a
specific large object whose id was reported in the summary's
"Largest array instances object ids" section.

## Capturing an Android heap dump

```bash
# pick the target process
adb shell ps | grep com.example.myapp

# capture
adb shell am dumpheap <pid> /data/local/tmp/heap.hprof
adb pull /data/local/tmp/heap.hprof
hprof-slurp -i heap.hprof
hprof-slurp -i heap.hprof --find-referrers <class>  # follow up
```

Android Studio's profiler ("Memory" → "Capture heap dump") emits a compatible
hprof. Captures taken under "Record memory allocations" additionally contain
allocation-site stack traces.

## Installation

### Releases

Using the provided binaries in https://github.com/agourlay/hprof-slurp/releases

### Crates.io

Using Cargo via [crates.io](https://crates.io/crates/hprof-slurp).

```bash
cargo install hprof-slurp
```

## Performance

On modern hardware `hprof-slurp` can process heap dump files at around 2GB/s.

To maximize performance make sure to run on a host with at least 4 cores.

## Limitations

- Tested only with `JAVA PROFILE 1.0.2` & `JAVA PROFILE 1.0.1` formats.
- Supports heap dumps with 4-byte and 8-byte HPROF identifiers.

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
