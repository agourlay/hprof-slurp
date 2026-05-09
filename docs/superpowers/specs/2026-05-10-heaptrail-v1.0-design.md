# heaptrail v1.0.0 — Retained size via dominator tree (feature E)

**Status:** Locked, ready for plan.

**Decisions:**
- Single `--retained-size` flag, integrated across **summary, `--paths-from-id`, `--find-referrers`** (mirrors v0.9.0's single-flag-many-surfaces pattern).
- **Lengauer-Tarjan** O(N α(N)) immediate-dominator algorithm. In-memory CSR graph.
- Output granularity: **classes + top-N instances** — the summary class table sorts by retained size when the flag is on; a new "Largest retained instances" block lists `(object_id, class, retained_bytes)` so the user can pivot directly into `--paths-from-id <id>` / `--preview-bytes`.

---

## 1. Problem

`summary` ranks classes by **shallow** size — bytes occupied by the instance header + own fields. For graphs whose root is a small object holding many big children (the engineering example was `ResolvedDisplayItem`: 88 bytes shallow, multi-KiB retained), shallow size badly under-represents the cost of allowing that class to live.

The user pain shape is "is this 35,000-instance class actually 35 MB or actually 350 MB?" — a question shallow size can't answer. Eclipse MAT answers it via dominator tree but at the cost of a GUI, full-RAM load, and minutes of wall time. heaptrail's competitive position is "MAT-grade triage in seconds at the CLI"; retained size is the last MAT-grade datum we don't surface.

## 2. Goals & non-goals

**Goals:**
- `--retained-size` flag adds a retained-bytes column to the summary class table; sorts the table by retained when set.
- New "Largest retained instances" output section lists top-N `(object_id, class, retained_bytes)`.
- `--paths-from-id` annotates each hop with the hop object's retained size.
- `--find-referrers` annotates each holder class with the class's retained size.
- Default `summary` behavior **byte-identical** to v0.9.0 when the flag is unset.
- Both canonical fixtures (`JAVA_PROFILE_1.0.2.hprof`, `JAVA_PROFILE_1.0.3.hprof`) covered by smoke tests.

**Non-goals:**
- No GUI, no per-package or per-namespace retained rollups, no histogram-of-retainees.
- No incremental / streaming dominator computation. v1.0.0 builds the full graph in memory; users who can't afford the working memory budget skip the flag.
- No retained-size deltas in `--diff-from`/`--diff-to`. Could come later; out of scope.
- No "what would freeing this class save?" hypothetical-retained / exclusive-retained metric. Plain dominator-tree retained only.
- **No reference-strength filtering.** v1.0.0 includes weak / soft / phantom-reference edges in the graph and therefore in retained-size sums — that is the strict graph-theoretic dominator-tree definition. MAT's default leak-hunting workflow excludes those edges, so a side-by-side comparison will show MAT's retained smaller than heaptrail's for any object reachable only via a `WeakReference`/`SoftReference`/`PhantomReference`. Selective exclusion ships in v1.1+ as `--exclude-soft-weak`, which rebuilds the graph dropping outgoing edges from `java.lang.ref.{Soft,Weak,Phantom}Reference` subclasses. Document this in the USERGUIDE so users don't read MAT-vs-heaptrail divergence as a bug.

## 3. Architecture

### 3.1 Reference-graph pipeline

```
   pass 1A (already exists in referrer.rs::pass1_index):
      utf8 + classes + layouts + GC roots → Pass1Index

   pass 2 (NEW: reference_graph::build):
      stream the hprof with retain_object_references=true
      for each instance dump: walk class layout (own + super chain) using
                              Pass1Index.fields_by_class_id, extract object-typed
                              field values → emit edges
      for each object-array dump: emit each non-zero element_id → edge
      collect static object refs from Pass1Index.static_object_fields_by_class_id
      → CSR graph: { node_ids, node_class, node_shallow, edges_csr, roots }

   pure compute (NEW):
      dominators::lengauer_tarjan(graph)               → idom[]
      retained::compute(graph, idom)                   → retained[]
      retained::aggregate(graph, retained)             → class_retained_map +
                                                          top_n_instances
```

### 3.2 Modules (new files)

| File | Responsibility |
|------|----------------|
| `src/reference_graph.rs` | Build the CSR object-reference graph from a parsed hprof + Pass1Index. Pure data-pipeline; no algorithm. |
| `src/dominators.rs` | Lengauer-Tarjan immediate-dominator computation. Pure algorithm: takes graph, returns `Vec<u32>` of `idom`. |
| `src/retained.rs` | Post-order DFS on dominator tree to compute retained sizes; rolls up to class totals + top-N instance hot list. |

### 3.3 Modified files

| File | Change |
|------|--------|
| `src/parser/record_parser.rs` | New mode flag `retain_object_references: bool`. When set, `InstanceDump` emits `body: Some(_)` so downstream code can extract refs (this overlaps with existing `retain_bodies`; the new flag is a strict subset gate — bodies retained, but the consumer extracts refs and discards). |
| `src/parser/record_stream_parser.rs` | New constructor variant or extend `with_modes`. |
| `src/result_recorder.rs` | When summary mode runs with `--retained-size`, the recorder consumes the reference graph + dominator output and populates `RenderedResult.class_retained_sizes` + `RenderedResult.top_retained_instances`. |
| `src/rendered_result.rs` | New optional fields. `render_memory_usage` extends class table with retained column when the map is `Some`; new `render_retained_instances` block. |
| `src/args.rs` | New flag `--retained-size`. Propagated to `Mode::Summary`, `Mode::Paths`, `Mode::FindReferrers`. |
| `src/main.rs` | `run_summary` accepts `retained_size: bool`; calls a new `slurp_file_with_retained` (or extends `slurp_file_with_preview`). |
| `src/paths.rs` | When `retained_size` set in `Mode::Paths`, build the graph + idom + retained vector once during `paths::run`; annotate each rendered hop with `retained_size_of(node_index_for_object_id)`. |
| `src/referrer.rs` | When `retained_size` set in `Mode::FindReferrers`, do the same; annotate holder class entries with `class_retained_sizes[holder_class_id]`. |

### 3.4 Data structures

```rust
// src/reference_graph.rs
pub struct ReferenceGraph {
    /// Sorted by object_id. node_index N is reserved for the virtual super-root
    /// that points to every GC root (so dominator tree has a single entry).
    pub node_ids: Vec<u64>,
    pub node_class: Vec<u32>,           // class index, u32::MAX for the super-root
    pub node_shallow: Vec<u32>,
    pub class_ids: Vec<u64>,            // class_index → class_object_id
    pub edges_offsets: Vec<u32>,        // length N+2; CSR row pointers
    pub edges_targets: Vec<u32>,        // length = total edges
    pub super_root: u32,                // == N
}

impl ReferenceGraph {
    /// Returns (root + reachable count, total node count). Unreachable nodes
    /// have idom == u32::MAX after LT and contribute 0 to retained sums.
    pub fn node_index_of(&self, object_id: u64) -> Option<u32>;
}

// src/dominators.rs
/// Lengauer-Tarjan. Returns immediate-dominator index per node.
/// `idom[super_root] = u32::MAX` sentinel; unreachable nodes also `u32::MAX`.
pub fn lengauer_tarjan(graph: &ReferenceGraph) -> Vec<u32>;

// src/retained.rs
pub struct RetainedAnalysis {
    pub retained: Vec<u64>,                            // per-node retained bytes
    pub class_retained: AHashMap<u64, u64>,            // class_object_id → retained
    pub top_instances: Vec<(u64, u64, u64)>,           // (object_id, class_object_id, retained_bytes)
}
pub fn compute(graph: &ReferenceGraph, idom: &[u32], top_n: usize) -> RetainedAnalysis;

// src/rendered_result.rs (additions)
pub struct RenderedResult {
    // ... existing fields ...
    pub class_retained_sizes: Option<AHashMap<u64, u64>>,
    pub top_retained_instances: Option<Vec<(u64, u64, u64)>>,
}
```

### 3.5 Edge representation — decision

`ReferenceGraph.edges_targets: Vec<u32>` stores **target node indices only** — no per-edge field-name id, no per-edge reference-strength tag. Rationale:

- Retained-size aggregation is label-free: it sums shallow bytes in the dominator-tree post-order, not per-field.
- Reference-strength filtering (v1.1.0 `--exclude-soft-weak`) can be implemented at the **node** level: at graph-build time, detect when a source class is a `java.lang.ref.{Soft,Weak,Phantom}Reference` subclass, and drop **all** of its outgoing edges. The `Reference.referent` field is effectively the only outgoing object reference on those classes that matters; suppressing the source-node's whole edge fan is equivalent to per-edge filtering for retained-size purposes. Class-id set lookup at build time costs nothing.
- Edge labels would cost ~30 MiB on a 200 MiB Android dump (`Vec<u16>` of length 15M edges). The only feature that would justify it is "merged shortest paths with field labels," which is independently scheduled for v1.1.0 but doesn't need per-edge labels — the renderer can resolve labels on demand by walking the source class layout for the few hops it needs to print.

Decision: **edges stay `u32` for the v1.x line.**

### 3.6 Internal API stability

`ReferenceGraph`, `lengauer_tarjan`, and `RetainedAnalysis` (with its `retained`, `class_retained`, and `top_instances` fields) are part of heaptrail's **internal v1.x API contract**. v1.1+ features depend on them:

- **`--leak-suspects`** consumes `retained` and `idom` to rank dominator-tree subtrees by retained share.
- **`--exclude-soft-weak`** modifies the *graph build* (drops outgoing edges from `Reference` subclasses) but reuses the dominator + retained pipeline as-is.
- **`--merge-paths`** consumes `idom` to fold paths that converge at the same dominator.

These types must not be renamed, restructured, or have their semantics changed without bumping the major version. New optional fields are fine; renaming `class_retained` is not. The crate is a binary so this contract is internal — the constraint is on heaptrail's own development cadence, not on downstream API users.

### 3.7 CLI surface

```
--retained-size       Compute and surface retained sizes via dominator tree.
                      When set: summary class table sorts by retained-bytes
                      and adds a 'retained' column; a 'Largest retained
                      instances' section is appended; --paths-from-id annotates
                      each hop with retained size; --find-referrers annotates
                      each holder class with retained size. Default off.
                      Adds ~250-300 MiB working memory and ~1-3 s wall time
                      on a 200 MiB Android dump.
```

Mutually compatible with: `--preview-bytes`, `--target-glob`, `-l`, `--allocation-sites`, `--json`. Not compatible with itself across `--diff-from`/`--diff-to` (the diff modes still report shallow only — out of scope).

## 4. Output format

### summary

When `--retained-size` is set, the existing class table grows a column and re-sorts:

```
   instances     shallow      retained  class
        135    11.59KiB     1.45MiB    com.example.ResolvedDisplayItem
      18342     2.10MiB     1.97MiB    java.lang.String
       1024    32.00KiB   544.01KiB    char[]
```

A new section follows the existing "Largest array instances" block:

```
Largest retained instances object ids:
     1.45MiB object_id=4097812752 com.example.ResolvedDisplayItem
   544.01KiB object_id=1723142144 char[]
   234.01KiB object_id=2097446928 char[]
```

(Top-N controlled by the existing `-t` / `--top` flag.)

### --paths-from-id

Each hop line gains a trailing `[retained=<size>]` annotation:

```
[id=4097812752] com.example.ResolvedDisplayItem (retained=1.45MiB)
  → field slots: ResolvedDisplayFieldSlots (retained=1.42MiB)
    → artwork: ArtworkBundle (retained=1.40MiB)
      → poster char[] (retained=234.01KiB)
```

### --find-referrers

Holder rows gain a `retained` column:

```
=== Direct referrers (1-hop) ===
  holder.field                          ref count   class retained
  com.example.HomeStore.cache                   1         8.43MiB
  java.lang.String.value                        1         3.10MiB
```

## 5. Performance & memory

Working memory budget on a 200 MiB Android dump (~3M objects, ~15M edges):

| Component | MiB |
|-----------|-----|
| Pass1Index (already paid) | ~50 |
| `node_ids` (Vec<u64>, ~3M) | 24 |
| `node_class` + `node_shallow` (u32 each) | 24 |
| `edges_offsets` (u32, N+2) | 12 |
| `edges_targets` (u32, ~15M) | 60 |
| LT scratch (semi/parent/ancestor/label, 4 × u32 × N) | 48 |
| `idom` (u32, N+1) | 12 |
| `retained` (u64, N+1) | 24 |
| **Total over v0.9.0 baseline** | **~210 MiB** |

Wall-time budget on the 1.0.3 fixture (186 MiB Android, ~1.3M objects):

| Phase | Budget |
|-------|--------|
| Pass1 index | ~250 ms |
| Pass2 graph build | ~700 ms |
| Lengauer-Tarjan | ~400 ms |
| Retained DFS + aggregation | ~150 ms |
| **`summary --retained-size` end-to-end** | **~1.5 s** |

Hard cap on `top_n` (the per-instance hot list) is `--top` (default 20). Class retained map is bounded by class count (typically a few thousand entries — negligible).

## 6. Testing strategy

### Unit tests

- `src/dominators.rs`: known textbook examples (the canonical 13-node Lengauer-Tarjan paper graph; a trivial linear chain; a graph with unreachable nodes).
- `src/retained.rs`: synthetic graphs with hand-computed retained sums.
- `src/reference_graph.rs`: build a tiny synthetic Pass1Index + record stream; assert the CSR shape matches expected.

### Integration tests

- `summary --retained-size` on `JAVA_PROFILE_1.0.2.hprof`: assert the top class by retained ≠ top by shallow (proves the metric is doing work).
- `summary --retained-size` on `JAVA_PROFILE_1.0.3.hprof`: assert the largest `char[]` (object 1723142144) appears in "Largest retained instances".
- `--paths-from-id 1723142144 --retained-size`: assert `(retained=...)` annotations present on every hop.
- `--find-referrers java.lang.String --retained-size`: assert `class retained` column present.
- Regression: `summary` without the flag is byte-identical to v0.9.0.

### Property tests (deferred)

A `proptest`-style invariant — `retained[idom(v)] ≥ retained[v]` for all reachable v — would be valuable but is not on the v1.0.0 critical path.

## 7. Rollout

Seven sequential PRs onto `master`, single `v1.0.0` tag at the end.

| PR | Title | Lands |
|----|-------|-------|
| 1 | parser: `retain_object_references` mode | parser pass + tests, no user-visible effect |
| 2 | `src/reference_graph.rs` CSR builder | graph from Pass1Index + record stream + tests |
| 3 | `src/dominators.rs` + `src/retained.rs` | LT algorithm + retained DFS + class rollup, all unit-tested |
| 4 | summary `--retained-size` integration | flag wired; class table column + "Largest retained instances" block |
| 5 | `--paths-from-id` retained annotation | per-hop `(retained=...)` |
| 6 | `--find-referrers` retained annotation | per-holder-class `class retained` column |
| 7 | docs + version 1.0.0 + tag + release | README, USERGUIDE, SKILL bumps; `/plugin update` notes; binaries |

## 8. Risk notes

- **Memory pressure on huge Android dumps:** the +210 MiB budget is on top of an already memory-heavy two-pass referrer mode. If a fixture appears with >5M objects (~1 GB Android dumps), the LT scratch alone could push toward 1 GB. Mitigation: `--retained-size` is opt-in. If users hit OOM, document recommending 8 GB+ host RAM. Hard cap not in scope for v1.0.0.
- **GC root completeness:** `Pass1Index.gc_root_ids` already covers JVM standard roots + Android extensions. Any class statics holding objects are also added as edges from the super-root. If we miss a root tag, the retained tree under it will appear unreachable (retained = 0) — a silent under-count, not a panic. Mitigation: add an assertion that ≥99 % of nodes are reachable from the super-root on the canonical fixtures; bail with a warning if not.
- **Lengauer-Tarjan correctness:** standard textbook algorithm with well-known pitfalls (semi-dominator vs immediate-dominator distinction, path-compression invariants). Mitigation: ship with the canonical 13-node test from the paper as a unit test, plus the linear-chain and unreachable-node tests. If a non-trivial bug surfaces, fall back to the simpler iterative-dominators algorithm (O(N²) but trivially correct) for v1.0.0 and revisit LT in v1.1.
- **Object array of huge size:** ObjectArray dumps can have hundreds of thousands of element ids. The current parser materializes them as `Vec<u64>`. The reference-graph builder must avoid copying — push directly into `edges_targets`. Mitigation: builder writes into the CSR vectors during the stream callback, no intermediate Vec.
- **Schema stability:** `RenderedResult` JSON output gains two optional fields. Add `#[serde(skip_serializing_if = "Option::is_none")]` so consumers parsing v0.9.0 JSON still get backward-compatible output when the flag is unset.
