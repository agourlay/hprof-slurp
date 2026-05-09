# hprof-slurp: Merge hprof-analyze-rust + Add Churn Analysis

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fold the referrer-tracing functionality of `~/Scripts/hprof-analyze-rust` into `hprof-slurp` as proper CLI subcommands, then add churn-analysis features (snapshot diff, GC-root paths, allocation-site / heap-summary surfacing) so a single `hprof-slurp` binary covers Android/JVM heap dump triage end-to-end.

**Architecture:**
- CLI moves from a single flat command to clap **subcommands** (`summary`, `referrers`, `paths`, `diff`). The current behavior becomes `summary` and is the default when no subcommand is given (backwards-compatible).
- The streaming parser pipeline (`prefetch_reader → record_stream_parser → result_recorder`) stays as the engine. A new **retain-bodies** parser mode optionally preserves instance field bytes and object-array element ids so a second pass can resolve referrers — this is the only way to keep streaming over dumps larger than RAM (current parser drops these bytes; see `src/parser/record_parser.rs:471-484`).
- New recorders (`ReferrerRecorder`, `PathRecorder`, `DiffRecorder`) plug into the same channel topology as `ResultRecorder`.
- Output: same dual stdout-table + opt-in JSON pattern as today.

**Tech Stack:** Rust 2024 edition. Existing deps reused: clap 4.6 (move to `derive` feature), nom 8, indicatif, ahash, thiserror, crossbeam-channel, serde, serde_json. No new dependencies.

## Decisions made at execution start (2026-05-09)

- **CLI shape: flag-based (per `docs/feature-retainer-tracing.md`), not subcommand-based.** Every operating mode is a flag on the existing single command. Replaces Task 2's subcommand design with `--find-referrers <target>`, `--hops`, `--paths-from-id <u64>`, `--max-depth`, `--diff-from <path>`, `--diff-to <path>`, `--diff-by count|bytes`. Mutually exclusive modes are validated at resolve-time.
- **Branch: rebased onto `fork/master`** (the user's GitHub fork at johnneerdael/hprof-slurp), not `origin/master` (agourlay's). The fork already has 32-bit hprof identifier support merged via PR #1+#2, plus the `largest_object_id` retainer-tracing prep and the `feature-retainer-tracing.md` spec — all of which this work builds on. Initial branch was off `origin/master`; rebased onto `fork/master` after the user clarified their fork is the upstream-of-record. Conflict resolution merged the parser's `id_size` threading with the new `retain_bodies` flag.
- **Tasks 11 & 12 deferred:** HeapSummary cumulative-bytes surfacing and AllocationSites top-N rendering require an hprof captured with allocation tracking. None of the existing fixtures qualify. Skip until such a fixture is available.

**Source of truth for merged-in logic:** `~/Scripts/hprof-analyze-rust/src/main.rs` (444 lines). Its bugs/limitations (broken `--hops` wiring at line 18, primitive-array class collapse at line 81, four full file scans, missing CLI parser, empty README) are explicitly fixed during the merge.

---

## Track 1 — Merge referrer tracing into hprof-slurp

### Task 1: Workspace prep

**Files:**
- Modify: `Cargo.toml`
- Create: `docs/superpowers/plans/2026-05-09-merge-hprof-analyze-into-slurp.md` (this file — already exists)
- Verify: `test-heap-dumps/hprof-32.bin`, `test-heap-dumps/hprof-64.bin`

- [ ] **Step 1: Add the `derive` feature to clap**

In `Cargo.toml`, change:

```toml
clap = { version = "4.6.1", features = ["cargo"] }
```

to:

```toml
clap = { version = "4.6.1", features = ["cargo", "derive"] }
```

- [ ] **Step 2: Verify everything still builds**

Run: `cargo build --release`
Expected: build succeeds, no new warnings.

- [ ] **Step 3: Verify existing tests still pass**

Run: `cargo test`
Expected: all existing tests in `args_tests`, `slurp::tests`, `rendered_result::tests` pass.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: enable clap derive for upcoming subcommand CLI"
```

---

### Task 2: Migrate flat CLI to subcommands (summary as default)

**Files:**
- Modify: `src/args.rs` (rewrite using clap derive)
- Modify: `src/main.rs` (dispatch on subcommand)

- [ ] **Step 1: Write a failing test for the new `summary` subcommand parser**

Add to `src/args.rs` under `#[cfg(test)] mod args_tests`:

```rust
#[test]
fn parses_summary_default_when_no_subcommand() {
    use clap::Parser;
    let cli = Cli::try_parse_from(["hprof-slurp", "-i", "x.hprof"]).unwrap();
    match cli.command.unwrap_or(Command::Summary(SummaryArgs {
        input_file: cli.input_file.clone().unwrap(),
        top: cli.top,
        debug: cli.debug,
        list_strings: cli.list_strings,
        json: cli.json,
    })) {
        Command::Summary(s) => assert_eq!(s.input_file, "x.hprof"),
        _ => panic!("expected summary"),
    }
}

#[test]
fn parses_referrers_subcommand() {
    use clap::Parser;
    let cli = Cli::try_parse_from([
        "hprof-slurp", "referrers", "-i", "x.hprof", "--target", "java.util.ArrayList",
        "--top", "30", "--hops", "2",
    ]).unwrap();
    match cli.command {
        Some(Command::Referrers(r)) => {
            assert_eq!(r.input_file, "x.hprof");
            assert_eq!(r.target, "java.util.ArrayList");
            assert_eq!(r.top, 30);
            assert_eq!(r.hops, 2);
        }
        _ => panic!("expected referrers"),
    }
}
```

