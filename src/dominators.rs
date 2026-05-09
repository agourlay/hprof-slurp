// Dead-code allowance until PR 4+ wires summary/paths/find-referrers
// consumers. Algorithm tested via the textbook fixtures in `tests`.
#![allow(dead_code)]

//! Lengauer–Tarjan O(N α(N)) immediate-dominator computation.
//!
//! Reference: Lengauer & Tarjan, "A Fast Algorithm for Finding
//! Dominators in a Flowgraph", TOPLAS 1979.
//!
//! Input: a [`ReferenceGraph`] whose `super_root` is the unique entry
//! node (every other node is reached along forward edges from it).
//!
//! Output: `idom[i]` is the immediate-dominator node index of node
//! `i`. For the super-root and for nodes unreachable from the
//! super-root, `idom[i] == u32::MAX` (sentinel).

use crate::reference_graph::ReferenceGraph;

pub fn lengauer_tarjan(graph: &ReferenceGraph) -> Vec<u32> {
    let n = graph.node_count();
    let r = graph.super_root as usize;
    if n == 0 {
        return Vec::new();
    }

    // Step 1 — DFS from super_root, assign DFS numbers.
    let mut parent = vec![u32::MAX; n]; // DFS-tree parent (in node-index space)
    let mut semi = vec![u32::MAX; n]; // semi-dominator (DFS-num)
    let mut vertex: Vec<u32> = Vec::with_capacity(n); // DFS-num → node
    let mut dfnum = vec![u32::MAX; n]; // node → DFS-num (or MAX if unreached)

    {
        // Iterative DFS that visits children in left-to-right order
        // (matches the recursive textbook formulation when we push
        // children in reverse).
        let mut stack: Vec<(u32, u32)> = Vec::new(); // (node, dfs-tree parent node-idx)
        stack.push((r as u32, u32::MAX));
        while let Some((v, p)) = stack.pop() {
            if dfnum[v as usize] != u32::MAX {
                continue;
            }
            let num = vertex.len() as u32;
            dfnum[v as usize] = num;
            semi[v as usize] = num;
            parent[v as usize] = p;
            vertex.push(v);
            // Reverse-iterate so that pop order is left-to-right.
            for &w in graph.out_edges(v).iter().rev() {
                if dfnum[w as usize] == u32::MAX {
                    stack.push((w, v));
                }
            }
        }
    }

    // Predecessors (only over reached nodes). Indexed by node.
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    for &v in &vertex {
        for &w in graph.out_edges(v) {
            if dfnum[w as usize] != u32::MAX {
                preds[w as usize].push(v);
            }
        }
    }

    // Link/Eval scratch.
    let mut ancestor = vec![u32::MAX; n];
    let mut label: Vec<u32> = (0..n as u32).collect(); // label[v] starts as v itself
    let mut bucket: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut idom = vec![u32::MAX; n];

    // Reverse-DFS-number iteration over reached nodes (skip the root at i=0).
    for i in (1..vertex.len()).rev() {
        let w = vertex[i];

        // Step 2 — semidominator of w.
        for vi in 0..preds[w as usize].len() {
            let v = preds[w as usize][vi];
            let u = eval(v, &mut ancestor, &mut label, &semi);
            if semi[u as usize] < semi[w as usize] {
                semi[w as usize] = semi[u as usize];
            }
        }
        let semi_w_node = vertex[semi[w as usize] as usize];
        bucket[semi_w_node as usize].push(w);

        // Link w to its DFS-tree parent.
        ancestor[w as usize] = parent[w as usize];

        // Step 3 — implicit immediate dominator for vertices in
        // bucket[parent(w)].
        let pw_node = parent[w as usize];
        if pw_node == u32::MAX {
            continue;
        }
        let drained = std::mem::take(&mut bucket[pw_node as usize]);
        for v in drained {
            let u = eval(v, &mut ancestor, &mut label, &semi);
            idom[v as usize] = if semi[u as usize] < semi[v as usize] {
                u
            } else {
                pw_node
            };
        }
    }

    // Step 4 — fix idoms whose semidominator was deferred.
    for &w in &vertex[1..] {
        let semi_w_node = vertex[semi[w as usize] as usize];
        if idom[w as usize] != semi_w_node {
            let parent_idom = idom[idom[w as usize] as usize];
            idom[w as usize] = parent_idom;
        }
    }

    idom[r] = u32::MAX;
    idom
}

