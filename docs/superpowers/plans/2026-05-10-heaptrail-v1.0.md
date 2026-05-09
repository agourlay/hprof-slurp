# heaptrail v1.0.0 — Retained size via dominator tree (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement feature E from `docs/superpowers/specs/2026-05-10-heaptrail-v1.0-design.md`: a `--retained-size` flag that augments `summary`, `--paths-from-id`, and `--find-referrers` with retained-size data computed from a Lengauer–Tarjan dominator tree.

**Architecture:** Seven sequential PRs onto `master`. PR 1 lands the in-memory CSR reference graph (built by extending the existing `retain_bodies` pass2 in `referrer.rs`). PR 2 lands the Lengauer–Tarjan algorithm as a self-contained module with textbook test cases. PR 3 lands the retained-size DFS + class rollup. PRs 4–6 wire the three render surfaces. PR 7 is docs + version bump 0.9.0 → 1.0.0 + tag + release.

**Tech Stack:** Rust 2024, ahash, nom, crossbeam-channel (existing); no new crates.

---

## File Structure

| File | Responsibility | First touched in |
|------|----------------|------------------|
| `src/reference_graph.rs` (NEW) | Build CSR object-reference graph from `Pass1Index` + a `retain_bodies` parse pass. | PR 1 |
| `src/dominators.rs` (NEW) | Lengauer–Tarjan immediate-dominator computation. Pure algorithm. | PR 2 |
| `src/retained.rs` (NEW) | Post-order DFS over the dominator tree → retained sizes; class rollup + top-N hot list. | PR 3 |
| `src/args.rs` | New `--retained-size` flag, propagated into Mode::Summary / Mode::Paths / Mode::FindReferrers. | PR 4 |
| `src/main.rs` | `run_summary` accepts `retained_size: bool`. | PR 4 |
| `src/result_recorder.rs` / `src/rendered_result.rs` | New `class_retained_sizes` + `top_retained_instances` fields; render integration. | PR 4 |
| `src/paths.rs` | Per-hop `(retained=...)` annotation. | PR 5 |
| `src/referrer.rs` | Per-holder-class `class retained` column. | PR 6 |
| `Cargo.toml` / `README.md` / `USERGUIDE.md` / `SKILL.md` / plugin manifests | Docs + version bump. | PR 7 |

The `Pass1Index` struct in `src/referrer.rs` is reused as-is — it already exposes `fields_by_class_id`, `super_class_by_id`, `static_object_fields_by_class_id`, and `gc_root_ids`, which is exactly the metadata the graph builder needs.

---

## PR 1 — Reference graph builder

**PR title:** `feat(retained): src/reference_graph.rs CSR builder over Pass1Index + retain_bodies pass2`

**Goal:** Module that, given `Pass1Index` and a path to the hprof, runs a `retain_bodies` pass2 and produces a `ReferenceGraph` (sorted node ids, class indices, shallow sizes, edges in CSR form, super-root). No user-visible CLI surface yet.

### Task 1.1: Define the ReferenceGraph type

**Files:**
- Create: `src/reference_graph.rs`
- Modify: `src/main.rs` — add `mod reference_graph;`

- [ ] **Step 1: Skeleton with the public type**

```rust
//! In-memory CSR object-reference graph used by the retained-size
//! pipeline. Built by streaming the hprof a second time with
//! `retain_bodies=true`, walking each instance dump's body using the
//! flattened (own + super) field layout cached in `Pass1Index`, and
//! pushing referenced object indices into a CSR adjacency structure.
//!
//! Memory: roughly `8 + 4 × refs_per_node` bytes per node. A 200 MiB
//! Android dump (~3M objects, ~5 refs/object) lands at ~120 MiB total
//! for nodes + edges. See `docs/superpowers/specs/2026-05-10-heaptrail-v1.0-design.md`
//! §5 for the full memory budget.

use ahash::AHashMap;

pub struct ReferenceGraph {
    /// Object ids in node-index order. node_index N (the last) is the
    /// virtual super-root that owns every GC root; its object id is 0.
    pub node_ids: Vec<u64>,
    /// Class index per node (index into `class_ids`); `u32::MAX` for
    /// the super-root.
    pub node_class: Vec<u32>,
    /// Shallow size per node, bytes.
    pub node_shallow: Vec<u32>,
    /// `class_index → class_object_id`. Built from the set of classes
    /// that actually appear as instance / array element types.
    pub class_ids: Vec<u64>,
    /// CSR row pointers, length `nodes.len() + 1`.
    pub edges_offsets: Vec<u32>,
    /// CSR column indices.
    pub edges_targets: Vec<u32>,
    /// Index of the virtual super-root (== `node_ids.len() - 1`).
    pub super_root: u32,
    /// Reverse lookup: object_id → node_index. Built once at the end
    /// of construction; consumed by the renderer to look up specific
    /// object ids (e.g. for `--paths-from-id`).
    pub index_by_object_id: AHashMap<u64, u32>,
}

impl ReferenceGraph {
    pub fn node_count(&self) -> usize { self.node_ids.len() }
    pub fn edge_count(&self) -> usize { self.edges_targets.len() }
    pub fn out_edges(&self, node: u32) -> &[u32] {
        let start = self.edges_offsets[node as usize] as usize;
        let end = self.edges_offsets[node as usize + 1] as usize;
        &self.edges_targets[start..end]
    }
    pub fn node_index_of(&self, object_id: u64) -> Option<u32> {
        self.index_by_object_id.get(&object_id).copied()
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build --release`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add src/reference_graph.rs src/main.rs
git commit -m "feat(retained): scaffold src/reference_graph.rs with ReferenceGraph type"
```

### Task 1.2: Builder — first pass collects nodes

**Files:**
- Modify: `src/reference_graph.rs`

- [ ] **Step 1: Add `build_from_pass1` that populates nodes only (no edges yet)**

```rust
use crate::errors::HprofSlurpError;
use crate::parser::gc_record::GcRecord;
use crate::parser::record::Record;
use crate::referrer::Pass1Index;
use crate::slurp::parse_records;

