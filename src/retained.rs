// Dead-code allowance until PR 4+ wires the retained-size pipeline
// into the user-facing modes. The unit tests below exercise every
// public item against synthetic graphs.
#![allow(dead_code)]

//! Retained-size computation. Given a [`ReferenceGraph`] and the
//! `idom` vector from [`crate::dominators::lengauer_tarjan`], computes:
//!
//!   retained[v] = shallow[v] + sum(retained[c]) for c where idom[c] == v
//!
//! Then rolls up to:
//!   * `class_retained: AHashMap<class_object_id, retained_bytes>`
//!   * `top_instances: Vec<(object_id, class_object_id, retained_bytes)>`
//!     sorted descending, length ≤ `top_n`.
//!
//! The DFS is iterative (the dominator tree on a 200 MiB Android dump
//! easily exceeds the default thread stack) and uses post-order on the
//! dominator tree rather than the reference graph itself — that's the
//! correct structure for retained-size accounting (every reachable
//! node has exactly one immediate dominator, so the tree has no
//! cycles).

use ahash::AHashMap;

use crate::reference_graph::ReferenceGraph;

pub struct RetainedAnalysis {
    /// `retained[v]` = shallow[v] + retained sum of all dominator-tree
    /// children. For unreachable nodes (idom == u32::MAX), retained
    /// equals shallow.
    pub retained: Vec<u64>,
    /// `class_object_id → retained_bytes` summed across all instances
    /// of the class. Excludes the super-root.
    pub class_retained: AHashMap<u64, u64>,
    /// `(object_id, class_object_id, retained_bytes)`, sorted by
    /// retained descending, length ≤ `top_n`. Excludes the super-root.
    pub top_instances: Vec<(u64, u64, u64)>,
}

/// Build the dominator-tree children list from `idom`. `dom_children[v]`
/// is the list of nodes whose immediate dominator is `v`. Unreachable
/// nodes (idom == u32::MAX) appear in no list.
///
/// Required by `--leak-suspects` (v1.1.0) for top-down traversal of a
/// suspect's dominated subtree.
pub fn dom_children(idom: &[u32]) -> Vec<Vec<u32>> {
    let n = idom.len();
    let mut children: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (v, &d) in idom.iter().enumerate() {
        if d != u32::MAX && (d as usize) < n {
            children[d as usize].push(v as u32);
        }
    }
    children
}

