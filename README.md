# hprof-slurp

[![Build status](https://github.com/agourlay/hprof-slurp/actions/workflows/ci.yml/badge.svg)](https://github.com/agourlay/hprof-slurp/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/hprof-slurp.svg)](https://crates.io/crates/hprof-slurp)

`hprof-slurp` is a specialized JVM and Android heap dump analyzer.

It is named after the `hprof` format which is used by the [JDK](https://github.com/openjdk/jdk/blob/master/src/hotspot/share/services/heapDumper.cpp) to encode heap dumps.

The design of this tool is described in details in the [following blog articles series](https://agourlay.github.io/tags/hprof-slurp/).

## Motivation

The underlying motivation is to enable the analysis of **huge** heap dumps which are much larger than the amount of RAM available on the host system.

`hprof-slurp` processes dump files in a **streaming fashion in a single pass** without storing intermediary results on the host.

This approach makes it possible to provide an extremely fast overview of dump files without the need to spin up an expensive beefy instance.

However, it does not replace tools like [Eclipse Mat](https://www.eclipse.org/mat/) and [VisualVM](https://visualvm.github.io/) which provide more advanced features at a different cost.

The reported sizes are **shallow**: the footprint of each object itself (its header and fields), not the objects it transitively references. Computing *retained* sizes would require building the full reference graph, which is out of scope for the single-pass design.

## Features

- supports the `JAVA PROFILE 1.0.1`, `1.0.2` and `1.0.3` formats — 32-bit and
  64-bit, including Android ART/Dalvik dumps.
- displays top `n` raw shallow heap classes found in the dump.
- displays number of instances per class.
- displays largest instance size per class.
- filters the reported classes by name.
- displays threads stack traces.
- lists all `Strings` found.
- outputs results as JSON, for the analysis and for the diff.
- fails a build when the heap grew more than a given budget.

## Limitations

- Reports **shallow** sizes only (see [Motivation](#motivation)). Retained
  sizes, dominator trees and reference chains require a full reference graph,
  which the single-pass design does not build.

## Usage

```
./hprof-slurp --help
JVM heap dump hprof file analyzer

Usage: hprof-slurp [OPTIONS] <FILE>
       hprof-slurp [OPTIONS] [FILE] <COMMAND>

Commands:
  diff  compare two dumps of the same process by per-class shallow heap deltas
  help  Print this message or the help of the given subcommand(s)

Arguments:
  <FILE>  binary hprof input file

Options:
  -t, --top <top>         the top results to display [default: 20]
  -f, --filter <PATTERN>  only report classes whose name contains this text
  -d, --debug             debug info
  -l, --list-strings      list all Strings found
      --json              additional JSON output in file
  -o, --output <output>   output file path for the JSON result (default: hprof-slurp-<timestamp>.json)
  -h, --help              Print help
  -V, --version           Print version
```

Progress and diagnostics go to `stderr`, so a redirected `stdout` holds the report and nothing else:

```bash
./hprof-slurp "my-hprof-file.bin" > report.txt
```

### Example table

```bash
./hprof-slurp "test-heap-dumps/hprof-64.bin"
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

### Filter by class name

Both the analysis and the diff accept `--filter`, which keeps only the classes whose name contains the given text. The dump wide totals are still reported, so the share held by the matched classes stays visible.

```bash
./hprof-slurp "test-heap-dumps/hprof-64.bin" --top 3 --filter java.util
```

```
Found a total of 2.51MiB of raw shallow heap objects in the dump.
Filter 'java.util' matches 51 classes totalling 68.55KiB (2.67% of the dump).

Top 3 raw shallow heap classes:

+------------+-----------+------------+---------------------------+
| Total size | Instances |    Largest | Class name                |
+------------+-----------+------------+---------------------------+
|   14.77KiB |       378 | 40.00bytes | java.util.LinkedList$Node |
|    9.94KiB |       212 | 48.00bytes | java.util.HashMap$Node    |
|    8.91KiB |       190 | 48.00bytes | java.util.LinkedList      |
+------------+-----------+------------+---------------------------+
```

When `--json` is used, the matched classes are the ones listed and an extra `heap.filter` object reports the pattern along with its class count and total.

### Diff two dumps

Compare two dumps of the same process to find the classes whose footprint grew between the captures.

```bash
./hprof-slurp diff "before.hprof" "after.hprof" --top 3
```

```
Heap diff of raw shallow sizes:
  from: before.hprof (137.98KiB)
  to:   after.hprof (2.51MiB)
  net:  +2.37MiB

Top 3 of 282 class deltas (by shallow size growth):

      Δ size  Δ instances        size (from → to) instances (from → to)  Class name
    +1.99MiB         +432       1.14KiB → 1.99MiB               4 → 436  int[]
  +130.56KiB        +1158    64.33KiB → 194.89KiB            833 → 1991  char[]
   +60.84KiB         +434     24.39KiB → 85.23KiB               9 → 443  byte[]
```

### Gate a build on heap growth

`diff` exits with code `2` when the net shallow growth goes over `--fail-over`, and `1` only when the run itself failed, so a CI job can tell the two apart. `--json` writes the full comparison for any policy the flag does not cover.

```
compare two dumps of the same process by per-class shallow heap deltas

Usage: hprof-slurp diff [OPTIONS] <FROM> <TO>

Arguments:
  <FROM>  baseline hprof file
  <TO>    hprof file to compare against the baseline

Options:
  -t, --top <top>         the top results to display [default: 20]
  -f, --filter <PATTERN>  only report classes whose name contains this text
      --json              additional JSON output in file
  -o, --output <output>   output file path for the JSON result (default: hprof-slurp-<timestamp>.json)
      --fail-over <SIZE>  exit with code 2 when the net shallow heap growth exceeds this size (plain bytes, or a unit such as 10MiB)
  -h, --help              Print help
```

```bash
./hprof-slurp diff "before.hprof" "after.hprof" --fail-over 10MiB --json -o diff.json
```

```JSON
{
  "schema_version": 1,
  "tool": { "name": "hprof-slurp", "version": "0.10.0" },
  "diff": {
    "from": {
      "file": "before.hprof",
      "file_size_bytes": 282310,
      "format": "JAVA PROFILE 1.0.1",
      "id_size_bytes": 4,
      "captured_at_epoch_millis": 1161941754984,
      "captured_at_utc": "2006-10-27 09:35:54 UTC",
      "total_shallow_bytes": 141288,
      "class_count": 160
    },
    "to": { "file": "after.hprof", "total_shallow_bytes": 2628000, "class_count": 233, ".." : ".." },
    "net_shallow_bytes_delta": 2486712,
    "fail_over_bytes": 10485760,
    "over_threshold": false,
    "class_delta_count": 268,
    "top_class_deltas": [
      {
        "class_name": "int[]",
        "instances_from": 4,
        "instances_to": 436,
        "delta_instances": 432,
        "bytes_from": 1168,
        "bytes_to": 2089368,
        "delta_bytes": 2088200
      }
    ]
  }
}
```

`class_delta_count` covers every reported class while `top_class_deltas` honours `--top`, so a consumer can tell a truncated listing from a complete one.

`net_shallow_bytes_delta` covers the classes the report lists, which is the whole dump unless `--filter` is used. Combining the two therefore gates the selection rather than the heap: `--filter com.mycompany --fail-over 10MiB` fails only when *your* classes grew past the budget. The whole dump totals stay available as `from.total_shallow_bytes` and `to.total_shallow_bytes`.

To gate on a single class rather than on the total:

```bash
jq -e '[.diff.top_class_deltas[] | select(.delta_bytes > 1048576)] | length == 0' diff.json
```

### Example JSON

```bash
./hprof-slurp "test-heap-dumps/hprof-64.bin" --top 3 --json
```

The output file name is printed on completion (it includes a timestamp, e.g. `hprof-slurp-1780171439141.json`).

```bash
jq . hprof-slurp-<timestamp>.json
```

```JSON
{
  "schema_version": 1,
  "tool": {
    "name": "hprof-slurp",
    "version": "0.10.0"
  },
  "dump": {
    "file": "test-heap-dumps/hprof-64.bin",
    "file_size_bytes": 3214090,
    "format": "JAVA PROFILE 1.0.1",
    "id_size_bytes": 8,
    "captured_at_epoch_millis": 1515934059480,
    "captured_at_utc": "2018-01-14 12:47:39 UTC"
  },
  "heap": {
    "total_shallow_bytes": 2628000,
    "class_count": 233,
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
}
```

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

## Generate a heap dump

Heap dump files are sometimes generated in case of a JVM crash depending on your runtime configuration.

It can also be done manually by triggering a heap dump using `jmap`.

Example:

`jmap -dump:format=b,file=my-hprof-file.bin <pid>`

On Android, dump an app's heap with `am dumpheap` and pull the raw file (do not
run it through `hprof-conv`, which strips the ART/Dalvik extension records).

Example:

`adb shell am dumpheap <pid> /data/local/tmp/my-hprof-file.bin`

## Prior art of HPROF parsing

Several projects have been very useful while researching and implementing this tool.
They have provided guidance and inspiration in moments of uncertainty.

- https://github.com/monoid/hprof_dump_parser
- https://github.com/eaftan/hprof-parser

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