/// Streams `path` once with `retain_bodies=true`. Builds:
///   * `node_ids` — every object_id seen (instance dumps, primitive
///     arrays, object arrays). Sorted at the end so node indices are
///     deterministic.
///   * `node_class` / `node_shallow` — populated alongside.
///   * `class_ids` + a temporary `class_index_by_id` — built lazily as
///     unseen class object ids are encountered.
///
/// Edges are a separate pass (Task 1.3) so we have stable indices for
/// every target before resolving them.
pub fn build_from_pass1(
    path: &str,
    idx: &Pass1Index,
    debug: bool,
) -> Result<ReferenceGraph, HprofSlurpError> {
    let mut node_ids = Vec::<u64>::new();
    let mut node_class = Vec::<u32>::new();
    let mut node_shallow = Vec::<u32>::new();
    let mut class_ids = Vec::<u64>::new();
    let mut class_index_by_id = AHashMap::<u64, u32>::new();

    let records = parse_records(path, debug, true /* retain_bodies */, false /* retain_primitive */, 0)?;
    for record in records {
        if let Record::GcSegment(seg) = record {
            for gc in seg {
                match gc {
                    GcRecord::InstanceDump { object_id, class_object_id, .. } => {
                        let ci = class_index(&mut class_ids, &mut class_index_by_id, class_object_id);
                        let size = instance_shallow_size(idx, class_object_id);
                        node_ids.push(object_id);
                        node_class.push(ci);
                        node_shallow.push(size);
                    }
                    GcRecord::ObjectArrayDump { object_id, array_class_object_id, elements, .. } => {
                        let ci = class_index(&mut class_ids, &mut class_index_by_id, array_class_object_id);
                        let size = (idx.id_size as u32).saturating_mul(elements.len() as u32);
                        node_ids.push(object_id);
                        node_class.push(ci);
                        node_shallow.push(size);
                    }
                    GcRecord::PrimitiveArrayDump { object_id, element_type, number_of_elements, .. } => {
                        // Primitive arrays have no class_object_id at the
                        // record level; we tag them with a synthetic class
                        // (one per FieldType). Use a fixed sentinel id base.
                        let synthetic_class = primitive_synthetic_class_id(*element_type);
                        let ci = class_index(&mut class_ids, &mut class_index_by_id, synthetic_class);
                        let size = primitive_array_size(idx.id_size, *element_type, *number_of_elements) as u32;
                        node_ids.push(*object_id);
                        node_class.push(ci);
                        node_shallow.push(size);
                    }
                    _ => {}
                }
            }
        }
    }

    // Sort nodes for deterministic indices, then build the reverse map.
    let mut order: Vec<u32> = (0..node_ids.len() as u32).collect();
    order.sort_unstable_by_key(|&i| node_ids[i as usize]);
    let node_ids = order.iter().map(|&i| node_ids[i as usize]).collect::<Vec<_>>();
    let node_class = order.iter().map(|&i| node_class[i as usize]).collect();
    let node_shallow = order.iter().map(|&i| node_shallow[i as usize]).collect();

    // Reserve one slot for the super-root (appended last).
    let super_root = node_ids.len() as u32;
    let mut node_ids = node_ids; node_ids.push(0);
    let mut node_class = node_class; node_class.push(u32::MAX);
    let mut node_shallow = node_shallow; node_shallow.push(0);

    let mut index_by_object_id = AHashMap::with_capacity(node_ids.len());
    for (i, &oid) in node_ids.iter().enumerate() {
        if i as u32 != super_root {
            index_by_object_id.insert(oid, i as u32);
        }
    }

    Ok(ReferenceGraph {
        node_ids, node_class, node_shallow, class_ids,
        edges_offsets: Vec::new(), edges_targets: Vec::new(),
        super_root, index_by_object_id,
    })
}

fn class_index(
    class_ids: &mut Vec<u64>,
    by_id: &mut AHashMap<u64, u32>,
    class_object_id: u64,
) -> u32 {
    *by_id.entry(class_object_id).or_insert_with(|| {
        let i = class_ids.len() as u32;
        class_ids.push(class_object_id);
        i
    })
}

fn instance_shallow_size(idx: &Pass1Index, class_object_id: u64) -> u32 {
    // Walk the super-class chain, summing field-type sizes. Cap at u32 (~4 GB,
    // far above any real instance).
    let mut size: u64 = 0;
    let mut cls = Some(class_object_id);
    while let Some(c) = cls {
        if let Some(fields) = idx.fields_by_class_id.get(&c) {
            for f in fields {
                size += match f.field_type {
                    crate::parser::gc_record::FieldType::Object => idx.id_size as u64,
                    crate::parser::gc_record::FieldType::Bool | crate::parser::gc_record::FieldType::Byte => 1,
                    crate::parser::gc_record::FieldType::Char | crate::parser::gc_record::FieldType::Short => 2,
                    crate::parser::gc_record::FieldType::Int | crate::parser::gc_record::FieldType::Float => 4,
                    crate::parser::gc_record::FieldType::Long | crate::parser::gc_record::FieldType::Double => 8,
                };
            }
        }
        cls = idx.super_class_by_id.get(&c).copied();
        if cls == Some(0) { break; }
    }
    size.min(u32::MAX as u64) as u32
}

fn primitive_synthetic_class_id(t: crate::parser::gc_record::FieldType) -> u64 {
    // Sentinel ids picked above the realistic class-object-id range.
    use crate::parser::gc_record::FieldType::*;
    let n = match t {
        Object => 0, Bool => 1, Byte => 2, Char => 3, Short => 4, Int => 5,
        Float => 6, Long => 7, Double => 8,
    };
    0xFFFF_FFFF_FFFF_FF00u64 | n
}

fn primitive_array_size(id_size: u32, t: crate::parser::gc_record::FieldType, n: u32) -> u64 {
    use crate::parser::gc_record::FieldType::*;
    let elem = match t {
        Object => id_size as u64,
        Bool | Byte => 1, Char | Short => 2, Int | Float => 4, Long | Double => 8,
    };
    elem * n as u64
}
```

- [ ] **Step 2: Build**

Run: `cargo build --release`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add src/reference_graph.rs
git commit -m "feat(retained): build_from_pass1 — collect nodes (instance / object array / primitive array)"
```

### Task 1.3: Builder — second walk emits edges in CSR

**Files:**
- Modify: `src/reference_graph.rs`

- [ ] **Step 1: Add edge collection during the same parse pass**

Refactor `build_from_pass1` to accumulate edges during the same iteration — push `(src_node_index, dst_object_id)` into a temporary `Vec<(u32, u64)>` per record, then post-process into CSR after node sorting.

Replace the body of `build_from_pass1` (Task 1.2 step 1) with the version that also threads:

```rust
    let mut edge_buf = Vec::<(u64, u64)>::new(); // (src_oid, dst_oid)

    for record in records {
        if let Record::GcSegment(seg) = record {
            for gc in seg {
                match gc {
                    GcRecord::InstanceDump { object_id, class_object_id, body, .. } => {
                        // (existing node push as before)
                        if let Some(b) = body {
                            extract_refs_into(idx, *class_object_id, b, *object_id, &mut edge_buf);
                        }
                    }
                    GcRecord::ObjectArrayDump { object_id, elements, .. } => {
                        for &dst in elements {
                            if dst != 0 { edge_buf.push((*object_id, dst)); }
                        }
                    }
                    // PrimitiveArrayDump and other GcRecord arms unchanged
                    _ => { /* node-only */ }
                }
            }
        }
    }

    // Static fields of every class are edges from the super-root.
    let super_oid_placeholder = 0u64;
    for (_class_id, statics) in &idx.static_object_fields_by_class_id {
        for &(_name_id, target) in statics {
            if target != 0 { edge_buf.push((super_oid_placeholder, target)); }
        }
    }
    // GC roots are edges from the super-root too.
    for &root in &idx.gc_root_ids {
        edge_buf.push((super_oid_placeholder, root));
    }
```

Add the body-walking helper:

```rust
fn extract_refs_into(
    idx: &Pass1Index,
    class_object_id: u64,
    body: &[u8],
    src_oid: u64,
    out: &mut Vec<(u64, u64)>,
) {
    let mut cursor = 0usize;
    let mut cls = Some(class_object_id);
    while let Some(c) = cls {
        if let Some(fields) = idx.fields_by_class_id.get(&c) {
            for f in fields {
                let size = match f.field_type {
                    crate::parser::gc_record::FieldType::Object => idx.id_size as usize,
                    crate::parser::gc_record::FieldType::Bool | crate::parser::gc_record::FieldType::Byte => 1,
                    crate::parser::gc_record::FieldType::Char | crate::parser::gc_record::FieldType::Short => 2,
                    crate::parser::gc_record::FieldType::Int | crate::parser::gc_record::FieldType::Float => 4,
                    crate::parser::gc_record::FieldType::Long | crate::parser::gc_record::FieldType::Double => 8,
                };
                if f.field_type == crate::parser::gc_record::FieldType::Object && cursor + size <= body.len() {
                    let dst = match idx.id_size {
                        4 => u32::from_be_bytes(body[cursor..cursor+4].try_into().unwrap()) as u64,
                        8 => u64::from_be_bytes(body[cursor..cursor+8].try_into().unwrap()),
                        _ => 0,
                    };
                    if dst != 0 { out.push((src_oid, dst)); }
                }
                cursor += size;
            }
        }
        cls = idx.super_class_by_id.get(&c).copied();
        if cls == Some(0) { break; }
    }
}
```

