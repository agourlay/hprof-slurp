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
  toward a GC root.
- **snapshot diff** (`--diff-from` / `--diff-to`) — per-class delta in instance
  count and shallow bytes between two captures (the strongest churn signal a
  pair of static dumps can give you).

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

## Beyond the summary

The summary above is the default mode. Three additional modes drive the
deeper investigation workflow — see [USERGUIDE.md](USERGUIDE.md) for the
full reference, flag-by-flag walkthrough, and worked Android-leak example.

### `--find-referrers` — who's holding it?

```bash
hprof-slurp -i my.hprof --find-referrers java.util.ArrayList --hops 2
hprof-slurp -i my.hprof --find-referrers id:1661812752    # specific object
```

Direct + multi-hop holders for an FQ class name or specific object id.
Hop 2 is usually where the real diagnosis lives — it goes through
`Object[]`-mediated holders like `ArrayList.elementData`. Details in
[USERGUIDE §4](USERGUIDE.md#4---find-referrers--whos-holding-it).

### `--paths-from-id` — chain to a GC root

```bash
hprof-slurp -i my.hprof --paths-from-id 1661812752 --max-depth 12
```

Walks holders upward one hop at a time until a GC root is reached or
`--max-depth` is exceeded. Details in
[USERGUIDE §5](USERGUIDE.md#5---paths-from-id--chain-to-a-gc-root).

### `--diff-from` / `--diff-to` — snapshot diff (churn signal)

```bash
hprof-slurp --diff-from before.hprof --diff-to after.hprof --diff-by count
```

Per-class delta in instance count and shallow bytes between two captures —
the strongest churn signal a pair of static dumps can give you. Details in
[USERGUIDE §6](USERGUIDE.md#6---diff-from----diff-to--snapshot-diff).

### `--json` — structured output for scripts

Append `--json` to any mode for a machine-parseable sidecar. Details in
[USERGUIDE §7](USERGUIDE.md#7---json--structured-output-for-scripts).

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

### Latest build (recommended — includes referrer tracing, paths, diff)

```bash
cargo install --git https://github.com/johnneerdael/hprof-slurp
```

This downloads the source, compiles in release mode (~1–2 minutes), and
installs the binary to `~/.cargo/bin/hprof-slurp` (or
`%USERPROFILE%\.cargo\bin\hprof-slurp.exe` on Windows). To upgrade later,
rerun the same command.

### Legacy build (crates.io 0.6.3 — summary mode only)

```bash
cargo install hprof-slurp
```

The published crates.io build does not yet include `--find-referrers`,
`--paths-from-id`, or `--diff-from`/`--diff-to`.

### Verify the install

```bash
hprof-slurp --version
hprof-slurp --help
```

If you see `command not found` (or `'hprof-slurp' is not recognized` on
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

[agourlay/hprof-slurp/releases](https://github.com/agourlay/hprof-slurp/releases)
hosts the legacy summary-only binaries (no `cargo` needed). For the new
modes (`--find-referrers`, `--paths-from-id`, `--diff-from`), use the
`cargo install --git` recipe above.

## Installing the Claude Code plugin

This repo doubles as a [Claude Code](https://claude.com/claude-code) plugin
marketplace. Installing the plugin gives Claude an `analysing-heap-dumps`
skill that auto-activates whenever you mention `.hprof` files, memory
leaks, retained size, `am dumpheap`, etc. — Claude will then run
`hprof-slurp` for you, install it via `cargo install --git` if missing,
and walk through the standard triage workflow (summary → find-referrers
→ paths-from-id → diff).

### One-time: add the marketplace

In Claude Code, run:

```
/plugin marketplace add johnneerdael/hprof-slurp
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

Claude will load the skill, verify `hprof-slurp` is on PATH (and install
it via `cargo install --git https://github.com/johnneerdael/hprof-slurp`
if not), then walk you through summary, retainer tracing, and path-to-root
in the right order.

### Updating

```
/plugin marketplace update johnneerdael/hprof-slurp
/plugin update analysing-heap-dumps@analysing-heap-dumps
```

### Uninstalling

```
/plugin uninstall analysing-heap-dumps@analysing-heap-dumps
/plugin marketplace remove johnneerdael/hprof-slurp
```

The skill content lives at
[`plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md`](plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md)
if you want to read or fork it without installing.

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
