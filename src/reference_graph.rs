// Dead-code allowance until PR 4+ wires summary/paths/find-referrers
// consumers. The `#[cfg(test)]` block + canonical-fixture smoke tests
// exercise every public item below.
#![allow(dead_code)]

//! In-memory CSR object-reference graph used by the retained-size
//! pipeline (v1.0.0 feature E).
//!
//! Built by streaming the hprof a second time with `retain_bodies=true`,
//! walking each instance dump's body using the flattened (own + super)
//! field layout cached in `Pass1Index`, and pushing referenced object
//! indices into a CSR adjacency structure.
//!
//! Memory: roughly `8 + 4 × refs_per_node` bytes per node. A 200 MiB
//! Android dump (~3M objects, ~5 refs/object) lands at ~120 MiB total
//! for nodes + edges. See
//! `docs/superpowers/specs/2026-05-10-heaptrail-v1.0-design.md` §5
//! for the full memory budget.

use ahash::AHashMap;

use crate::errors::HprofSlurpError;
use crate::parser::gc_record::{FieldType, GcRecord};
use crate::parser::record::Record;
use crate::referrer::Pass1Index;
use crate::slurp::parse_records;

pub struct ReferenceGraph {
    /// Object ids in node-index order. The last entry (index ==
    /// `super_root`) is the virtual super-root that owns every GC
    /// root; its object id slot is 0.
    pub node_ids: Vec<u64>,
    /// Class index per node (index into `class_ids`); `u32::MAX` for
    /// the super-root.
    pub node_class: Vec<u32>,
    /// Shallow size per node, bytes.
    pub node_shallow: Vec<u32>,
    /// `class_index → class_object_id`. Built lazily as unseen class
    /// ids are encountered. Primitive arrays use synthetic class ids
    /// in the `0xFFFF_FFFF_FFFF_FF00` range.
    pub class_ids: Vec<u64>,
    /// CSR row pointers, length `node_count() + 1`.
    pub edges_offsets: Vec<u32>,
    /// CSR column indices.
    pub edges_targets: Vec<u32>,
    /// Index of the virtual super-root (== `node_count() - 1`).
    pub super_root: u32,
    /// Reverse lookup: object_id → node_index. Used by callers (paths,
    /// referrers) to look up specific object ids.
    pub index_by_object_id: AHashMap<u64, u32>,
}

impl ReferenceGraph {
    pub fn node_count(&self) -> usize {
        self.node_ids.len()
    }
    pub fn edge_count(&self) -> usize {
        self.edges_targets.len()
    }
    pub fn out_edges(&self, node: u32) -> &[u32] {
        let start = self.edges_offsets[node as usize] as usize;
        let end = self.edges_offsets[node as usize + 1] as usize;
        &self.edges_targets[start..end]
    }
    pub fn node_index_of(&self, object_id: u64) -> Option<u32> {
        self.index_by_object_id.get(&object_id).copied()
    }
}