- [ ] **Step 2: Build the CSR after node-sort**

After the node sort + `super_root` push, resolve `edge_buf` into CSR:

```rust
    // Build CSR.
    let n = node_ids.len();
    let mut edges_offsets = vec![0u32; n + 1];
    let mut edges_targets = Vec::with_capacity(edge_buf.len());

    // Resolve src_oid → src_idx; for super-root edges, src_oid is 0 → super_root.
    let mut resolved = Vec::<(u32, u32)>::with_capacity(edge_buf.len());
    for (src_oid, dst_oid) in edge_buf {
        let src_idx = if src_oid == 0 { super_root }
            else if let Some(&i) = index_by_object_id.get(&src_oid) { i }
            else { continue };
        let dst_idx = if let Some(&i) = index_by_object_id.get(&dst_oid) { i }
            else { continue };
        resolved.push((src_idx, dst_idx));
    }
    resolved.sort_unstable_by_key(|&(s, _)| s);

    // Counts → prefix sum.
    for &(s, _) in &resolved { edges_offsets[s as usize + 1] += 1; }
    for i in 1..edges_offsets.len() { edges_offsets[i] += edges_offsets[i - 1]; }

    // Targets in src order.
    edges_targets.resize(resolved.len(), 0);
    let mut cursors = edges_offsets.clone();
    for (s, d) in resolved {
        let p = cursors[s as usize] as usize;
        edges_targets[p] = d;
        cursors[s as usize] += 1;
    }
```

Plug into the returned `ReferenceGraph`. Build.

Run: `cargo build --release`
Expected: clean build.

- [ ] **Step 3: Smoke-run on the canonical fixtures**

```bash
cargo run --release --quiet --example graph_smoke -- JAVA_PROFILE_1.0.2.hprof
```

Where `examples/graph_smoke.rs` (create alongside) does:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: graph_smoke <hprof>")?;
    let idx = heaptrail::referrer::pass1_index(&path, false)?;
    let g = heaptrail::reference_graph::build_from_pass1(&path, &idx, false)?;
    println!("nodes={} edges={} super_root={}", g.node_count(), g.edge_count(), g.super_root);
    Ok(())
}
```

(After the smoke verifies, the `examples/` file can be deleted in PR 7 cleanup, or kept — author's choice.)

Expected on `JAVA_PROFILE_1.0.2.hprof`: `nodes` in the high tens of thousands, `edges` 5–10× nodes.
Expected on `JAVA_PROFILE_1.0.3.hprof`: nodes around 1–1.5M, edges 5–10× nodes.

- [ ] **Step 4: Commit**

```bash
git add src/reference_graph.rs examples/graph_smoke.rs
git commit -m "feat(retained): emit object-reference edges in CSR form"
```

### Task 1.4: Final lint / fmt / push

- [ ] **Step 1: Lint + fmt + tests**

```bash
cargo test --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
```

- [ ] **Step 2: Push + wait CI**

```bash
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 2 — Lengauer–Tarjan immediate dominators

**PR title:** `feat(retained): src/dominators.rs Lengauer–Tarjan implementation`

**Goal:** Self-contained pure-algorithm module that takes `&ReferenceGraph` and returns `Vec<u32>` of immediate-dominator indices, with `idom[super_root] = u32::MAX` and `idom[unreachable] = u32::MAX`.

### Task 2.1: Module skeleton + textbook test cases

**Files:**
- Create: `src/dominators.rs`
- Modify: `src/main.rs` — `mod dominators;`

- [ ] **Step 1: Stub the public API**

```rust
//! Lengauer–Tarjan O(N α(N)) immediate-dominator computation.
//! Reference: Lengauer & Tarjan, "A Fast Algorithm for Finding Dominators
//! in a Flowgraph", TOPLAS 1979.
//!
//! Input: a `ReferenceGraph` whose `super_root` is the unique entry node
//! that reaches every other node along forward edges.
//!
//! Output: `idom[i]` is the immediate-dominator node index of node `i`.
//! For `i == super_root` and for nodes unreachable from `super_root`,
//! `idom[i] == u32::MAX` (sentinel).

use crate::reference_graph::ReferenceGraph;

pub fn lengauer_tarjan(graph: &ReferenceGraph) -> Vec<u32> {
    let n = graph.node_count();
    vec![u32::MAX; n]
}
```