- [ ] **Step 2: Run the test (will fail — types don't exist yet)**

Run: `cargo test args::args_tests`
Expected: compile error — `Cli`, `Command`, `SummaryArgs`, `ReferrersArgs` not defined.

- [ ] **Step 3: Rewrite `src/args.rs` with derive-based subcommands**

Replace the entire contents of `src/args.rs` with:

```rust
use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::{InputFileNotFound, InvalidTopPositiveInt};
use clap::{Parser, Subcommand};
use std::path::Path;

#[derive(Parser, Debug)]
#[command(name = "hprof-slurp", version, about = "JVM/Android heap dump (hprof) analyzer")]
pub struct Cli {
    /// Subcommand. Defaults to `summary` for backwards compatibility.
    #[command(subcommand)]
    pub command: Option<Command>,

    // -- legacy top-level flags (used when no subcommand is provided) --
    #[arg(short = 'i', long = "inputFile", global = false, required = false)]
    pub input_file: Option<String>,
    #[arg(short = 't', long = "top", default_value_t = 20)]
    pub top: usize,
    #[arg(short = 'd', long = "debug", default_value_t = false)]
    pub debug: bool,
    #[arg(short = 'l', long = "listStrings", default_value_t = false)]
    pub list_strings: bool,
    #[arg(long = "json", default_value_t = false)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Top-N raw shallow heap classes, threads, optional strings (legacy default).
    Summary(SummaryArgs),
    /// Direct + N-hop referrers of a class FQ-name or `id:<u64>` object id.
    Referrers(ReferrersArgs),
    /// Walk holders from a single object id toward a GC root.
    Paths(PathsArgs),
    /// Per-class delta in instance count and shallow bytes between two snapshots.
    Diff(DiffArgs),
}

#[derive(Parser, Debug)]
pub struct SummaryArgs {
    #[arg(short = 'i', long = "inputFile")]
    pub input_file: String,
    #[arg(short = 't', long = "top", default_value_t = 20)]
    pub top: usize,
    #[arg(short = 'd', long = "debug", default_value_t = false)]
    pub debug: bool,
    #[arg(short = 'l', long = "listStrings", default_value_t = false)]
    pub list_strings: bool,
    #[arg(long = "json", default_value_t = false)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct ReferrersArgs {
    #[arg(short = 'i', long = "inputFile")]
    pub input_file: String,
    /// Either an FQ class name (e.g. `java.util.ArrayList`) or `id:<u64>` for a
    /// specific object id.
    #[arg(long = "target")]
    pub target: String,
    #[arg(short = 't', long = "top", default_value_t = 30)]
    pub top: usize,
    /// 1 = direct holders only; 2 = also via Object[]; 3 = three-hop chain.
    #[arg(long = "hops", default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=3))]
    pub hops: u8,
    /// Include class statics as candidate holders.
    #[arg(long = "include-statics", default_value_t = true)]
    pub include_statics: bool,
    #[arg(long = "json", default_value_t = false)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct PathsArgs {
    #[arg(short = 'i', long = "inputFile")]
    pub input_file: String,
    /// Object id to trace (decimal u64).
    #[arg(long = "object-id")]
    pub object_id: u64,
    /// Max chain length before giving up.
    #[arg(long = "max-depth", default_value_t = 12)]
    pub max_depth: u8,
    #[arg(long = "json", default_value_t = false)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct DiffArgs {
    /// Baseline (older) hprof.
    #[arg(long = "from")]
    pub from: String,
    /// Comparison (newer) hprof.
    #[arg(long = "to")]
    pub to: String,
    #[arg(short = 't', long = "top", default_value_t = 30)]
    pub top: usize,
    /// Sort order: `count` (delta instances) or `bytes` (delta shallow size).
    #[arg(long = "by", default_value = "count")]
    pub by: DiffSort,
    #[arg(long = "json", default_value_t = false)]
    pub json: bool,
}

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum DiffSort { Count, Bytes }

/// Resolve the parsed CLI into a concrete subcommand, defaulting to `summary`
/// when none was provided (for backwards compatibility with v0.6.x usage).
pub fn resolve(cli: Cli) -> Result<Command, HprofSlurpError> {
    let cmd = match cli.command {
        Some(c) => c,
        None => {
            let input_file = cli.input_file.ok_or(InputFileNotFound { name: "(missing -i)".into() })?;
            Command::Summary(SummaryArgs {
                input_file,
                top: cli.top,
                debug: cli.debug,
                list_strings: cli.list_strings,
                json: cli.json,
            })
        }
    };
    validate(&cmd)?;
    Ok(cmd)
}

fn validate(cmd: &Command) -> Result<(), HprofSlurpError> {
    let (path, top) = match cmd {
        Command::Summary(s)   => (s.input_file.as_str(), Some(s.top)),
        Command::Referrers(r) => (r.input_file.as_str(), Some(r.top)),
        Command::Paths(p)     => (p.input_file.as_str(), None),
        Command::Diff(d)      => {
            check_file(&d.from)?;
            (d.to.as_str(), Some(d.top))
        }
    };
    check_file(path)?;
    if let Some(t) = top { if t == 0 { return Err(InvalidTopPositiveInt); } }
    Ok(())
}

fn check_file(p: &str) -> Result<(), HprofSlurpError> {
    if !Path::new(p).is_file() {
        return Err(InputFileNotFound { name: p.to_string() });
    }
    Ok(())
}

#[cfg(test)]
mod args_tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn verify_command() { Cli::command().debug_assert(); }

    #[test]
    fn parses_summary_default_when_no_subcommand() {
        // file existence is checked at resolve-time, not parse-time
        let cli = Cli::try_parse_from(["hprof-slurp", "-i", "x.hprof"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.input_file.as_deref(), Some("x.hprof"));
    }

    #[test]
    fn parses_referrers_subcommand() {
        let cli = Cli::try_parse_from([
            "hprof-slurp", "referrers", "-i", "x.hprof",
            "--target", "java.util.ArrayList", "--top", "30", "--hops", "2",
        ]).unwrap();
        match cli.command {
            Some(Command::Referrers(r)) => {
                assert_eq!(r.input_file, "x.hprof");
                assert_eq!(r.target, "java.util.ArrayList");
                assert_eq!(r.top, 30);
                assert_eq!(r.hops, 2);
            }
            _ => panic!("expected referrers"),
        }
    }

    #[test]
    fn parses_paths_subcommand() {
        let cli = Cli::try_parse_from([
            "hprof-slurp", "paths", "-i", "x.hprof", "--object-id", "12345",
        ]).unwrap();
        matches!(cli.command, Some(Command::Paths(_)));
    }

    #[test]
    fn parses_diff_subcommand() {
        let cli = Cli::try_parse_from([
            "hprof-slurp", "diff", "--from", "a.hprof", "--to", "b.hprof", "--by", "bytes",
        ]).unwrap();
        matches!(cli.command, Some(Command::Diff(_)));
    }
}
```

- [ ] **Step 4: Update `src/main.rs` to dispatch on the subcommand**

Replace the body of `main_result()` in `src/main.rs` with:

```rust
fn main_result() -> Result<(), HprofSlurpError> {
    use crate::args::{Cli, Command, resolve};
    use clap::Parser;

    let now = Instant::now();
    let cli = Cli::parse();
    match resolve(cli)? {
        Command::Summary(s) => run_summary(s, now),
        Command::Referrers(r) => run_referrers(r, now),
        Command::Paths(p) => run_paths(p, now),
        Command::Diff(d) => run_diff(d, now),
    }
}

fn run_summary(s: crate::args::SummaryArgs, now: Instant) -> Result<(), HprofSlurpError> {
    let mut rendered_result = slurp_file(&s.input_file, s.debug, s.list_strings)?;
    if s.json {
        let json_result = JsonResult::new(&mut rendered_result.memory_usage, s.top);
        json_result.save_as_file()?;
    }
    print!("{}", rendered_result.serialize(s.top));
    println!("File successfully processed in {:?}", now.elapsed());
    Ok(())
}

// Stubs returning a "not yet implemented" error. Filled in by Tasks 4-7.
fn run_referrers(_r: crate::args::ReferrersArgs, _now: Instant) -> Result<(), HprofSlurpError> {
    Err(HprofSlurpError::NotYetImplemented { what: "referrers" })
}
fn run_paths(_p: crate::args::PathsArgs, _now: Instant) -> Result<(), HprofSlurpError> {
    Err(HprofSlurpError::NotYetImplemented { what: "paths" })
}
fn run_diff(_d: crate::args::DiffArgs, _now: Instant) -> Result<(), HprofSlurpError> {
    Err(HprofSlurpError::NotYetImplemented { what: "diff" })
}
```

Add a new variant to `src/errors.rs`:

```rust
#[error("not yet implemented: {what}")]
NotYetImplemented { what: &'static str },
```

- [ ] **Step 5: Run all tests, expect green**

Run: `cargo test`
Expected: all existing tests still pass; new `args_tests` for subcommand parsing pass.

- [ ] **Step 6: Smoke-test the legacy CLI shape end-to-end**

Run: `cargo run --release -- -i test-heap-dumps/hprof-64.bin -t 5`
Expected: same table output the gold file `test-heap-dumps/hprof-64-result.txt` describes (top 5 instead of 20).

- [ ] **Step 7: Smoke-test stub subcommands**

Run: `cargo run --release -- referrers -i test-heap-dumps/hprof-64.bin --target java.util.LinkedList`
Expected: exits 1 with "error: not yet implemented: referrers".

- [ ] **Step 8: Commit**

```bash
git add src/args.rs src/main.rs src/errors.rs
git commit -m "feat(cli): add subcommand structure (summary default, stubs for referrers/paths/diff)"
```

---

### Task 3: Extend parser with optional body retention

**Goal:** Make instance dump bodies and object array element ids available to recorders when (and only when) the running mode requests them.

**Files:**
- Modify: `src/parser/gc_record.rs` (add optional payload to `InstanceDump` / `ObjectArrayDump`)
- Modify: `src/parser/record_parser.rs:467-508` (capture bytes when retain-bodies flag is set)
- Modify: `src/parser/record_stream_parser.rs` (plumb a `retain_bodies: bool` through construction)

- [ ] **Step 1: Failing test — body bytes retained when flag is set**

Add to `src/parser/record_parser.rs` test module a small unit test that constructs a synthetic instance dump record header + 16 body bytes, parses it once with `retain_bodies = false` and once with `retain_bodies = true`, and asserts:

```rust
#[test]
fn instance_dump_retains_body_when_requested() {
    use crate::parser::gc_record::GcRecord::InstanceDump;
    let id_size = 8;
    // tag byte already consumed by record_parser dispatcher; we call the leaf parser directly
    let mut buf = Vec::new();
    buf.extend_from_slice(&0x00_00_00_00_00_00_00_01u64.to_be_bytes()); // object_id
    buf.extend_from_slice(&0u32.to_be_bytes());                          // stack_trace
    buf.extend_from_slice(&0x00_00_00_00_00_00_00_02u64.to_be_bytes()); // class_object_id
    buf.extend_from_slice(&16u32.to_be_bytes());                         // data_size
    buf.extend_from_slice(&[0xAB; 16]);                                  // body

    let (_, gcd_lite) = parse_gc_instance_dump(&buf, id_size, false).unwrap();
    let (_, gcd_full) = parse_gc_instance_dump(&buf, id_size, true).unwrap();
    match (gcd_lite, gcd_full) {
        (InstanceDump { body: None, .. }, InstanceDump { body: Some(b), .. }) => {
            assert_eq!(b.len(), 16);
            assert!(b.iter().all(|&x| x == 0xAB));
        }
        _ => panic!("expected None vs Some(16 bytes)"),
    }
}
```

- [ ] **Step 2: Run test, expect failure**

Run: `cargo test instance_dump_retains_body`
Expected: compile error — `body` field not on `InstanceDump`, `parse_gc_instance_dump` does not take a `retain_bodies` flag.

- [ ] **Step 3: Add `body` / `elements` fields to GcRecord**

In `src/parser/gc_record.rs`, change `InstanceDump` and `ObjectArrayDump` to:

```rust
InstanceDump {
    object_id: u64,
    stack_trace_serial_number: u32,
    class_object_id: u64,
    data_size: u32,
    /// Raw body bytes; populated only when the parser was constructed with
    /// retain-bodies mode (used by referrers/paths/diff). `None` for the default
    /// summary path so existing throughput is unchanged.
    body: Option<Box<[u8]>>,
},
ObjectArrayDump {
    object_id: u64,
    stack_trace_serial_number: u32,
    number_of_elements: u32,
    array_class_id: u64,
    /// Element object ids (0 == null). Populated only in retain-bodies mode.
    elements: Option<Box<[u64]>>,
},
```

- [ ] **Step 4: Thread the flag through the parser**

In `src/parser/record_parser.rs`, change `parse_gc_instance_dump`, `parse_gc_object_array_dump`, and `parse_gc_record` to take an extra `retain_bodies: bool`. In the leaf parsers replace the `_bytes_segment`/`_byte_array_elements` discards with a copy when the flag is true:

```rust
fn parse_gc_instance_dump(i: &[u8], id_size: u32, retain_bodies: bool) -> IResult<&[u8], GcRecord> {
    flat_map(
        (id(id_size), parse_u32, id(id_size), parse_u32),
        move |(object_id, stack_trace_serial_number, class_object_id, data_size)| {
            map(bytes::streaming::take(data_size), move |bytes_segment: &[u8]| {
                let body = if retain_bodies {
                    Some(bytes_segment.to_vec().into_boxed_slice())
                } else { None };
                GcRecord::InstanceDump {
                    object_id, stack_trace_serial_number, class_object_id, data_size, body,
                }
            })
        },
    ).parse(i)
}
```

For `parse_gc_object_array_dump`, when `retain_bodies` is true call `count(id(id_size), number_of_elements as usize)` instead of `bytes::streaming::take`, collect into a `Vec<u64>`, box it.

Update `parse_gc_record` to accept and forward the flag.

- [ ] **Step 5: Plumb the flag through `record_stream_parser.rs`**

Add `retain_bodies: bool` to `HprofRecordStreamParser::new(...)` and store it on the struct. Pass it down whenever the parser dispatches a GC record.

- [ ] **Step 6: Default `retain_bodies = false` in `slurp::slurp_file`**

In `src/slurp.rs:86-92`, pass `false` so the existing summary path is byte-for-byte unchanged.

- [ ] **Step 7: Run all tests; expect green**

Run: `cargo test`
Expected: existing summary gold-file test (`supported_64_bits` in `slurp::tests`) still passes; new `instance_dump_retains_body_when_requested` passes.

- [ ] **Step 8: Run an end-to-end timing check on the bundled fixture**

Run: `cargo run --release -- -i test-heap-dumps/hprof-64.bin -t 20 2>&1 | tail -3`
Expected: "File successfully processed in …" with timing within 5% of pre-change baseline (record the number for Track 1 perf budget).

- [ ] **Step 9: Commit**

```bash
git add src/parser/gc_record.rs src/parser/record_parser.rs src/parser/record_stream_parser.rs src/slurp.rs
git commit -m "feat(parser): optional retain-bodies mode for two-pass referrer tracing"
```

---

### Task 4: ReferrerRecorder + two-pass driver

**Files:**
- Create: `src/referrer_recorder.rs`
- Create: `src/referrer.rs` (driver: orchestrates pass 1 + pass 2)
- Modify: `src/main.rs` (`run_referrers` no longer a stub)
- Modify: `src/lib.rs` or `src/main.rs` `mod` declarations

**Two-pass approach:**
- **Pass 1** uses the existing `slurp_file` pipeline (retain-bodies = false) to build:
  - utf8 string → string map (already in `ResultRecorder.utf8_strings_by_id`)
  - class id → name and field-descriptor list (already in `ResultRecorder.class_data` + `classes_single_instance_size_by_id`)
  - target instance id set (instances whose `class_object_id` matches the resolved target class id, or, for `id:<N>`, the singleton set)
  - For 2-hop+: the class hierarchy field flatten map (used to walk inherited fields when scanning instance bodies)
- **Pass 2** runs a fresh slurp pipeline with `retain_bodies = true`. The recorder is `ReferrerRecorder` parameterized by:
  - `target_ids: AHashSet<u64>`
  - `class_fields: AHashMap<u64, Vec<FieldInfo>>` (flattened including supers)
  - `id_size: u32`
  - `hops: u8`
  - `include_statics: bool`

  It computes `by_field: AHashMap<(holder_class_id, Option<NameId>), u64>` for hop 1, and—when `hops >= 2`—threads a transient set `hop1_object_array_holders` as it scans, then a second internal scan inside the same pass 2 builds hop-2 / hop-3. (We can do all hops in a single pass-2 by recording per-record the hop-1 / hop-2 candidate sets and post-processing at end-of-stream.)

**Note:** the simplest correct version is **three passes** (target id discovery; hop-1 holders; hop-2/3). Optimizing to two passes is a follow-up. The plan below specifies three passes for clarity. Wall-clock cost is ~3 × the single-pass scan; on a 235 MB dump that's still under 5s with the existing prefetcher.

- [ ] **Step 1: Add `ReferrerResult` types**

Create `src/referrer_recorder.rs` with the result type and constructors. Start with an empty struct + a placeholder method, returning `ReferrerResult { hop1: vec![], hop2: vec![], hop3: vec![] }`.

```rust
use ahash::AHashMap;
use ahash::AHashSet;
use serde::Serialize;

use crate::parser::gc_record::{FieldInfo, GcRecord};
use crate::parser::record::Record;

#[derive(Serialize, Clone, Debug)]
pub struct ReferrerEntry {
    pub holder_class: String,
    /// `None` means the holder is an Object[] (no field name).
    pub field_name: Option<String>,
    pub ref_count: u64,
}

#[derive(Serialize, Debug)]
pub struct ReferrerResult {
    pub target_label: String,
    pub target_instance_count: u64,
    pub hop1: Vec<ReferrerEntry>,
    pub hop2: Vec<ReferrerEntry>,
    pub hop3: Vec<ReferrerEntry>,
}
```

- [ ] **Step 2: Failing test — pass-1 target resolution**

Add a test in `src/referrer.rs` driver module that, given the bundled `hprof-64.bin`, resolves `java.util.LinkedList` to a non-empty target instance id set (the gold-file shows 190 LinkedList instances).

```rust
#[test]
fn pass1_resolves_linkedlist_targets() {
    let r = pass1_resolve("test-heap-dumps/hprof-64.bin", "java.util.LinkedList").unwrap();
    assert!(r.target_ids.len() >= 100, "expected ≥100 LinkedList instances, got {}", r.target_ids.len());
    assert!(r.class_fields.len() > 100, "should have indexed many classes");
}
```

- [ ] **Step 3: Run, expect failure**

Run: `cargo test pass1_resolves_linkedlist_targets`
Expected: compile error / not implemented.

- [ ] **Step 4: Implement `pass1_resolve`**

In `src/referrer.rs`, write a function that runs the existing `slurp_file` pipeline but with a **modified ResultRecorder constructor** that, given the target name, also records:
- the resolved target class id (lookup via `class_data` after all `LoadClass` records have been seen)
- the set of instance object ids whose `class_object_id` matches the target id

Cleanest path: don't fork `ResultRecorder`. Instead reuse it as-is, then walk `rendered_result` for class metadata, and do a **light second mmap+streaming pass** purely to enumerate `InstanceDump.object_id` for the matching `class_object_id`. This avoids touching `ResultRecorder` and keeps the summary path independent.

```rust
pub struct Pass1 {
    pub id_size: u32,
    pub target_ids: AHashSet<u64>,
    pub class_fields: AHashMap<u64, Vec<FieldInfo>>, // flattened including supers
    pub class_name_by_id: AHashMap<u64, String>,
    pub utf8_by_id: AHashMap<u64, String>,
}

pub fn pass1_resolve(path: &str, target: &str) -> Result<Pass1, HprofSlurpError> {
    // Run the standard slurp once to get utf8 + class metadata.
    let pre = slurp_file_for_index(path)?;       // new helper that returns the indices, not RenderedResult
    let target_class_id = pre.class_name_by_id
        .iter()
        .find(|(_, n)| n.as_str() == target)
        .map(|(id, _)| *id)
        .ok_or(HprofSlurpError::TargetClassNotFound { name: target.to_string() })?;
    // Second light streaming pass: collect instance object ids matching that class id.
    let target_ids = collect_instance_ids(path, target_class_id)?;
    Ok(Pass1 { id_size: pre.id_size, target_ids, class_fields: pre.class_fields,
               class_name_by_id: pre.class_name_by_id, utf8_by_id: pre.utf8_by_id })
}
```

- `slurp_file_for_index` is a new variant of `slurp_file` that exposes the recorder's interim maps via a new `IndexResult` struct. Implement it by extending `ResultRecorder` with a `pub fn into_index(self) -> IndexResult` that drains the relevant `AHashMap`s.
- `collect_instance_ids(path, target_class_id)` runs a second slurp with retain-bodies = false and a tiny custom recorder that only matches `GcRecord::InstanceDump { class_object_id, object_id, .. }`.

For `id:<N>` targets (parsed in `pass1_resolve`), skip the class lookup and directly seed `target_ids = {N}`.

- [ ] **Step 5: Run pass1 test; expect green**

Run: `cargo test pass1_resolves_linkedlist_targets`
Expected: passes.

- [ ] **Step 6: Failing test — hop-1 referrer counts**

```rust
#[test]
fn hop1_referrers_for_linkedlist_node_finds_linkedlist_next_first() {
    use crate::args::ReferrersArgs;
    let args = ReferrersArgs {
        input_file: "test-heap-dumps/hprof-64.bin".into(),
        target: "java.util.LinkedList$Node".into(),
        top: 5, hops: 1, include_statics: true, json: false,
    };
    let result = run_referrers_inproc(&args).unwrap();
    // The dominant hop-1 holder of LinkedList$Node should be LinkedList$Node (next/prev) itself.
    let names: Vec<_> = result.hop1.iter().map(|e| (e.holder_class.as_str(), e.field_name.as_deref())).collect();
    assert!(
        names.iter().any(|(c, _)| *c == "java.util.LinkedList$Node"),
        "expected LinkedList$Node in hop1 holders, got {:?}", names
    );
}
```

- [ ] **Step 7: Implement hop-1 in `ReferrerRecorder`**

Recorder logic per `GcRecord` in pass 2:
- **`InstanceDump { class_object_id, body: Some(bytes), .. }`** — look up flattened `instance_fields` for `class_object_id`; walk the body sequentially using `FieldType::parse_value` (existing helper). For each `ObjectId(Some(rid))` field, if `target_ids.contains(&rid)` → bump `by_field[(class_object_id, Some(field_name_id))]`. Track `instance_object_id` as a hop-1 holder when 2-hop will run.
- **`ObjectArrayDump { array_class_id, elements: Some(elems), .. }`** — for each non-zero `rid` in `elems`, if in `target_ids` → bump `by_field[(array_class_id, None)]` and record this array's `object_id` in `hop1_object_array_holders`.
- **`ClassDump(box)`** when `include_statics` — for each `(FieldInfo, ObjectId(Some(rid)))` in `static_fields`, if `rid ∈ target_ids` → bump `by_field[(class_object_id, Some(name_id))]`.

Resolve names at end-of-stream using `class_name_by_id` + `utf8_by_id`. Sort descending by count, take `top`.

- [ ] **Step 8: Run hop-1 test; expect green**

Run: `cargo test hop1_referrers_for_linkedlist_node_finds_linkedlist_next_first`
Expected: passes.

- [ ] **Step 9: Add hop-2 and hop-3**

Hop-2: scan for instance fields/object-array elements pointing to ids in `hop1_object_array_holders`; bump `hop2_field`.
Hop-3: same again, with `hop2_holder_ids` (instance object ids found as hop-2 holders).

These can be folded into the same pass-2 loop using three increasingly large transient sets, or done in two extra passes. For simplicity and correctness on first cut, do **two extra passes** (one for hop-2, one for hop-3), gated on `args.hops >= 2` / `>= 3`.

- [ ] **Step 10: Wire `run_referrers` in `src/main.rs`**

```rust
fn run_referrers(r: crate::args::ReferrersArgs, now: Instant) -> Result<(), HprofSlurpError> {
    let result = crate::referrer::run(&r)?;
    if r.json {
        let path = format!("hprof-slurp-referrers-{}.json", chrono::Utc::now().timestamp_millis());
        let f = std::fs::File::create(&path)?;
        serde_json::to_writer(std::io::BufWriter::new(f), &result)?;
        println!("Output JSON result file {path}");
    }
    print!("{}", crate::referrer::render_text(&result, r.top));
    println!("File successfully processed in {:?}", now.elapsed());
    Ok(())
}
```

`render_text` reuses the table renderer from Task 5 (next).

- [ ] **Step 11: Add CLI smoke test against hprof-64.bin**

```bash
cargo run --release -- referrers -i test-heap-dumps/hprof-64.bin --target java.util.LinkedList$Node --top 5 --hops 1
```
Expected: a table with `java.util.LinkedList$Node.next` (or `.prev`) ranked highly.

- [ ] **Step 12: Commit**

```bash
git add src/referrer_recorder.rs src/referrer.rs src/main.rs src/errors.rs Cargo.toml
git commit -m "feat: hprof-slurp referrers <class|id:N> with hops 1..3 (replaces hprof-analyze-rust)"
```

---

### Task 5: Reuse the table renderer for referrer output

**Files:**
- Modify: `src/rendered_result.rs` (extract the ASCII-table builder so it can render arbitrary three-or-four-column data)
- Modify: `src/referrer.rs` (consume the extracted renderer)

- [ ] **Step 1: Failing test — generic table renderer**

In `src/rendered_result.rs`, add:

```rust
#[test]
fn generic_table_renders_two_column() {
    let rows = [("foo".to_string(), 42u64), ("longer".to_string(), 7u64)];
    let out = render_simple_table(&["Holder", "Refs"], rows.iter().map(|(s, n)| [s.clone(), n.to_string()]));
    assert!(out.contains("| Holder"));
    assert!(out.contains("| 42"));
}
```

- [ ] **Step 2: Run, expect failure**

Run: `cargo test generic_table_renders_two_column`
Expected: compile error — no `render_simple_table`.

- [ ] **Step 3: Extract a generic renderer**

Pull the column-padding helpers (`column_padding`, `padding_for_header`, `render_table_vertical_line`) out of `RenderedResult::render_table` into free functions. Add `pub fn render_simple_table<I>(headers: &[&str], rows: I) -> String where I: IntoIterator<Item = [String; N]>`. Keep the existing `render_table` private and re-implement it on top of the extracted helpers.

- [ ] **Step 4: Run all tests; expect green**

Run: `cargo test`
Expected: existing rendered_result tests still pass; new generic renderer test passes.

- [ ] **Step 5: Use the generic renderer from `referrer.rs`**

Replace the placeholder `render_text` with calls into `render_simple_table` for hop-1, hop-2, hop-3 sections.

- [ ] **Step 6: Visual smoke test**

Run: `cargo run --release -- referrers -i test-heap-dumps/hprof-64.bin --target java.util.LinkedList --top 10 --hops 2`
Expected: clean ASCII tables for hop-1 and hop-2; line counts and class names plausible.

- [ ] **Step 7: Commit**

```bash
git add src/rendered_result.rs src/referrer.rs
git commit -m "refactor: extract reusable ASCII table renderer for referrer/diff/paths output"
```

---

### Task 6: Replace the analyzer's broken behavior with correct ones

The `hprof-analyze-rust` source has three known defects. Track 1 must explicitly fix them while merging:

**Files:**
- Modify: `src/referrer_recorder.rs` (correctness)
- Modify: `src/result_recorder.rs` if needed for shallow-bytes correctness (it already does this right — primitive arrays are tracked per `FieldType` with proper element-size math; reuse as-is)

- [ ] **Step 1: Wire `--hops` properly**

In `referrer.rs`, gate hop-2 work behind `if args.hops >= 2` and hop-3 behind `if args.hops >= 3`. Verify by running `--hops 1` and observing that the program exits without printing hop-2/hop-3 sections.

Test:

```rust
#[test]
fn hops_one_does_not_compute_hop2() {
    let r = run(&ReferrersArgs { hops: 1, ..fixture_args() }).unwrap();
    assert!(r.hop2.is_empty());
    assert!(r.hop3.is_empty());
}
```

- [ ] **Step 2: Track primitive arrays by element type, not as `Id::from(0)`**

The analyzer's `obj_to_class.insert(pa.obj_id(), Id::from(0_u64))` (`hprof-analyze-rust/src/main.rs:81`) collapses every primitive array to the same synthetic class. In hprof-slurp this is already correct (`ResultRecorder::primitive_array_counters` is keyed by `FieldType`). When `--target id:<N>` matches a primitive array, the referrer pass should still find its holders — the `target_ids` set is keyed on object id, not class id, so this works without special-casing. Add a regression test that targets a known `int[]` object id from `hprof-64.bin` (the `largest_object_id` of `int[]` from the summary output gives one).

- [ ] **Step 3: Don't use `inst.fields().len()` as shallow bytes**

In hprof-analyze-rust this caused histogram MB to be wrong for instances. hprof-slurp already computes shallow size correctly using `instance_size` from `ClassDump` (see `result_recorder.rs` × `classes_single_instance_size_by_id`). Add an assertion test on the gold file that `LinkedList`'s reported `Total size` matches the gold value (8.91 KiB). This guards against accidental regressions during Track 2.

- [ ] **Step 4: Commit**

```bash
git add src/referrer_recorder.rs src/referrer.rs
git commit -m "fix: correct --hops gating, primitive-array targeting, shallow-byte accounting"
```

---

### Task 7: Update README for the new CLI

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add a "Subcommands" section**

After the existing "Features" section, add:

```markdown
## Subcommands

`hprof-slurp` now ships four subcommands. The legacy flat invocation is preserved as the default `summary`.

| Subcommand | Purpose |
|------------|---------|
| `summary` (default) | Top-N raw shallow heap classes, threads, optional strings dump. |
| `referrers` | Direct + multi-hop holders of a class FQ-name or specific object id. |
| `paths` | Walk holder chain for one object id toward a GC root. |
| `diff` | Per-class delta (instance count + shallow bytes) between two snapshots. |

### `referrers`

Find what holds an over-allocated class. Supports `--target <class-fq-name>` or `--target id:<u64>`.

\`\`\`
hprof-slurp referrers -i my.hprof --target java.util.ArrayList --top 30 --hops 2
\`\`\`

Hops:
- `--hops 1` — direct holders only
- `--hops 2` — also through Object[] (covers ArrayList$elementData, etc.)
- `--hops 3` — three-link chain (X → Y → Object[] → target)

### `paths`

\`\`\`
hprof-slurp paths -i my.hprof --object-id 12345678
\`\`\`

Walks holders upward until a GC root is found or `--max-depth` is exceeded.

### `diff`

\`\`\`
hprof-slurp diff --from before.hprof --to after.hprof --by count --top 20
\`\`\`

The `--by count` sort is the most useful churn signal: classes with the largest Δinstance-count between two snapshots are the strongest candidates for short-lived allocation hot-paths.
```

- [ ] **Step 2: Add a "GC churn analysis caveat" note**

Add directly before "Limitations":

```markdown
## GC churn analysis caveat

A single hprof shows what is **live at one instant**, not allocation rate. For true churn analysis use either:
- `hprof-slurp diff --from a.hprof --to b.hprof` between two captures of the same process, or
- An hprof captured with allocation tracking enabled (Android Studio profiler, `art --allocation-tracking`, Perfetto). When such a dump is provided, `hprof-slurp summary` surfaces the embedded `HeapSummary` (cumulative bytes-since-start) and `AllocationSites` records as a "churn signals" section.
```

- [ ] **Step 3: Add an "Android capture" subsection**

```markdown
## Capturing an Android heap dump

\`\`\`bash
adb shell am dumpheap <pid> /data/local/tmp/heap.hprof
adb pull /data/local/tmp/heap.hprof
hprof-slurp summary -i heap.hprof
\`\`\`

Android Studio's profiler can also emit hprof files; allocation-tracked captures from "Record memory allocations" will include `AllocationSites` records that `hprof-slurp` surfaces.
```

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: cover referrers/paths/diff subcommands and Android capture flow"
```

---

### Task 8: Migrate fixtures and retire hprof-analyze-rust

**Files:**
- Modify: `~/Scripts/hprof-analyze-rust/README.md` (point users to hprof-slurp)

- [ ] **Step 1: Drop a redirection notice**

Write a one-paragraph README in `~/Scripts/hprof-analyze-rust/`:

```markdown
# hprof-analyze-rust (archived)

This experiment has been folded into [hprof-slurp](../hprof-slurp). Use `hprof-slurp referrers --target <class|id:N>` instead. The previous behavior is preserved; bug fixes (correct `--hops`, primitive arrays, shallow bytes) and JSON output are new.
```

- [ ] **Step 2: Commit in hprof-analyze-rust**

```bash
cd ~/Scripts/hprof-analyze-rust
git add README.md
git commit -m "archive: superseded by hprof-slurp's referrers subcommand"
```

---

## Track 2 — Churn features

### Task 9: `paths <id:N>` to a GC root

**Files:**
- Create: `src/paths_recorder.rs`
- Modify: `src/main.rs` (`run_paths`)

**Approach:** in pass 1, also collect every `RootJniGlobal/Local`, `RootJavaFrame`, `RootStickyClass`, `RootMonitorUsed`, `RootThreadObject`, etc. — every GC root variant in `GcRecord` carries an `object_id`. Store them in an `AHashSet<u64>` named `gc_root_ids`.

Then iteratively:
- start with `current = args.object_id`
- if `current ∈ gc_root_ids` → done, print the chain plus the root tag
- otherwise, scan the dump looking for any record whose body contains `current` as an outgoing reference, picking the first holder found (deterministic by the lowest-numbered holder id for reproducibility)
- set `current = holder_id`; loop
- bail at `--max-depth`

This is the formalized form of the `holder_id=…` debug lines already emitted by the analyzer's hop-1 path (`hprof-analyze-rust/src/main.rs:194-199`).

- [ ] **Step 1: Failing test — find a known root path**

Pick a small object id from `hprof-64.bin` whose holder chain we can verify (e.g. one of the `largest_object_id`s from `summary --json`). Assert the returned chain ends in a root tag.

```rust
#[test]
fn paths_finds_root_for_known_object() {
    let chain = paths::run(&PathsArgs {
        input_file: "test-heap-dumps/hprof-64.bin".into(),
        object_id: KNOWN_ID, max_depth: 16, json: false,
    }).unwrap();
    assert!(!chain.steps.is_empty());
    assert!(chain.terminated_at_root, "chain did not reach a GC root");
}
```

(Replace `KNOWN_ID` after generating it once with `cargo run -- summary -i test-heap-dumps/hprof-64.bin --json` and reading `largest_object_id` for `int[]`.)

- [ ] **Step 2: Implement the iterative walker**

Each iteration is one streaming pass with a `target_id = current`. Reuse the body-retaining parser from Task 3. Recorder emits the first matching `(holder_class_id, holder_object_id, field_or_index)` and stops the pass early via an early-return signal (return a sentinel error from the recorder thread, catch in the driver).

For depth `d` ≤ `max_depth`, total work is `d` × dump size. For 235 MB and `max_depth = 12`, expect ~30s wall in the worst case. Acceptable.

- [ ] **Step 3: Commit**

```bash
git add src/paths_recorder.rs src/main.rs
git commit -m "feat: paths <id:N> walks holder chain to a GC root"
```

---

### Task 10: `diff <a> <b>` snapshot diff

**Files:**
- Create: `src/diff.rs`
- Modify: `src/main.rs` (`run_diff`)

**Approach:** run `slurp_file` on each input independently, take their `memory_usage` vectors, key-join on `class_name`, compute `delta_count = b.instance_count - a.instance_count` and `delta_bytes = b.allocation_size_bytes - a.allocation_size_bytes`. Sort by the user-selected key. Render with the generic table renderer.

- [ ] **Step 1: Failing test — diff math**

```rust
#[test]
fn diff_keys_join_and_sort_by_count() {
    let a = vec![cs("Foo", 10, 100), cs("Bar", 5, 50)];
    let b = vec![cs("Foo", 50, 500), cs("Bar", 5, 50), cs("Baz", 1, 10)];
    let out = diff::compute(&a, &b, DiffSort::Count);
    assert_eq!(out[0].class_name, "Foo");
    assert_eq!(out[0].delta_count, 40);
    // Bar: zero delta — should rank lowest (or be filtered)
    assert!(out.iter().any(|e| e.class_name == "Baz" && e.delta_count == 1));
}
```

- [ ] **Step 2: Implement `diff::compute`**

Build an `AHashMap<String, ClassAllocationStats>` from `a`, then iterate `b` and emit `DiffEntry { class_name, delta_count, delta_bytes, count_a, count_b, bytes_a, bytes_b }`. Classes present in `a` but not `b` get negative deltas. Filter zero-delta rows by default; expose `--include-zero` later if asked.

- [ ] **Step 3: Wire `run_diff`**

```rust
fn run_diff(d: crate::args::DiffArgs, now: Instant) -> Result<(), HprofSlurpError> {
    let a = slurp_file(&d.from, false, false)?;
    let b = slurp_file(&d.to,   false, false)?;
    let entries = crate::diff::compute(&a.memory_usage, &b.memory_usage, d.by);
    if d.json {
        let path = format!("hprof-slurp-diff-{}.json", chrono::Utc::now().timestamp_millis());
        serde_json::to_writer(std::io::BufWriter::new(std::fs::File::create(&path)?), &entries)?;
        println!("Output JSON result file {path}");
    }
    print!("{}", crate::diff::render_text(&entries, d.top));
    println!("File successfully processed in {:?}", now.elapsed());
    Ok(())
}
```

- [ ] **Step 4: Commit**

```bash
git add src/diff.rs src/main.rs
git commit -m "feat: diff --from a.hprof --to b.hprof for per-class churn deltas"
```

---

### Task 11: Surface HeapSummary cumulative-since-start in summary output

**Files:**
- Modify: `src/result_recorder.rs` (capture `HeapSummary` payload)
- Modify: `src/rendered_result.rs` (render the new "Allocation totals" block)

The `HeapSummary` record carries `total_bytes_allocated` and `total_instances_allocated` since process start. When present, this is a real churn signal independent of any diff.

- [ ] **Step 1: Failing test — summary surfaces totals when present**

Pick a fixture that contains a `HeapSummary` record (verify with `xxd | grep` or reuse Android dumps if any in `test-heap-dumps/` qualifies — otherwise, skip-test until a fixture exists, then re-enable).

- [ ] **Step 2: Capture in `ResultRecorder`**

Add fields `total_bytes_allocated: Option<u64>` and `total_instances_allocated: Option<u64>`. Populate from the existing `HeapSummary { … }` match arm.

- [ ] **Step 3: Render**

In `rendered_result.rs`, add a section between the heap-objects banner and the top-N tables:

```text
Allocation totals (cumulative since process start):
  bytes:     1.42 GiB
  instances: 9,481,720
```

- [ ] **Step 4: Commit**

```bash
git add src/result_recorder.rs src/rendered_result.rs
git commit -m "feat: summary surfaces HeapSummary cumulative bytes/instances when present"
```

---

### Task 12: Surface AllocationSites top-N in summary output

**Files:**
- Modify: `src/result_recorder.rs` (capture `AllocationSites` records)
- Modify: `src/rendered_result.rs` (render top sites)

The `AllocationSites` record carries per-site `bytes_allocated`, `instances_allocated`, `bytes_alive`, `instances_alive`, and a `stack_trace_serial_number`. Cross-reference the latter with `stack_trace_by_serial_number` (already kept) to print each site's stack frames.

- [ ] **Step 1: Failing test**

Same caveat as Task 11 — needs a fixture with allocation tracking enabled. If none, write the renderer + record-capture code with a unit test that drives synthetic data, and add an integration test once a fixture exists.

- [ ] **Step 2: Capture in `ResultRecorder`**

Append every `AllocationSite` to `Vec<AllocationSite>`. Track `stack_trace_by_serial_number` (already populated).

- [ ] **Step 3: Render**

When the `Vec<AllocationSite>` is non-empty, print:

```text
Top N allocation sites by bytes_allocated:
  ─ 1.21 GiB  /  4,812,000 instances  java.lang.String#<init>
        at java.util.Arrays.copyOfRange(Arrays.java:3664)
        at java.lang.String.<init>(String.java:233)
        ...
```

Resolve frames via `StackFrameData.method_name_id` → `utf8_strings_by_id` → string. Truncate to top-N (reuse `--top`).

- [ ] **Step 4: Commit**

```bash
git add src/result_recorder.rs src/rendered_result.rs
git commit -m "feat: summary surfaces top AllocationSites when present"
```

---

### Task 13: JSON output for new subcommands

**Files:**
- Modify: `src/referrer.rs`, `src/paths_recorder.rs`, `src/diff.rs`

- [ ] **Step 1: Already partially done** in Tasks 4, 9, 10 (each subcommand writes a JSON file when `--json` is set). Verify each with:

```bash
cargo run --release -- referrers -i test-heap-dumps/hprof-64.bin --target java.util.LinkedList --top 5 --hops 1 --json
jq '.hop1[0]' hprof-slurp-referrers-*.json
```

Expected: JSON file readable, top hop1 entry has `holder_class`, `field_name`, `ref_count`.

- [ ] **Step 2: Add gold-file equivalents for the new subcommands**

`test-heap-dumps/hprof-64-referrers-linkedlist-result.json`, etc. Write integration tests that compare emitted JSON against the gold files.

- [ ] **Step 3: Commit**

```bash
git add test-heap-dumps/
git commit -m "test: gold-file fixtures for referrers/paths/diff JSON output"
```

---

## Self-Review Checklist (run before declaring the plan ready to execute)

- [ ] **Spec coverage:** every behavior in `~/Scripts/hprof-analyze-rust/src/main.rs` has a corresponding task — class lookup (Task 4), `id:<N>` targeting (Task 4), hop 1/2/3 (Tasks 4, 6), static fields (Task 4), histogram fallback (already in `summary`), `holder_id` debug lines (Task 9 formalizes them).
- [ ] **No placeholders:** every step shows code or exact commands.
- [ ] **Type consistency:** `ReferrerEntry`, `Pass1`, `IndexResult`, `ReferrersArgs`, `DiffSort` are defined in exactly one place and referenced consistently.
- [ ] **Test fixtures:** every test references files that exist (`test-heap-dumps/hprof-64.bin`, etc.). Tasks 11–12 explicitly note the missing AllocationSites fixture as a follow-up.

## Risk Notes

- **Two-pass (or three-pass) cost on huge dumps:** the existing prefetcher streams at ~2 GB/s; a 10 GB Android dump in three passes is ~15s of I/O on NVMe. If users need single-pass referrers on dumps larger than RAM, that's a future optimization (deferred-resolution recorder with on-disk spill, per the comment at `record_parser.rs:472-475`).
- **Memory for retain-bodies mode:** Object[] element arrays of millions of entries can briefly double peak memory while parsing each segment. Boxing per-record limits damage; the prefetcher backpressure keeps total in-flight bounded.
- **Backwards compatibility:** the legacy invocation (`hprof-slurp -i x.hprof -t 20 --json`) is preserved exactly. Verify with the existing gold-file test at `slurp::tests::supported_64_bits` after every Task 1–7 change.