/// Streams `path` once with `retain_bodies=true`. Builds the full
/// CSR graph in a single pass: nodes are accumulated as records
/// arrive, edges (instance refs + object-array elements) are buffered
/// keyed by source object id and resolved to node indices after the
/// node set is finalized. Edges from the virtual super-root are added
/// at the end from `Pass1Index` (GC roots + class statics).
pub fn build_from_pass1(
    path: &str,
    idx: &Pass1Index,
    debug: bool,
) -> Result<ReferenceGraph, HprofSlurpError> {
    let id_size = idx.id_size;

    let mut node_ids = Vec::<u64>::new();
    let mut node_class = Vec::<u32>::new();
    let mut node_shallow = Vec::<u32>::new();
    let mut class_ids = Vec::<u64>::new();
    let mut class_index_by_id = AHashMap::<u64, u32>::new();

    // Edge buffer keyed by source object id; resolved to node indices below.
    let mut edge_buf = Vec::<(u64, u64)>::new();

    parse_records(path, debug, true, |record| {
        if let Record::GcSegment(gc) = record {
            match gc {
                GcRecord::InstanceDump {
                    object_id,
                    class_object_id,
                    body,
                    ..
                } => {
                    let ci = class_index(&mut class_ids, &mut class_index_by_id, class_object_id);
                    let size = instance_shallow_size(idx, class_object_id);
                    node_ids.push(object_id);
                    node_class.push(ci);
                    node_shallow.push(size);
                    if let Some(b) = body {
                        extract_refs_into(idx, class_object_id, &b, object_id, &mut edge_buf);
                    }
                }
                GcRecord::ObjectArrayDump {
                    object_id,
                    array_class_id,
                    number_of_elements,
                    elements,
                    ..
                } => {
                    let ci = class_index(&mut class_ids, &mut class_index_by_id, array_class_id);
                    let size = (id_size as u64).saturating_mul(number_of_elements as u64);
                    node_ids.push(object_id);
                    node_class.push(ci);
                    node_shallow.push(size.min(u32::MAX as u64) as u32);
                    if let Some(elems) = elements {
                        for &dst in elems.iter() {
                            if dst != 0 {
                                edge_buf.push((object_id, dst));
                            }
                        }
                    }
                }
                GcRecord::PrimitiveArrayDump {
                    object_id,
                    element_type,
                    number_of_elements,
                    ..
                } => {
                    let synthetic = primitive_synthetic_class_id(element_type);
                    let ci = class_index(&mut class_ids, &mut class_index_by_id, synthetic);
                    let size = primitive_array_size(id_size, element_type, number_of_elements);
                    node_ids.push(object_id);
                    node_class.push(ci);
                    node_shallow.push(size.min(u32::MAX as u64) as u32);
                }
                GcRecord::PrimitiveArrayNoDataDump {
                    object_id,
                    element_type,
                    ..
                } => {
                    let synthetic = primitive_synthetic_class_id(element_type);
                    let ci = class_index(&mut class_ids, &mut class_index_by_id, synthetic);
                    node_ids.push(object_id);
                    node_class.push(ci);
                    node_shallow.push(0);
                }
                _ => {}
            }
        }
    })?;

    // Sort by object_id for deterministic node-index assignment.
    let n_real = node_ids.len();
    let mut order: Vec<u32> = (0..n_real as u32).collect();
    order.sort_unstable_by_key(|&i| node_ids[i as usize]);
    let node_ids = order
        .iter()
        .map(|&i| node_ids[i as usize])
        .collect::<Vec<_>>();
    let node_class = order
        .iter()
        .map(|&i| node_class[i as usize])
        .collect::<Vec<_>>();
    let node_shallow = order
        .iter()
        .map(|&i| node_shallow[i as usize])
        .collect::<Vec<_>>();

    // Append the virtual super-root.
    let super_root = node_ids.len() as u32;
    let mut node_ids = node_ids;
    node_ids.push(0);
    let mut node_class = node_class;
    node_class.push(u32::MAX);
    let mut node_shallow = node_shallow;
    node_shallow.push(0);

    // Reverse lookup table.
    let mut index_by_object_id = AHashMap::with_capacity(n_real);
    for (i, &oid) in node_ids.iter().enumerate().take(n_real) {
        index_by_object_id.insert(oid, i as u32);
    }

    // Super-root edges: GC roots + every object-typed class static.
    for &root in &idx.gc_root_ids {
        if index_by_object_id.contains_key(&root) {
            edge_buf.push((0, root));
        }
    }
    for statics in idx.static_object_fields_by_class_id.values() {
        for &(_name_id, target) in statics {
            if target != 0 && index_by_object_id.contains_key(&target) {
                edge_buf.push((0, target));
            }
        }
    }

    // Resolve to (src_idx, dst_idx); src_oid == 0 sentinel means super-root.
    let mut resolved = Vec::<(u32, u32)>::with_capacity(edge_buf.len());
    for (src_oid, dst_oid) in edge_buf {
        let src_idx = if src_oid == 0 {
            super_root
        } else if let Some(&i) = index_by_object_id.get(&src_oid) {
            i
        } else {
            continue;
        };
        let dst_idx = if let Some(&i) = index_by_object_id.get(&dst_oid) {
            i
        } else {
            continue;
        };
        resolved.push((src_idx, dst_idx));
    }
    resolved.sort_unstable_by_key(|&(s, _)| s);

    let total_nodes = node_ids.len();
    let mut edges_offsets = vec![0u32; total_nodes + 1];
    for &(s, _) in &resolved {
        edges_offsets[s as usize + 1] += 1;
    }
    for i in 1..edges_offsets.len() {
        edges_offsets[i] += edges_offsets[i - 1];
    }
    let mut edges_targets = vec![0u32; resolved.len()];
    let mut cursors = edges_offsets.clone();
    for (s, d) in resolved {
        let p = cursors[s as usize] as usize;
        edges_targets[p] = d;
        cursors[s as usize] += 1;
    }

    Ok(ReferenceGraph {
        node_ids,
        node_class,
        node_shallow,
        class_ids,
        edges_offsets,
        edges_targets,
        super_root,
        index_by_object_id,
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

/// Walks the (own + super) field layout for a class and sums shallow
/// bytes. Caps at `u32::MAX` for safety on pathological dumps.
fn instance_shallow_size(idx: &Pass1Index, class_object_id: u64) -> u32 {
    let id_size = idx.id_size as u64;
    let mut size: u64 = 0;
    let mut cls = Some(class_object_id);
    while let Some(c) = cls {
        if c == 0 {
            break;
        }
        if let Some(fields) = idx.fields_by_class_id.get(&c) {
            for f in fields {
                size += field_size(id_size, f.field_type);
            }
        }
        cls = idx.super_class_by_id.get(&c).copied();
    }
    size.min(u32::MAX as u64) as u32
}

fn field_size(id_size: u64, t: FieldType) -> u64 {
    match t {
        FieldType::Object => id_size,
        FieldType::Bool | FieldType::Byte => 1,
        FieldType::Char | FieldType::Short => 2,
        FieldType::Int | FieldType::Float => 4,
        FieldType::Long | FieldType::Double => 8,
    }
}

fn primitive_synthetic_class_id(t: FieldType) -> u64 {
    let n: u64 = match t {
        FieldType::Object => 0,
        FieldType::Bool => 1,
        FieldType::Byte => 2,
        FieldType::Char => 3,
        FieldType::Short => 4,
        FieldType::Int => 5,
        FieldType::Float => 6,
        FieldType::Long => 7,
        FieldType::Double => 8,
    };
    0xFFFF_FFFF_FFFF_FF00u64 | n
}

fn primitive_array_size(id_size: u32, t: FieldType, n: u32) -> u64 {
    field_size(id_size as u64, t).saturating_mul(n as u64)
}

/// Walks an instance body using the (own + super) field layout for
/// `class_object_id` and pushes every non-null object reference into
/// `out` keyed by `src_oid`.
fn extract_refs_into(
    idx: &Pass1Index,
    class_object_id: u64,
    body: &[u8],
    src_oid: u64,
    out: &mut Vec<(u64, u64)>,
) {
    let id_size = idx.id_size as usize;
    let mut cursor = 0usize;
    let mut cls = Some(class_object_id);
    while let Some(c) = cls {
        if c == 0 {
            break;
        }
        if let Some(fields) = idx.fields_by_class_id.get(&c) {
            for f in fields {
                let size = field_size(id_size as u64, f.field_type) as usize;
                if cursor + size > body.len() {
                    return;
                }
                if f.field_type == FieldType::Object {
                    let dst = match id_size {
                        4 => {
                            u32::from_be_bytes(body[cursor..cursor + 4].try_into().unwrap()) as u64
                        }
                        8 => u64::from_be_bytes(body[cursor..cursor + 8].try_into().unwrap()),
                        _ => 0,
                    };
                    if dst != 0 {
                        out.push((src_oid, dst));
                    }
                }
                cursor += size;
            }
        }
        cls = idx.super_class_by_id.get(&c).copied();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_class_ids_are_distinct_per_field_type() {
        let bytes = primitive_synthetic_class_id(FieldType::Byte);
        let chars = primitive_synthetic_class_id(FieldType::Char);
        let ints = primitive_synthetic_class_id(FieldType::Int);
        assert_ne!(bytes, chars);
        assert_ne!(bytes, ints);
        assert_ne!(chars, ints);
        // All fall in the high sentinel range.
        assert!(bytes >> 8 == 0x00FF_FFFF_FFFF_FFFFu64);
    }

    #[test]
    fn field_size_matches_hprof_spec() {
        assert_eq!(field_size(4, FieldType::Object), 4);
        assert_eq!(field_size(8, FieldType::Object), 8);
        assert_eq!(field_size(8, FieldType::Bool), 1);
        assert_eq!(field_size(8, FieldType::Char), 2);
        assert_eq!(field_size(8, FieldType::Int), 4);
        assert_eq!(field_size(8, FieldType::Long), 8);
    }

    #[test]
    fn build_on_canonical_jvm_fixture_produces_nontrivial_graph() {
        let path = "JAVA_PROFILE_1.0.2.hprof";
        if !std::path::Path::new(path).exists() {
            eprintln!("skipping — fixture {path} not present");
            return;
        }
        let idx = crate::referrer::pass1_index(path, false).expect("pass1");
        let g = build_from_pass1(path, &idx, false).expect("graph");
        assert!(
            g.node_count() > 100,
            "expected many nodes, got {}",
            g.node_count()
        );
        assert!(g.edge_count() > g.node_count(), "expected edges > nodes");
        // Super-root must have at least one outgoing edge (the GC roots).
        assert!(!g.out_edges(g.super_root).is_empty());
    }

    #[test]
    fn build_on_canonical_android_fixture_produces_nontrivial_graph() {
        let path = "JAVA_PROFILE_1.0.3.hprof";
        if !std::path::Path::new(path).exists() {
            eprintln!("skipping — fixture {path} not present");
            return;
        }
        let idx = crate::referrer::pass1_index(path, false).expect("pass1");
        let g = build_from_pass1(path, &idx, false).expect("graph");
        assert!(g.node_count() > 1000);
        assert!(g.edge_count() > g.node_count());
        assert!(!g.out_edges(g.super_root).is_empty());
    }
}