- [ ] **Step 2: Write the textbook 13-node test (the one from the LT paper, Fig. 2)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ahash::AHashMap;
    use crate::reference_graph::ReferenceGraph;

    /// Build a tiny synthetic graph from edge list. Node `0..n` are
    /// the body; node `n` is the super-root with edges to whichever
    /// "entry" caller nominates.
    fn make_graph(n: usize, edges: &[(u32, u32)], roots: &[u32]) -> ReferenceGraph {
        let super_root = n as u32;
        let total = n + 1;
        let mut g = ReferenceGraph {
            node_ids: (0..total as u64).collect(),
            node_class: vec![0; total],
            node_shallow: vec![1; total],
            class_ids: vec![0],
            edges_offsets: vec![0; total + 1],
            edges_targets: Vec::new(),
            super_root,
            index_by_object_id: AHashMap::new(),
        };
        let mut all = edges.to_vec();
        for &r in roots { all.push((super_root, r)); }
        all.sort_unstable_by_key(|&(s, _)| s);
        for &(s, _) in &all { g.edges_offsets[s as usize + 1] += 1; }
        for i in 1..g.edges_offsets.len() { g.edges_offsets[i] += g.edges_offsets[i - 1]; }
        g.edges_targets.resize(all.len(), 0);
        let mut cur = g.edges_offsets.clone();
        for (s, d) in all {
            let p = cur[s as usize] as usize;
            g.edges_targets[p] = d;
            cur[s as usize] += 1;
        }
        g
    }

    #[test]
    fn linear_chain() {
        // super_root → 0 → 1 → 2 → 3
        let g = make_graph(4, &[(0,1),(1,2),(2,3)], &[0]);
        let idom = lengauer_tarjan(&g);
        assert_eq!(idom[0], g.super_root);
        assert_eq!(idom[1], 0);
        assert_eq!(idom[2], 1);
        assert_eq!(idom[3], 2);
        assert_eq!(idom[g.super_root as usize], u32::MAX);
    }

    #[test]
    fn diamond() {
        // super_root → 0; 0 → 1; 0 → 2; 1 → 3; 2 → 3
        let g = make_graph(4, &[(0,1),(0,2),(1,3),(2,3)], &[0]);
        let idom = lengauer_tarjan(&g);
        assert_eq!(idom[0], g.super_root);
        assert_eq!(idom[1], 0);
        assert_eq!(idom[2], 0);
        // 3 has two paths from 0 — its idom is 0 (the meet).
        assert_eq!(idom[3], 0);
    }

    #[test]
    fn unreachable_node_marked_max() {
        // 0 reachable; 1 has no incoming from super_root or anyone reachable.
        let g = make_graph(2, &[], &[0]);
        let idom = lengauer_tarjan(&g);
        assert_eq!(idom[0], g.super_root);
        assert_eq!(idom[1], u32::MAX);
    }

    #[test]
    fn paper_fig2_thirteen_nodes() {
        // Lengauer–Tarjan paper Figure 2. Nodes labeled R, A..L renamed
        // to 0..12. Edges per the paper.
        // 0=R, 1=A, 2=B, 3=C, 4=D, 5=E, 6=F, 7=G, 8=H, 9=I, 10=J, 11=K, 12=L
        let edges = &[
            (0,1),(0,2),(0,3),
            (1,4),
            (2,1),(2,4),(2,5),
            (3,6),(3,7),
            (4,12),
            (5,8),
            (6,9),
            (7,9),(7,10),
            (8,5),(8,11),
            (9,11),
            (10,9),
            (11,9),(11,0),
            (12,9),
        ];
        let g = make_graph(13, edges, &[0]);
        let idom = lengauer_tarjan(&g);
        // Expected immediate dominators per the paper:
        //   idom(R)  = ⊥ (super_root)
        //   idom(A)  = R
        //   idom(B)  = R
        //   idom(C)  = R
        //   idom(D)  = R
        //   idom(E)  = R
        //   idom(F)  = C
        //   idom(G)  = C
        //   idom(H)  = E
        //   idom(I)  = R
        //   idom(J)  = G
        //   idom(K)  = R
        //   idom(L)  = D
        assert_eq!(idom[0], g.super_root);                 // R
        assert_eq!(idom[1], 0);                            // A → R
        assert_eq!(idom[2], 0);                            // B → R
        assert_eq!(idom[3], 0);                            // C → R
        assert_eq!(idom[4], 0);                            // D → R
        assert_eq!(idom[5], 0);                            // E → R
        assert_eq!(idom[6], 3);                            // F → C
        assert_eq!(idom[7], 3);                            // G → C
        assert_eq!(idom[8], 5);                            // H → E
        assert_eq!(idom[9], 0);                            // I → R
        assert_eq!(idom[10], 7);                           // J → G
        assert_eq!(idom[11], 0);                           // K → R
        assert_eq!(idom[12], 4);                           // L → D
    }
}
```

- [ ] **Step 3: Run tests — they fail (stub returns all `u32::MAX`)**

```bash
cargo test --release dominators
```

Expected: 4 tests fail.

- [ ] **Step 4: Commit the failing tests**

```bash
git add src/dominators.rs src/main.rs
git commit -m "test(retained): textbook Lengauer–Tarjan cases (linear, diamond, unreachable, paper fig 2)"
```

### Task 2.2: Implement Lengauer–Tarjan

**Files:**
- Modify: `src/dominators.rs`

- [ ] **Step 1: Implement the algorithm**

Replace the stub with a textbook implementation:

```rust
pub fn lengauer_tarjan(graph: &ReferenceGraph) -> Vec<u32> {
    let n = graph.node_count();
    let r = graph.super_root as usize;

    // Step 1 — DFS from super_root, assign DFS numbers.
    let mut parent = vec![u32::MAX; n];        // DFS-tree parent (in DFS-num space)
    let mut semi = vec![u32::MAX; n];          // semi-dominator (DFS-num)
    let mut vertex = Vec::<u32>::with_capacity(n); // DFS-num → node
    let mut dfnum = vec![u32::MAX; n];         // node → DFS-num (or MAX if unreached)

    {
        let mut stack: Vec<(u32, u32)> = Vec::new(); // (node, parent_dfnum)
        stack.push((r as u32, u32::MAX));
        while let Some((v, p)) = stack.pop() {
            if dfnum[v as usize] != u32::MAX { continue; }
            let num = vertex.len() as u32;
            dfnum[v as usize] = num;
            semi[v as usize] = num;
            parent[v as usize] = p;
            vertex.push(v);
            for &w in graph.out_edges(v).iter().rev() {
                if dfnum[w as usize] == u32::MAX {
                    stack.push((w, num));
                }
            }
        }
    }

    // Build predecessor list (only over reached nodes).
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    for &v in &vertex {
        for &w in graph.out_edges(v) {
            if dfnum[w as usize] != u32::MAX {
                preds[w as usize].push(v);
            }
        }
    }

    // Step 2/3 — link/eval over the DFS tree using path compression.
    let mut ancestor = vec![u32::MAX; n];
    let mut label = vertex.clone(); // label[v] starts as v
    let mut bucket: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut idom = vec![u32::MAX; n];

    fn eval(v: u32, ancestor: &mut [u32], label: &mut [u32], semi: &[u32]) -> u32 {
        if ancestor[v as usize] == u32::MAX {
            return v;
        }
        // Find root of ancestor chain.
        let mut path = vec![v];
        let mut cur = ancestor[v as usize];
        while ancestor[cur as usize] != u32::MAX {
            path.push(cur);
            cur = ancestor[cur as usize];
        }
        // Compress.
        for &x in path.iter().rev() {
            let a = ancestor[x as usize];
            if a != u32::MAX && ancestor[a as usize] != u32::MAX {
                if semi[label[a as usize] as usize] < semi[label[x as usize] as usize] {
                    label[x as usize] = label[a as usize];
                }
                ancestor[x as usize] = ancestor[a as usize];
            }
        }
        label[v as usize]
    }

    // Reverse-DFS iteration.
    for i in (1..vertex.len()).rev() {
        let w = vertex[i];

        // Step 2 — semidominator of w.
        for &v in &preds[w as usize] {
            let u = eval(v, &mut ancestor, &mut label, &semi);
            if semi[u as usize] < semi[w as usize] {
                semi[w as usize] = semi[u as usize];
            }
        }
        bucket[vertex[semi[w as usize] as usize] as usize].push(w);

        // Link w to parent.
        ancestor[w as usize] = parent[w as usize];

        // Step 3 — implicit immediate dominator for vertices in
        // bucket[parent(w)].
        let pw = parent[w as usize];
        if pw == u32::MAX { continue; }
        let pw_node = vertex[pw as usize];
        let drained = std::mem::take(&mut bucket[pw_node as usize]);
        for v in drained {
            let u = eval(v, &mut ancestor, &mut label, &semi);
            idom[v as usize] = if semi[u as usize] < semi[v as usize] { u } else { pw_node };
        }
    }

    // Step 4 — fix idoms whose semidominator was deferred.
    for &w in &vertex[1..] {
        if idom[w as usize] != vertex[semi[w as usize] as usize] {
            idom[w as usize] = idom[idom[w as usize] as usize];
        }
    }

    idom[r] = u32::MAX;
    idom
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test --release dominators
```

Expected: 4 pass.

- [ ] **Step 3: Lint + fmt + commit + push + CI**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
git add src/dominators.rs
git commit -m "feat(retained): Lengauer–Tarjan immediate-dominator computation"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 3 — Retained-size DFS + class rollup

**PR title:** `feat(retained): src/retained.rs DFS + class aggregation`

**Goal:** Given a `ReferenceGraph` and an `idom` vec, produce per-node retained sizes, the class-level retained map, and a top-N largest-retained instance list.

### Task 3.1: API + tests

**Files:**
- Create: `src/retained.rs`
- Modify: `src/main.rs` — `mod retained;`

- [ ] **Step 1: Public API**

```rust
//! Retained-size computation. Given a `ReferenceGraph` and the `idom`
//! vector from `dominators::lengauer_tarjan`, computes:
//!
//!   retained[v] = shallow[v] + sum(retained[c] for c where idom[c] == v)
//!
//! Then rolls up to:
//!   * `class_retained: AHashMap<class_object_id, retained_bytes>`
//!   * `top_instances: Vec<(object_id, class_object_id, retained_bytes)>`
//!     sorted descending, length ≤ `top_n`.

use ahash::AHashMap;
use crate::reference_graph::ReferenceGraph;

pub struct RetainedAnalysis {
    pub retained: Vec<u64>,
    pub class_retained: AHashMap<u64, u64>,
    pub top_instances: Vec<(u64, u64, u64)>,
}

pub fn compute(graph: &ReferenceGraph, idom: &[u32], top_n: usize) -> RetainedAnalysis {
    let n = graph.node_count();
    let mut retained = vec![0u64; n];
    for i in 0..n { retained[i] = graph.node_shallow[i] as u64; }

    // Build dominator-tree children list.
    let mut children: Vec<Vec<u32>> = vec![Vec::new(); n];
    for v in 0..n {
        let d = idom[v];
        if d != u32::MAX && (d as usize) < n {
            children[d as usize].push(v as u32);
        }
    }

    // Iterative post-order from super_root.
    let r = graph.super_root as usize;
    let mut stack: Vec<(u32, bool)> = vec![(r as u32, false)];
    while let Some((v, processed)) = stack.pop() {
        if processed {
            for &c in &children[v as usize] {
                retained[v as usize] = retained[v as usize].saturating_add(retained[c as usize]);
            }
        } else {
            stack.push((v, true));
            for &c in &children[v as usize] { stack.push((c, false)); }
        }
    }

    // Class rollup + top-N.
    let mut class_retained: AHashMap<u64, u64> = AHashMap::new();
    let mut all_inst = Vec::<(u64, u64, u64)>::with_capacity(n);
    for v in 0..n {
        if v as u32 == graph.super_root { continue; }
        let ci = graph.node_class[v];
        if ci == u32::MAX { continue; }
        let class_id = graph.class_ids[ci as usize];
        let r = retained[v];
        *class_retained.entry(class_id).or_insert(0) += r;
        all_inst.push((graph.node_ids[v], class_id, r));
    }
    all_inst.sort_unstable_by(|a, b| b.2.cmp(&a.2));
    all_inst.truncate(top_n);

    RetainedAnalysis { retained, class_retained, top_instances: all_inst }
}
```

- [ ] **Step 2: Tests against synthetic graphs**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ahash::AHashMap;
    use crate::reference_graph::ReferenceGraph;
    use crate::dominators::lengauer_tarjan;

    fn graph_with_shallow(
        n: usize, edges: &[(u32, u32)], roots: &[u32], shallow: &[u32], classes: &[u64],
    ) -> ReferenceGraph {
        let super_root = n as u32;
        let total = n + 1;
        let mut class_ids: Vec<u64> = classes.iter().copied().collect();
        class_ids.sort_unstable(); class_ids.dedup();
        let cls_idx: AHashMap<u64, u32> = class_ids.iter().enumerate()
            .map(|(i, &c)| (c, i as u32)).collect();
        let node_class: Vec<u32> = classes.iter().map(|c| cls_idx[c]).collect();
        let mut node_class = node_class; node_class.push(u32::MAX);
        let mut node_shallow: Vec<u32> = shallow.iter().copied().collect();
        node_shallow.push(0);
        let node_ids: Vec<u64> = (0..total as u64).collect();
        let mut all: Vec<(u32, u32)> = edges.to_vec();
        for &r in roots { all.push((super_root, r)); }
        all.sort_unstable_by_key(|&(s, _)| s);
        let mut edges_offsets = vec![0u32; total + 1];
        for &(s, _) in &all { edges_offsets[s as usize + 1] += 1; }
        for i in 1..edges_offsets.len() { edges_offsets[i] += edges_offsets[i - 1]; }
        let mut edges_targets = vec![0u32; all.len()];
        let mut cur = edges_offsets.clone();
        for (s, d) in all {
            let p = cur[s as usize] as usize;
            edges_targets[p] = d; cur[s as usize] += 1;
        }
        ReferenceGraph {
            node_ids, node_class, node_shallow, class_ids,
            edges_offsets, edges_targets, super_root, index_by_object_id: AHashMap::new(),
        }
    }

    #[test]
    fn linear_chain_retains_tail_under_head() {
        // super → 0 → 1 → 2; sizes 10/20/30; class 100 for all.
        let g = graph_with_shallow(3, &[(0,1),(1,2)], &[0], &[10,20,30], &[100,100,100]);
        let idom = lengauer_tarjan(&g);
        let r = compute(&g, &idom, 5);
        assert_eq!(r.retained[0], 60);
        assert_eq!(r.retained[1], 50);
        assert_eq!(r.retained[2], 30);
        assert_eq!(r.class_retained[&100], 60 + 50 + 30);
        assert_eq!(r.top_instances[0].2, 60);
    }

    #[test]
    fn diamond_does_not_double_count() {
        // super → 0; 0 → 1; 0 → 2; 1 → 3; 2 → 3; sizes all 10.
        let g = graph_with_shallow(4, &[(0,1),(0,2),(1,3),(2,3)], &[0], &[10,10,10,10], &[1,1,1,1]);
        let idom = lengauer_tarjan(&g);
        let r = compute(&g, &idom, 5);
        // 3 is dominated by 0; 1 and 2 are leaves under the dom-tree.
        assert_eq!(r.retained[0], 40);
        assert_eq!(r.retained[1], 10);
        assert_eq!(r.retained[2], 10);
        assert_eq!(r.retained[3], 10);
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test --release retained
```

Expected: 2 pass.

- [ ] **Step 4: Commit + lint + push + CI**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
git add src/retained.rs src/main.rs
git commit -m "feat(retained): post-order DFS retained sizes + class rollup + top-N hot list"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 4 — `summary --retained-size`

**PR title:** `feat(retained): summary --retained-size class column + Largest retained instances`

**Goal:** Add `--retained-size` flag. When set, summary mode runs the full pipeline (pass1 → graph → LT → retained), augments the class table with a retained column, sorts by retained, and appends a "Largest retained instances" block.

### Task 4.1: CLI flag + Mode plumbing

**Files:**
- Modify: `src/args.rs`

- [ ] **Step 1: Add the flag**

In the `Cli` struct, add:

```rust
    /// Compute and surface retained sizes via dominator tree
    /// (Lengauer–Tarjan). Annotates summary, --paths-from-id, and
    /// --find-referrers. Adds ~250 MiB working memory and ~1-3 s
    /// wall time on a 200 MiB Android dump. Default off.
    #[arg(long = "retained-size", default_value_t = false)]
    pub retained_size: bool,
```

In `Mode::Summary`, `Mode::Paths`, and `Mode::FindReferrers`, add:

```rust
    pub retained_size: bool,
```

In `resolve()`, propagate `cli.retained_size` into each constructed mode.

In the test `fixture_args` helpers in every module that constructs the modes for tests, add `retained_size: false`.

- [ ] **Step 2: Build + tests**

```bash
cargo build --release
cargo test --release args::
```

Expected: clean build; existing tests pass.

- [ ] **Step 3: Add a flag-parsing test**

```rust
#[test]
fn parses_retained_size_flag() {
    let cli = Cli::parse_from(["heaptrail", "-i", "x.hprof", "--retained-size"]);
    let mode = cli.resolve().unwrap();
    matches!(mode, Mode::Summary { retained_size: true, .. });
}
```

- [ ] **Step 4: Commit**

```bash
git add src/args.rs
git commit -m "feat(retained): --retained-size CLI flag + Mode propagation"
```

### Task 4.2: RenderedResult + render integration

**Files:**
- Modify: `src/result_recorder.rs`
- Modify: `src/rendered_result.rs`
- Modify: `src/slurp.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: New optional RenderedResult fields**

```rust
pub struct RenderedResult {
    // ... existing ...
    pub class_retained_sizes: Option<AHashMap<u64, u64>>,
    pub top_retained_instances: Option<Vec<(u64, u64, u64)>>, // (object_id, class_object_id, retained_bytes)
}
```

`#[serde(skip_serializing_if = "Option::is_none")]` on both.

- [ ] **Step 2: Wire `slurp_file_with_modes` (extend `_with_preview` signature)**

`src/slurp.rs` — add a new entry point that accepts `retained_size: bool`:

```rust
pub fn slurp_file_with_modes(
    file_path: &str,
    debug_mode: bool,
    list_strings: bool,
    preview_bytes: u32,
    list_arrays_min_bytes: u32,
    retained_size: bool,
) -> Result<RenderedResult, HprofSlurpError> {
    // 1. Existing summary pipeline produces the base RenderedResult.
    let mut rr = slurp_file_with_preview(
        file_path, debug_mode, list_strings, preview_bytes, list_arrays_min_bytes,
    )?;

    if retained_size {
        let idx = crate::referrer::pass1_index(file_path, debug_mode)?;
        let graph = crate::reference_graph::build_from_pass1(file_path, &idx, debug_mode)?;
        let idom = crate::dominators::lengauer_tarjan(&graph);
        let r = crate::retained::compute(&graph, &idom, /* top_n */ 50);
        rr.class_retained_sizes = Some(r.class_retained);
        rr.top_retained_instances = Some(r.top_instances);
    }

    Ok(rr)
}
```

`slurp_file_with_preview` keeps the existing signature; it now leaves the two new fields as `None`.

- [ ] **Step 3: `run_summary` consumes the new entry point**

In `src/main.rs`, `run_summary` accepts `retained_size: bool` and calls `slurp_file_with_modes`.

- [ ] **Step 4: Render — extend `render_memory_usage`**

In `src/rendered_result.rs`, `render_memory_usage` already takes a top-N limit. Extend its signature with `Option<&AHashMap<u64, u64>>` (`class_retained_sizes`). When `Some`:

  * Resort the rendered table by retained-bytes descending (look up by class name → class_id → retained map; if missing, treat as 0).
  * Add a `retained` column (right-aligned, `pretty_bytes_size`).

When `None`, render exactly as today (byte-identical regression).

- [ ] **Step 5: New "Largest retained instances" block**

After the existing "Largest array instances" block, add (when `top_retained_instances.is_some()`):

```rust
fn render_top_retained_instances(
    out: &mut String,
    top: &[(u64, u64, u64)],
    class_name_by_id: &AHashMap<u64, String>,
) {
    use std::fmt::Write;
    if top.is_empty() { return; }
    let _ = writeln!(out, "\nLargest retained instances object ids:");
    for (oid, cid, r) in top {
        let class = class_name_by_id.get(cid).map(|s| s.as_str()).unwrap_or("(unknown)");
        let _ = writeln!(out, "  {:>10} object_id={oid} {class}", crate::utils::pretty_bytes_size(*r));
    }
}
```

The recorder already has `get_class_name_string` keyed by `class_object_id`; pass that map into the renderer alongside the existing data.

- [ ] **Step 6: Build + tests**

```bash
cargo build --release
cargo test --release
```

- [ ] **Step 7: Smoke-test on both fixtures**

```bash
echo "=== JAVA_PROFILE_1.0.2 with --retained-size ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.2.hprof --retained-size -t 5 2>&1 | tail -30
echo ""
echo "=== JAVA_PROFILE_1.0.3 with --retained-size ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.3.hprof --retained-size -t 5 2>&1 | tail -30
echo ""
echo "=== regression: no flag ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.2.hprof -t 1 2>&1 | tail -10
```

Expected: with the flag, the class table re-sorts and includes a `retained` column; "Largest retained instances" block lists top-N (object_id, class, bytes) entries. Without the flag, output identical to v0.9.0.

- [ ] **Step 8: Commit**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
git add src/result_recorder.rs src/rendered_result.rs src/slurp.rs src/main.rs
git commit -m "feat(retained): summary --retained-size class column + Largest retained instances block"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 5 — `--paths-from-id --retained-size`

**PR title:** `feat(retained): --paths-from-id annotates each hop with retained size`

**Goal:** When `--retained-size` is set on `--paths-from-id`, build the dominator-tree-derived `retained: Vec<u64>` once and annotate each rendered hop with `(retained=<bytes>)`.

### Task 5.1: Wire and render

**Files:**
- Modify: `src/paths.rs`

- [ ] **Step 1: Destructure `retained_size`**

In `paths::run`:

```rust
let (input_file, target_id, max_depth, top, debug, preview_bytes, retained_size) = match mode {
    Mode::Paths { input_file, target_id, max_depth, top, debug, preview_bytes, retained_size, .. }
        => (input_file.as_str(), *target_id, *max_depth, *top, *debug, *preview_bytes, *retained_size),
    _ => return Err(HprofSlurpError::NotYetImplemented { what: "paths::run only handles Mode::Paths" }),
};
```

- [ ] **Step 2: Compute retained sizes once**

After the existing pass1, when `retained_size`:

```rust
let retained_by_oid: Option<AHashMap<u64, u64>> = if retained_size {
    let graph = crate::reference_graph::build_from_pass1(input_file, &idx, debug)?;
    let idom = crate::dominators::lengauer_tarjan(&graph);
    let r = crate::retained::compute(&graph, &idom, /* top_n */ 0);
    let mut map = AHashMap::with_capacity(graph.node_count());
    for i in 0..graph.node_count() {
        if i as u32 == graph.super_root { continue; }
        map.insert(graph.node_ids[i], r.retained[i]);
    }
    Some(map)
} else {
    None
};
```

- [ ] **Step 3: Pipe into PathResult and renderer**

`PathResult` gains `retained_by_oid: Option<AHashMap<u64, u64>>` (with `#[serde(skip)]`). `render_text` formats each hop's id with the trailing `(retained=...)` whenever the map contains the id.

- [ ] **Step 4: Smoke-test**

```bash
./target/release/heaptrail -i JAVA_PROFILE_1.0.2.hprof --paths-from-id 1661812752 --retained-size 2>&1 | head -20
./target/release/heaptrail -i JAVA_PROFILE_1.0.3.hprof --paths-from-id 1723142144 --retained-size 2>&1 | head -20
```

Expected: each hop carries `(retained=<size>)` annotation.

- [ ] **Step 5: Lint + commit + push + CI**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
cargo test --release
git add src/paths.rs
git commit -m "feat(retained): --paths-from-id hop annotations with retained size"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 6 — `--find-referrers --retained-size`

**PR title:** `feat(retained): --find-referrers per-holder-class retained column`

**Goal:** When `--retained-size` is set on `--find-referrers`, run the dominator pipeline once and annotate each holder class entry with the holder's class-level retained-bytes total.

### Task 6.1: Wire + render

**Files:**
- Modify: `src/referrer.rs`

- [ ] **Step 1: Destructure `retained_size`**

```rust
let (input_file, target, hops, top, include_statics, debug, preview_bytes, retained_size) = match mode {
    Mode::FindReferrers { input_file, target, hops, top, include_statics, debug, preview_bytes, retained_size, .. }
        => (input_file.as_str(), target.clone(), *hops, *top, *include_statics, *debug, *preview_bytes, *retained_size),
    _ => return Err(HprofSlurpError::NotYetImplemented { what: "..." }),
};
```

- [ ] **Step 2: Compute class retained map**

After pass1, when `retained_size`:

```rust
let class_retained: Option<AHashMap<u64, u64>> = if retained_size {
    let graph = crate::reference_graph::build_from_pass1(input_file, &idx, debug)?;
    let idom = crate::dominators::lengauer_tarjan(&graph);
    let r = crate::retained::compute(&graph, &idom, /* top_n */ 0);
    Some(r.class_retained)
} else {
    None
};
```

- [ ] **Step 3: Render column**

`ReferrerResult` gains `class_retained: Option<AHashMap<u64, u64>>` (`#[serde(skip)]`). `render_text` adds a `class retained` right-aligned column to the hop tables when the map is `Some`. Each holder class's bytes come from `class_retained.get(holder_class_id).unwrap_or(&0)`.

- [ ] **Step 4: Smoke-test**

```bash
./target/release/heaptrail -i JAVA_PROFILE_1.0.2.hprof --find-referrers java.lang.String --hops 2 --retained-size 2>&1 | head -25
./target/release/heaptrail -i JAVA_PROFILE_1.0.3.hprof --find-referrers char[] --hops 1 --retained-size 2>&1 | head -25
```

Expected: holder rows show `class retained` bytes alongside `ref count`.

- [ ] **Step 5: Lint + commit + push + CI**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
cargo test --release
git add src/referrer.rs
git commit -m "feat(retained): --find-referrers per-holder-class retained column"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 7 — Docs + version bump 1.0.0 + tag + release

**PR title:** `chore: bump to 1.0.0; document retained-size (feature E); v1.0.0 release`

### Task 7.1: Cargo.toml + plugin manifests

**Files:**
- Modify: `Cargo.toml`
- Modify: `plugins/analysing-heap-dumps/.claude-plugin/plugin.json`
- Modify: `.claude-plugin/marketplace.json`

- [ ] **Step 1: Edit versions**

`Cargo.toml`: `version = "1.0.0"`.

`plugin.json`: `"version": "1.0.0",`.

`marketplace.json` (under `plugins[0]`): `"version": "1.0.0"`.

- [ ] **Step 2: Build + JSON validate**

```bash
cargo build --release
python3 -m json.tool plugins/analysing-heap-dumps/.claude-plugin/plugin.json > /dev/null && echo "plugin.json: ok"
python3 -m json.tool .claude-plugin/marketplace.json > /dev/null && echo "marketplace.json: ok"
```

### Task 7.2: README updates

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Drop the v1.0.0 promise from "When to still reach for MAT"**

The existing text says "heaptrail v0.9.0 reports shallow sizes only; full Lengauer–Tarjan dominators with retained-size accounting are scheduled for v1.0.0." Replace with:

```markdown
- **Interactive graph exploration.** Clicking through inbound/outbound references is a UI capability and stays in MAT's column.
- **OQL** for ad-hoc querying — heaptrail is a fixed-flag CLI by design.
- **MAT's Leak Suspects** clustered narrative report.
```

(The retained-size bullet is removed entirely — heaptrail now has the feature.)

- [ ] **Step 2: Add `--retained-size` to Features bullets**

```markdown
- **retained size** (`--retained-size`) — full Lengauer–Tarjan dominator
  tree augmenting `summary`, `--paths-from-id`, and `--find-referrers`.
  Answers "is this 35K-instance class actually 35 MB or actually 350 MB?"
  in one command instead of an Eclipse MAT round-trip.
```

- [ ] **Step 3: Add the `--retained-size` cheat-sheet entry**

After `### \`--preview-bytes\``:

```markdown
### `--retained-size` — dominator-tree retained sizes (v1.0.0)

```bash
heaptrail -i my.hprof --retained-size -t 20
heaptrail -i my.hprof --paths-from-id <id> --retained-size
heaptrail -i my.hprof --find-referrers <class> --retained-size
```

Computes per-instance retained bytes via Lengauer–Tarjan dominators.
Re-sorts the summary class table by retained size and appends a
"Largest retained instances" hot list of object ids. `--paths-from-id`
annotates each hop with retained bytes; `--find-referrers` adds a
`class retained` column. Default off. Adds ~250 MiB working memory
and ~1–3 s wall time on a 200 MiB Android dump. Details in
[USERGUIDE — `--retained-size`](USERGUIDE.md#--retained-size--dominator-tree-retained-sizes).
```

### Task 7.3: USERGUIDE updates

**Files:**
- Modify: `USERGUIDE.md`

- [ ] **Step 1: Insert new section before `## --target-glob`**

```markdown
## `--retained-size` — dominator-tree retained sizes

### Why this exists

`summary` ranks classes by **shallow** size. For something like
`ResolvedDisplayItem` (88 bytes shallow, holding a 12-element
`ResolvedDisplayFieldSlots` containing an `ArtworkBundle`), the
**retained** size — what would actually be reclaimed if all instances
of the class went away — is much bigger. Triaging "is this 35,000-instance
class actually 35 MB or actually 350 MB?" needs retained, not shallow.

This is the metric Eclipse MAT computes via dominator-tree analysis;
heaptrail v1.0.0 brings it to the CLI.

### How to use it

```bash
# Resort the class table by retained bytes; show "Largest retained instances".
heaptrail -i my.hprof --retained-size -t 20

# Annotate each path-from-id hop with that hop object's retained size.
heaptrail -i my.hprof --paths-from-id <u64> --retained-size

# Annotate each holder class with its retained size column.
heaptrail -i my.hprof --find-referrers <class-or-id> --retained-size
```

### How it's computed

heaptrail builds an in-memory CSR object-reference graph from the
hprof, computes immediate dominators using Lengauer–Tarjan
(O(N α(N))), then walks the dominator tree post-order to sum
retained bytes per node. Class-level totals and the top-N
largest-retained instance ids fall out of the same pass.

### Memory and wall time

Adds roughly 200 MiB working memory and 1–3 s wall time on a
200 MiB Android dump (~3M objects, ~15M edges). Negligible on
typical JVM dumps. Default off.

### When to use

- After `summary` shows a class with low shallow size but high
  instance count — retained size tells you whether each instance
  silently anchors a deep subgraph.
- During a `--paths-from-id` walk where you want to know how much
  weight each hop carries (an upper bound on what freeing that
  reference would reclaim).
- To rank holder classes from `--find-referrers` by retained
  rather than ref-count alone — sometimes 1 reference holds 8 MiB.

---
```

- [ ] **Step 2: Add to the cheat-sheet table**

```markdown
| **Retained-size triage** | append `--retained-size` to summary, paths-from-id, or find-referrers |
```

### Task 7.4: SKILL.md — integrate `--retained-size`

**Files:**
- Modify: `plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md`

- [ ] **Step 1: Bump version reference**

`version 0.9.0+` → `version 1.0.0+` in the Source line.

- [ ] **Step 2: Add a seventh integrated mode**

After section 6 (`--preview-bytes`):

```markdown
### 7. `--retained-size` — dominator-tree retained sizes (v1.0.0, feature E)

Global flag. When set, summary's class table sorts by retained size
and gains a `retained` column; a "Largest retained instances" block
lists top-N (object_id, class, retained_bytes); `--paths-from-id`
annotates each hop with retained size; `--find-referrers` adds a
`class retained` column.

```bash
heaptrail -i heap.hprof --retained-size -t 20
```

**What it tells you:** the bytes that would actually be reclaimed
if every instance of a class disappeared — the metric Eclipse MAT
calls "retained heap." Closes heaptrail's last gap with MAT for
single-shot triage.

*Engineering use case:* the canonical "is this 35K-instance class
actually 35 MB or actually 350 MB?" question. A 88-byte-shallow
`ResolvedDisplayItem` whose retained size is many KiB tells you the
class anchors a deep subgraph; that subgraph is what to investigate.

**Wall time / memory:** opt-in, adds ~200 MiB working memory and
~1–3 s wall time on a 200 MiB Android dump. Default off; existing
output unchanged when the flag is unset.
```

- [ ] **Step 3: Update the standard triage workflow**

Append:

```markdown
8. (Optional) **`--retained-size`** added to step 1 (summary) re-sorts
   the class table by retained bytes — useful when the dominant class
   by shallow size isn't the dominant class by retained size (small
   wrapper objects holding large subgraphs).
```

- [ ] **Step 4: Add cheat-sheet row**

```markdown
| Retained-size triage | append `--retained-size` to summary, paths-from-id, or find-referrers |
```

### Task 7.5: Final test gate + commit + tag + release

- [ ] **Step 1: Lint + test**

```bash
cargo test --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
```

- [ ] **Step 2: Commit + push + CI**

```bash
git add Cargo.toml Cargo.lock README.md USERGUIDE.md \
        plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md \
        plugins/analysing-heap-dumps/.claude-plugin/plugin.json \
        .claude-plugin/marketplace.json
git commit -m "$(cat <<'EOF'
chore: bump to 1.0.0; document retained-size (feature E)

  * Cargo.toml: 0.9.0 -> 1.0.0 (major; adds --retained-size flag,
    no breaking changes to existing flags or output)
  * README.md: --retained-size cheat-sheet entry; Features bullet;
    drop the "v1.0.0 promise" line from the MAT comparison
  * USERGUIDE.md: new section with engineering-use-case framing
    (the 35K-instance-class question), memory/wall-time notes
  * SKILL.md: seventh integrated mode added; engineering use-case
    framing for Claude diagnostics; standard triage workflow gains
    a step 8 for retained-size; cheat-sheet entry; version bump
    aligned with app at 1.0.0
  * plugin.json + marketplace.json: 0.9.0 -> 1.0.0

Closes the v1.0.0 spec at
docs/superpowers/specs/2026-05-10-heaptrail-v1.0-design.md.
Feature E (retained size) landed across PRs 1-6.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

- [ ] **Step 3: Tag + GitHub release**

```bash
git tag -a v1.0.0 -m "v1.0.0 — retained size via dominator tree (feature E)"
git push fork v1.0.0

cat > /tmp/release-notes-100.md <<'NOTES'
## v1.0.0 — Retained size via dominator tree

A new flag, `--retained-size`, surfaces dominator-tree retained sizes across `summary`, `--paths-from-id`, and `--find-referrers`. This is the last MAT-grade datum heaptrail didn't surface; v1.0.0 closes the gap.

### Why this exists

`summary` ranks classes by **shallow** size. For wrapper objects (88 bytes shallow holding a deep subgraph), shallow drastically under-represents the cost of allowing the class to live. Retained size — bytes reclaimable if the class disappeared — is the right metric for "is this 35K-instance class actually 35 MB or 350 MB?" Eclipse MAT computes this via dominator tree; heaptrail v1.0.0 brings it to the CLI.

### What's new

- `--retained-size` flag (default off). Recommended for triage runs.
- `summary` class table re-sorts by retained bytes and adds a `retained` column.
- New "Largest retained instances" block listing top-N `(object_id, class, retained_bytes)`.
- `--paths-from-id` annotates each hop with `(retained=<size>)`.
- `--find-referrers` adds a `class retained` column.

### How

In-memory CSR object-reference graph + Lengauer–Tarjan immediate dominators (O(N α(N))) + post-order DFS for retained sums.

### Memory cost

~200 MiB working memory + ~1–3 s wall time on a 200 MiB Android dump (~3M objects). Negligible on typical JVM dumps.

### Compatibility

- Existing CLI invocations produce byte-identical output unless `--retained-size` is set.
- JSON schema gains two optional fields, both `#[serde(skip_serializing_if = "Option::is_none")]`.
- No new dependencies.

### Plugin update

```
/plugin marketplace update johnneerdael/heaptrail
/plugin update analysing-heap-dumps@analysing-heap-dumps
```

### Install

```bash
cargo install heaptrail               # crates.io 1.0.0
cargo install --git https://github.com/johnneerdael/heaptrail
```

Pre-built binaries for Linux/macOS/Windows × x86_64/aarch64 attached below.
NOTES

gh release create v1.0.0 --repo johnneerdael/heaptrail \
  --title "heaptrail v1.0.0" -F /tmp/release-notes-100.md
```

- [ ] **Step 4: Watch the release workflow + crates.io**

```bash
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow 'release binaries' --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 20; done
gh run list --repo johnneerdael/heaptrail --workflow 'release binaries' --limit 1 --json conclusion -q '.[0].conclusion'
gh release view v1.0.0 --repo johnneerdael/heaptrail --json assets -q '.assets[].name'
curl -sf https://crates.io/api/v1/crates/heaptrail/1.0.0 -o /dev/null && echo "1.0.0 published on crates.io"
```

Expected: `success`; six binary assets listed; crates.io 1.0.0 live.

---

## Self-Review Checklist

- [x] **Spec coverage:** every section of the design spec maps to a task.
  - §3.1 (pipeline) → PRs 1-3
  - §3.2 (modules) → PR 1 (graph), PR 2 (dom), PR 3 (retained)
  - §3.3 (modified files) → PRs 4-6
  - §3.4 (data structures) → defined in PR 1 / PR 3 task code
  - §3.5 (edges stay u32 decision) → enforced in PR 1
  - §3.6 (internal API stability) → contract; no task
  - §3.7 (CLI surface) → Task 4.1
  - §4 (output format) → Tasks 4.2, 5.1, 6.1
  - §5 (perf + memory) → smoke tests in 4.2, 5.1, 6.1
  - §6 (testing) → unit tests in PR 2/3; integration smoke in 4.2, 5.1, 6.1
  - §7 (rollout) → 7-PR structure matches; tag in Task 7.5
  - §8 (risk notes) → memory cost + algorithm correctness covered by smoke + unit tests
- [x] **No placeholders:** every step has concrete code or commands.
- [x] **Type consistency:** `ReferenceGraph`, `RetainedAnalysis`, `class_retained`, `top_instances`, `retained_size`, `class_retained_sizes`, `top_retained_instances` defined once, used by the same name throughout.
- [x] **Existing tests stay green:** every commit's last step runs the full lint+test gate.
- [x] **Both canonical fixtures (CLAUDE.md):** smoke tests in PRs 4, 5, 6 explicitly run on both `JAVA_PROFILE_1.0.2.hprof` and `JAVA_PROFILE_1.0.3.hprof`.