pub fn compute(graph: &ReferenceGraph, idom: &[u32], top_n: usize) -> RetainedAnalysis {
    let n = graph.node_count();
    if n == 0 {
        return RetainedAnalysis {
            retained: Vec::new(),
            class_retained: AHashMap::new(),
            top_instances: Vec::new(),
        };
    }
    assert_eq!(idom.len(), n, "idom length must match node count");

    // Initialise retained = shallow.
    let mut retained: Vec<u64> = graph.node_shallow.iter().map(|&s| s as u64).collect();

    // Build dominator-tree children list.
    let mut children: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (v, &d) in idom.iter().enumerate() {
        if d != u32::MAX && (d as usize) < n {
            children[d as usize].push(v as u32);
        }
    }

    // Iterative post-order from super_root.
    let r = graph.super_root;
    let mut stack: Vec<(u32, bool)> = Vec::with_capacity(n);
    stack.push((r, false));
    while let Some((v, processed)) = stack.pop() {
        if processed {
            for &c in &children[v as usize] {
                let acc = retained[v as usize].saturating_add(retained[c as usize]);
                retained[v as usize] = acc;
            }
        } else {
            stack.push((v, true));
            for &c in &children[v as usize] {
                stack.push((c, false));
            }
        }
    }

    // Class rollup + top-N hot list. Skip the super-root (its node_class
    // is u32::MAX — `synthetic`/sentinel — and its retained is the
    // total of all reachable bytes, which is uninteresting per-class).
    let mut class_retained: AHashMap<u64, u64> = AHashMap::new();
    let mut all_inst: Vec<(u64, u64, u64)> = Vec::with_capacity(n);
    for (v, &r_v) in retained.iter().enumerate() {
        if v as u32 == graph.super_root {
            continue;
        }
        let ci = graph.node_class[v];
        if ci == u32::MAX {
            continue;
        }
        let class_id = graph.class_ids[ci as usize];
        *class_retained.entry(class_id).or_insert(0) += r_v;
        all_inst.push((graph.node_ids[v], class_id, r_v));
    }
    all_inst.sort_unstable_by_key(|t| std::cmp::Reverse(t.2));
    all_inst.truncate(top_n);

    RetainedAnalysis {
        retained,
        class_retained,
        top_instances: all_inst,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dominators::lengauer_tarjan;
    use ahash::AHashMap;

    fn graph_with_shallow(
        n: usize,
        edges: &[(u32, u32)],
        roots: &[u32],
        shallow: &[u32],
        classes: &[u64],
    ) -> ReferenceGraph {
        let super_root = n as u32;
        let total = n + 1;
        // class_ids assignment in encounter order so test class indices
        // are predictable.
        let mut class_ids: Vec<u64> = Vec::new();
        let mut by_id: AHashMap<u64, u32> = AHashMap::new();
        let node_class: Vec<u32> = classes
            .iter()
            .map(|c| {
                *by_id.entry(*c).or_insert_with(|| {
                    let i = class_ids.len() as u32;
                    class_ids.push(*c);
                    i
                })
            })
            .collect();
        let mut node_class = node_class;
        node_class.push(u32::MAX);

        let mut node_shallow: Vec<u32> = shallow.to_vec();
        node_shallow.push(0);

        let node_ids: Vec<u64> = (0..total as u64).collect();

        let mut all: Vec<(u32, u32)> = edges.to_vec();
        for &r in roots {
            all.push((super_root, r));
        }
        all.sort_by_key(|&(s, _)| s);

        let mut edges_offsets = vec![0u32; total + 1];
        for &(s, _) in &all {
            edges_offsets[s as usize + 1] += 1;
        }
        for i in 1..edges_offsets.len() {
            edges_offsets[i] += edges_offsets[i - 1];
        }
        let mut edges_targets = vec![0u32; all.len()];
        let mut cur = edges_offsets.clone();
        for (s, d) in all {
            let p = cur[s as usize] as usize;
            edges_targets[p] = d;
            cur[s as usize] += 1;
        }
        ReferenceGraph {
            node_ids,
            node_class,
            node_shallow,
            class_ids,
            edges_offsets,
            edges_targets,
            super_root,
            index_by_object_id: AHashMap::new(),
        }
    }

    #[test]
    fn dom_children_reflects_idom_inversion() {
        // super → 0 → 1, super → 0 → 2, super → 0 → 3 (linear under 0)
        let g = graph_with_shallow(
            4,
            &[(0, 1), (0, 2), (0, 3)],
            &[0],
            &[10, 10, 10, 10],
            &[1, 1, 1, 1],
        );
        let idom = lengauer_tarjan(&g);
        let kids = dom_children(&idom);
        let sr = g.super_root as usize;
        assert_eq!(kids[sr], vec![0]);
        let mut k0 = kids[0].clone();
        k0.sort_unstable();
        assert_eq!(k0, vec![1, 2, 3]);
        assert!(kids[1].is_empty() && kids[2].is_empty() && kids[3].is_empty());
    }

    #[test]
    fn linear_chain_retains_tail_under_head() {
        // super → 0 → 1 → 2; sizes 10/20/30; class 100 for all.
        let g = graph_with_shallow(3, &[(0, 1), (1, 2)], &[0], &[10, 20, 30], &[100, 100, 100]);
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
        // super → 0; 0 → 1; 0 → 2; 1 → 3; 2 → 3; sizes all 10; one class.
        let g = graph_with_shallow(
            4,
            &[(0, 1), (0, 2), (1, 3), (2, 3)],
            &[0],
            &[10, 10, 10, 10],
            &[1, 1, 1, 1],
        );
        let idom = lengauer_tarjan(&g);
        let r = compute(&g, &idom, 5);
        // Dominator tree under super: super → 0 → {1, 2, 3}
        // (3 is dominated by 0 because both 1 and 2 reach it; 1 and 2 are
        // siblings under 0.)
        assert_eq!(r.retained[0], 40);
        assert_eq!(r.retained[1], 10);
        assert_eq!(r.retained[2], 10);
        assert_eq!(r.retained[3], 10);
    }

    #[test]
    fn class_rollup_and_top_instances() {
        // super → 0; 0 → 1; 1 → 2. shallow 10/100/1000. classes 1/2/3.
        let g = graph_with_shallow(3, &[(0, 1), (1, 2)], &[0], &[10, 100, 1000], &[1, 2, 3]);
        let idom = lengauer_tarjan(&g);
        let r = compute(&g, &idom, 5);
        assert_eq!(r.retained[0], 1110);
        assert_eq!(r.retained[1], 1100);
        assert_eq!(r.retained[2], 1000);
        assert_eq!(r.class_retained[&1], 1110);
        assert_eq!(r.class_retained[&2], 1100);
        assert_eq!(r.class_retained[&3], 1000);
        // Top instances sorted by retained desc: node 0 first (1110).
        assert_eq!(r.top_instances[0].2, 1110);
        assert_eq!(r.top_instances[1].2, 1100);
        assert_eq!(r.top_instances[2].2, 1000);
    }
}
