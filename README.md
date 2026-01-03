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

- displays top `n` allocated classes.
- displays number of instances per class.
- displays largest instance size per class.
- displays threads stack traces.
- lists all `Strings` found.
- output results as JSON possible

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
Found a total of 2.53MiB of instances allocated on the heap.

Top 20 allocated classes:

+------------+-----------+-------------+----------------------------------------------+
| Total size | Instances |     Largest | Class name                                   |
+------------+-----------+-------------+----------------------------------------------+
|    1.99MiB |       436 |   634.78KiB | int[]                                        |
|  197.11KiB |      1991 |    16.02KiB | char[]                                       |
|   85.25KiB |       443 |     8.02KiB | byte[]                                       |
|   47.38KiB |      1516 |  32.00bytes | java.lang.String                             |
|   45.42KiB |       560 |     8.02KiB | java.lang.Object[]                           |
|   15.26KiB |       126 | 124.00bytes | java.lang.reflect.Field                      |
|   14.77KiB |       378 |  40.00bytes | java.util.LinkedList$Node                    |
|    9.94KiB |       212 |  48.00bytes | java.util.HashMap$Node                       |
|    8.91KiB |       190 |  48.00bytes | java.util.LinkedList                         |
|    8.42KiB |        98 |  88.00bytes | java.lang.ref.SoftReference                  |
|    6.05KiB |       258 |  24.00bytes | java.lang.Integer                            |
|    5.91KiB |        18 |     2.02KiB | java.util.HashMap$Node[]                     |
|    5.86KiB |       150 |  40.00bytes | java.lang.StringBuilder                      |
|    5.44KiB |       116 |  48.00bytes | java.util.Hashtable$Entry                    |
|    5.05KiB |        38 | 136.00bytes | sun.util.locale.LocaleObjectCache$CacheEntry |
|    5.00KiB |        40 | 128.00bytes | java.lang.ref.Finalizer                      |
|    3.50KiB |        32 | 112.00bytes | java.net.URL                                 |
|    3.42KiB |        73 |  48.00bytes | java.io.File                                 |
|    3.17KiB |        12 | 776.00bytes | java.util.Hashtable$Entry[]                  |
|    3.13KiB |        56 | 144.00bytes | java.lang.String[]                           |
+------------+-----------+-------------+----------------------------------------------+
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
      "allocation_size_bytes": 2091112
    },
    {
      "class_name": "char[]",
      "instance_count": 1991,
      "largest_allocation_bytes": 16400,
      "allocation_size_bytes": 201842
    },
    {
      "class_name": "byte[]",
      "instance_count": 443,
      "largest_allocation_bytes": 8208,
      "allocation_size_bytes": 87294
    }
  ],
  "top_largest_instances": [..]
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

## Limitations

- Tested only with `JAVA PROFILE 1.0.2` & `JAVA PROFILE 1.0.1` formats.
- Does not support dumps generated by 32 bits JVM.

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