/// Path-compressing eval. Returns the node with the minimum-`semi`
/// label along the ancestor chain rooted at `v`.
fn eval(v: u32, ancestor: &mut [u32], label: &mut [u32], semi: &[u32]) -> u32 {
    if ancestor[v as usize] == u32::MAX {
        return label[v as usize];
    }
    // Collect the chain so we can compress in a single pass.
    let mut chain = vec![v];
    let mut cur = ancestor[v as usize];
    while ancestor[cur as usize] != u32::MAX {
        chain.push(cur);
        cur = ancestor[cur as usize];
    }
    // Compress.
    for k in (0..chain.len() - 1).rev() {
        let x = chain[k];
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference_graph::ReferenceGraph;
    use ahash::AHashMap;

    /// Builds a synthetic graph from an edge list where node ids 0..n
    /// are the body and node n is the super-root pointing to `roots`.
    fn make_graph(n: usize, edges: &[(u32, u32)], roots: &[u32]) -> ReferenceGraph {
        let super_root = n as u32;
        let total = n + 1;
        let mut all = edges.to_vec();
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
            node_ids: (0..total as u64).collect(),
            node_class: vec![0; total],
            node_shallow: vec![1; total],
            class_ids: vec![0],
            edges_offsets,
            edges_targets,
            super_root,
            index_by_object_id: AHashMap::new(),
        }
    }

    #[test]
    fn linear_chain() {
        // super_root → 0 → 1 → 2 → 3
        let g = make_graph(4, &[(0, 1), (1, 2), (2, 3)], &[0]);
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
        let g = make_graph(4, &[(0, 1), (0, 2), (1, 3), (2, 3)], &[0]);
        let idom = lengauer_tarjan(&g);
        assert_eq!(idom[0], g.super_root);
        assert_eq!(idom[1], 0);
        assert_eq!(idom[2], 0);
        assert_eq!(idom[3], 0);
    }

    #[test]
    fn unreachable_node_marked_max() {
        // 0 is reachable; 1 has no incoming edge.
        let g = make_graph(2, &[], &[0]);
        let idom = lengauer_tarjan(&g);
        assert_eq!(idom[0], g.super_root);
        assert_eq!(idom[1], u32::MAX);
    }

    #[test]
    fn paper_fig2_thirteen_nodes() {
        // Lengauer–Tarjan paper Figure 2.
        // 0=R, 1=A, 2=B, 3=C, 4=D, 5=E, 6=F, 7=G, 8=H, 9=I, 10=J, 11=K, 12=L
        let edges = &[
            (0, 1),
            (0, 2),
            (0, 3),
            (1, 4),
            (2, 1),
            (2, 4),
            (2, 5),
            (3, 6),
            (3, 7),
            (4, 12),
            (5, 8),
            (6, 9),
            (7, 9),
            (7, 10),
            (8, 5),
            (8, 11),
            (9, 11),
            (10, 9),
            (11, 9),
            (11, 0),
            (12, 9),
        ];
        let g = make_graph(13, edges, &[0]);
        let idom = lengauer_tarjan(&g);
        // Expected immediate dominators per the paper:
        //   idom(R)=⊥, idom(A..L) below.
        assert_eq!(idom[0], g.super_root); // R
        assert_eq!(idom[1], 0); // A → R
        assert_eq!(idom[2], 0); // B → R
        assert_eq!(idom[3], 0); // C → R
        assert_eq!(idom[4], 0); // D → R
        // E's only path from R is R→B→E (H→E is a back edge from a
        // descendant of E, so doesn't form an alternative dominator path).
        assert_eq!(idom[5], 2); // E → B
        assert_eq!(idom[6], 3); // F → C
        assert_eq!(idom[7], 3); // G → C
        assert_eq!(idom[8], 5); // H → E
        assert_eq!(idom[9], 0); // I → R
        assert_eq!(idom[10], 7); // J → G
        assert_eq!(idom[11], 0); // K → R
        assert_eq!(idom[12], 4); // L → D
    }
}
