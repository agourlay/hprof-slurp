# Feature proposal: retainer (reverse-reference) tracing

## Motivation

`hprof-slurp` already surfaces *what* the heap holds (per-class allocation
totals, largest single instances, and — with the `largest_object_id` patch —
which specific instance is the largest of its class). What it does not show
is *who holds it*: when you find a 54.93 MiB single `char[]`, you need a
holder chain to know whether it's a JSON parse buffer, a logcat trace, a
serialized snapshot, or an in-memory image.

Traditional Java heap analyzers (Eclipse MAT, YourKit, JProfiler) answer
this with retained-size + path-to-GC-roots queries. We don't need anything
that fancy. A 1-hop direct-referrer histogram covers ~95% of the diagnostic
work.

## Proposed CLI

```
# Find what fields/arrays directly point at object_id 66277392
hprof-slurp -i heap.hprof --find-referrers 66277392

# Same, but for every instance of a class
hprof-slurp -i heap.hprof --find-referrers java.lang.String

# Trace 2 hops (what holds the Object[] arrays that hold the target)
hprof-slurp -i heap.hprof --find-referrers 66277392 --hops 2
```

Output:

```
Direct referrers of object_id 66277392 (1 instance):
  java.lang.String.value                                                   1
```

## Architectural challenge

hprof-slurp's existing parser is streaming: `GcRecord::InstanceDump` stores
`object_id`, `class_object_id`, `data_size` — but discards the actual
instance bytes. Same for `GcRecord::ObjectArrayDump` (keeps `array_class_id`
and `number_of_elements`, discards the element ids).

Retainer tracing needs either:

1. **In-memory retention of all instance/array data** — too expensive for
   large dumps (the data we'd need to keep can be 50%+ of the dump size).

2. **A second parsing pass** — read the file twice. First pass: collect
   `(class_object_id → instance_field_layout)` and resolve the target id
   set. Second pass: re-read each `InstanceDump`'s payload bytes, walk
   field descriptors, check each `OBJECT`-typed field's value against the
   target set.

(2) is the right tradeoff. The pass-2 read is sequential and prefetched, so
on warm OS cache the cost is comparable to the first pass.

## Implementation sketch

### New parser variants (or option flag on existing variants)

`InstanceDump` currently only emits `data_size`. We need an alternate path
that emits the bytes. Add a second parsing mode controlled by a flag:

```rust
pub enum InstanceDumpMode {
    SizeOnly,           // current behavior: emit data_size, skip body
    WithFields,         // emit Vec<u8> field block for retainer scanning
}
```

In streaming mode (default), prefer `SizeOnly`. When `--find-referrers`
is set, hprof-slurp does a first pass in `SizeOnly` mode to build the
class-layout index and resolve the target set, then re-reads the file in
`WithFields` mode for pass 2.

For `ObjectArrayDump`, similarly: emit either `count` or `Vec<u64>` of
element ids.

### Class layout index

Already populated as a side effect of `ClassDump` records: the
`ClassInfo` struct in `result_recorder.rs` already keeps
`instance_field_types: Vec<FieldType>`. Promote it from a private detail
to a published structure usable by pass 2.

Class layouts are inherited: an instance's bytes contain its own class's
fields, then super class's fields, etc. The pass-2 walker needs to do
the same hierarchy walk.

### Pass-2 walker

```rust
struct ReferrerWalker<'a> {
    target_ids: &'a HashSet<u64>,
    classes: &'a HashMap<u64, ClassInfo>,
    utf8_by_id: &'a HashMap<u64, String>,
    holder_histogram: HashMap<(u64, u64), u64>,  // (holder_class_id, name_id) -> count
}
```

For each `InstanceDump` with body bytes, walk the field descriptors of
`class_object_id`'s hierarchy. For each `OBJECT` field, decode the
identifier (4 or 8 bytes depending on `id_size`) and look it up in
`target_ids`. On hit, increment `holder_histogram[(class_id, name_id)]`.

For each `ObjectArrayDump` with element ids, scan each element. On hit,
increment a per-array-class entry (no field name; use a sentinel "[]").

### Multi-hop

For `--hops N` where N > 1: pass 2 finds 1-hop holders. Run pass 3 with
the holder ids as the new target set. Each hop is one re-read of the
file. Cap N at 5 to avoid pathological loops.

### CLI plumbing

Add to `args.rs`:

```rust
#[arg(long)]
find_referrers: Option<String>,  // accepts "<u64>" or "<class fq name>"

#[arg(long, default_value = "1")]
hops: u8,
```

Resolve the string in `slurp.rs`:

```rust
let target_ids = match find_referrers {
    Some(s) if s.parse::<u64>().is_ok() => HashSet::from([s.parse().unwrap()]),
    Some(s) => /* find class id by name, collect all instance ids */,
    None => return /* skip retainer phase */,
};
```

### Performance

The first pass already takes ~130 ms on a 305 MiB dump. Pass 2 will be
slightly slower because we read field bytes (was skipped before) but the
total budget is comfortably under 1 second. With multi-hop, multiply by
N — still cheap.

## Compatibility

Existing CLI invocations are unaffected. Retainer tracing is opt-in via
`--find-referrers`. JSON output (`--json`) gains an optional
`referrers` array if `--find-referrers` was set.

## Testing

- Unit test: a tiny hand-crafted hprof with a known field reference, run
  `--find-referrers <known-id>`, assert output names the right field.
- Integration: real Android dumps (the existing `test-heap-dumps/` set
  is JVM-only; add a 32-bit Android sample for parity).

## References

- Real-world need: https://github.com/agourlay/hprof-slurp/issues/<TBD>
- Adjacent project (`jvm-hprof` Rust crate) does this end-to-end but is
  10x slower because it materializes all instance bytes by default. Our
  two-pass model is the streaming equivalent.

## Status

- 2026-05-09: spec drafted while diagnosing a Modern Home memory leak in
  com.nexio.tv. Patched a one-off Rust analyzer (`/tmp/hprof-analyze-rust/`)
  to do this; consolidating the capability into hprof-slurp is the right
  long-term home.
