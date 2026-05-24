// Dead-code allowance until the orchestrating dispatcher in main.rs
// + smoke runs exercise everything.
#![allow(dead_code)]

//! Leak Suspects (v1.1.0 feature H).
//!
//! Auto-ranks dominators by retained share, clusters each suspect's
//! dominated subtree by class, and emits a narrative + path-to-root +
//! content-preview report per suspect. The output format is heaptrail's
//! daily Android leak-hunting answer to MAT's "Leak Suspects" report.

use serde::Serialize;

use crate::args::Mode;
use crate::errors::HprofSlurpError;
use crate::reference_graph::ReferenceGraph;
use crate::referrer::Pass1Index;

#[derive(Serialize, Debug)]
pub struct Suspect {
    pub dominator_id: u64,
    pub dominator_class: String,
    pub retained_bytes: u64,
    pub heap_share_pct: f32,
    pub accumulating_class: String,
    pub accumulating_count: u32,
    pub accumulating_total_bytes: u64,
    pub path_to_root: crate::paths::PathResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_content_label: Option<crate::preview::ContentLabel>,
    /// True when the suspect's retained share is below the threshold
    /// but it appears in the top-3 fallback.
    pub below_threshold: bool,
}

#[derive(Serialize, Debug)]
pub struct SuspectsReport {
    pub total_heap_bytes: u64,
    pub retained_reachable_bytes: u64,
    pub threshold_pct: f32,
    pub suspects: Vec<Suspect>,
}

pub fn run(mode: &Mode) -> Result<SuspectsReport, HprofSlurpError> {
    let (input_file, top, threshold, exclude_soft_weak, preview_bytes, debug) = match mode {
        Mode::LeakSuspects {
            input_file,
            top,
            threshold,
            exclude_soft_weak,
            preview_bytes,
            debug,
            ..
        } => (
            input_file.as_str(),
            *top,
            *threshold,
            *exclude_soft_weak,
            *preview_bytes,
            *debug,
        ),
        _ => {
            return Err(HprofSlurpError::NotYetImplemented {
                what: "leak_suspects::run only handles Mode::LeakSuspects",
            });
        }
    };

    let idx = crate::referrer::pass1_index(input_file, debug)?;
    let graph = crate::reference_graph::build_from_pass1_with(
        input_file,
        &idx,
        debug,
        crate::reference_graph::BuildOptions { exclude_soft_weak },
    )?;
    let idom = crate::dominators::lengauer_tarjan(&graph);
    let analysis = crate::retained::compute(&graph, &idom, 0);
    let dom_children = crate::retained::dom_children(&idom);

    let total_heap_bytes: u64 = graph.node_shallow.iter().map(|&s| s as u64).sum();
    let retained_reachable_bytes = analysis.retained[graph.super_root as usize];

    // Rank dominators by retained share (skip super_root).
    let mut ranked: Vec<(u32, u64)> = (0..graph.node_count() as u32)
        .filter(|&i| i != graph.super_root)
        .map(|i| (i, analysis.retained[i as usize]))
        .collect();
    ranked.sort_unstable_by_key(|&(_, r)| std::cmp::Reverse(r));

    let cutoff = (retained_reachable_bytes as f64 * threshold as f64) as u64;
    let above_threshold: Vec<&(u32, u64)> = ranked
        .iter()
        .filter(|&&(_, r)| r >= cutoff)
        .take(top)
        .collect();
    let final_set: Vec<(u32, u64, bool)> = if above_threshold.is_empty() {
        ranked.iter().take(3).map(|&(i, r)| (i, r, true)).collect()
    } else {
        above_threshold
            .iter()
            .map(|&&(i, r)| (i, r, false))
            .collect()
    };

    let array_previews: ahash::AHashMap<u64, crate::result_recorder::ArrayPreview> =
        if preview_bytes > 0 {
            crate::paths::collect_primitive_array_previews(input_file, debug, preview_bytes)?
        } else {
            ahash::AHashMap::new()
        };

    let mut suspects = Vec::with_capacity(final_set.len());
    for (node_idx, retained, below) in final_set {
        let dom_oid = graph.node_ids[node_idx as usize];
        let dom_ci = graph.node_class[node_idx as usize];
        let dom_class_name = if dom_ci == u32::MAX {
            "(super-root)".to_string()
        } else {
            crate::referrer::class_label_for_id(&idx, graph.class_ids[dom_ci as usize])
        };

        let (accum_class, accum_count, accum_bytes) =
            cluster_by_class(&graph, &idx, &dom_children, node_idx);

        // Resolve path-to-root via paths::run with a synthetic Mode.
        let path_mode = Mode::Paths {
            input_file: input_file.to_string(),
            object_id: dom_oid,
            max_depth: 12,
            debug,
            json: false,
            json_out: None,
            preview_bytes: 0,
            retained_size: false,
            exclude_soft_weak,
            merge_paths: false,
            mapping: crate::args::MappingOptions::default(),
        };
        let path_to_root = crate::paths::run(&path_mode)?;

        let (preview_snippet, preview_content_label) = array_previews
            .get(&dom_oid)
            .map(|p| {
                use crate::preview::{PreviewKind, render_preview};
                let kind = render_preview(&p.bytes, p.element_type, p.total_bytes as usize);
                let label = kind.content_label();
                let snippet = match kind {
                    PreviewKind::Text { snippet, .. } => {
                        snippet.chars().take(120).collect::<String>()
                    }
                    PreviewKind::Hex { total_bytes, .. } => format!("binary, {total_bytes} bytes"),
                };
                (Some(snippet), Some(label))
            })
            .unwrap_or((None, None));

        let heap_share_pct = if retained_reachable_bytes == 0 {
            0.0
        } else {
            (retained as f64 / retained_reachable_bytes as f64) as f32 * 100.0
        };

        suspects.push(Suspect {
            dominator_id: dom_oid,
            dominator_class: dom_class_name,
            retained_bytes: retained,
            heap_share_pct,
            accumulating_class: accum_class,
            accumulating_count: accum_count,
            accumulating_total_bytes: accum_bytes,
            path_to_root,
            preview_snippet,
            preview_content_label,
            below_threshold: below,
        });
    }

    Ok(SuspectsReport {
        total_heap_bytes,
        retained_reachable_bytes,
        threshold_pct: threshold * 100.0,
        suspects,
    })
}

fn cluster_by_class(
    graph: &ReferenceGraph,
    idx: &Pass1Index,
    dom_children: &[Vec<u32>],
    root_idx: u32,
) -> (String, u32, u64) {
    let mut counts: ahash::AHashMap<u32, (u32, u64)> = ahash::AHashMap::new();
    let mut stack = vec![root_idx];
    while let Some(v) = stack.pop() {
        let ci = graph.node_class[v as usize];
        if ci != u32::MAX {
            let entry = counts.entry(ci).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += graph.node_shallow[v as usize] as u64;
        }
        for &c in &dom_children[v as usize] {
            stack.push(c);
        }
    }
    let best = counts
        .iter()
        .max_by_key(|(_, (c, _))| *c)
        .map(|(&k, &(c, b))| (k, c, b));
    match best {
        Some((ci, count, bytes)) => {
            let class_name = crate::referrer::class_label_for_id(idx, graph.class_ids[ci as usize]);
            (class_name, count, bytes)
        }
        None => ("(none)".to_string(), 0, 0),
    }
}

pub fn render_text(r: &SuspectsReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Heap: {} total, {} retained-reachable.",
        crate::utils::pretty_bytes_size(r.total_heap_bytes),
        crate::utils::pretty_bytes_size(r.retained_reachable_bytes),
    );
    let above = r.suspects.iter().filter(|s| !s.below_threshold).count();
    let _ = writeln!(
        out,
        "Threshold: {:.1} % retained share. Showing {} suspect(s) ({} above threshold).",
        r.threshold_pct,
        r.suspects.len(),
        above,
    );

    for (i, s) in r.suspects.iter().enumerate() {
        let _ = writeln!(out);
        let banner = if s.below_threshold {
            format!(
                "Suspect {} — {} ({:.1} % of heap, below threshold)",
                i + 1,
                crate::utils::pretty_bytes_size(s.retained_bytes),
                s.heap_share_pct,
            )
        } else {
            format!(
                "Suspect {} — {} ({:.1} % of heap)",
                i + 1,
                crate::utils::pretty_bytes_size(s.retained_bytes),
                s.heap_share_pct,
            )
        };
        let _ = writeln!(out, "{banner}");
        let _ = writeln!(
            out,
            "  dominator: {} (object_id={})",
            s.dominator_class, s.dominator_id
        );
        let _ = writeln!(
            out,
            "  accumulating: {} instances of {}, total {}",
            s.accumulating_count,
            s.accumulating_class,
            crate::utils::pretty_bytes_size(s.accumulating_total_bytes),
        );
        if let Some(preview) = &s.preview_snippet {
            if let Some(label) = s.preview_content_label {
                let _ = writeln!(out, "  preview: content: {}, {preview}", label.display());
            } else {
                let _ = writeln!(out, "  preview: {preview}");
            }
        }
        let _ = writeln!(out, "  path to GC root:");
        let path = crate::paths::render_text(&s.path_to_root);
        for line in path.lines() {
            let _ = writeln!(out, "  {line}");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_path_result() -> crate::paths::PathResult {
        crate::paths::PathResult {
            start_object_id: 42,
            steps: Vec::new(),
            terminated_at_root: false,
            root_kind: None,
            root_thread_name: None,
            root_frame: None,
            max_depth_reached: false,
            depth: 0,
            array_previews: ahash::AHashMap::new(),
            retained_by_oid: None,
            terminated_by_soft_weak: None,
        }
    }

    #[test]
    fn serializes_preview_content_label() {
        let report = SuspectsReport {
            total_heap_bytes: 100,
            retained_reachable_bytes: 100,
            threshold_pct: 5.0,
            suspects: vec![Suspect {
                dominator_id: 42,
                dominator_class: "byte[]".to_string(),
                retained_bytes: 80,
                heap_share_pct: 80.0,
                accumulating_class: "byte[]".to_string(),
                accumulating_count: 1,
                accumulating_total_bytes: 80,
                path_to_root: empty_path_result(),
                preview_snippet: Some("{\"ok\":true}".to_string()),
                preview_content_label: Some(crate::preview::ContentLabel::Json),
                below_threshold: false,
            }],
        };

        let value = serde_json::to_value(report).unwrap();

        assert_eq!(value["suspects"][0]["preview_content_label"], "json");
    }
}
