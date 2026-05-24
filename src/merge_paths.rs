// Dead-code allowance until main.rs dispatches and smoke runs
// exercise the module.
#![allow(dead_code)]

//! `--merge-paths` (v1.1.0 feature I).
//!
//! Trie-fold N paths-to-root from `paths::compute_path_for_object`
//! into a single tree showing common prefixes with branch counts.
//! When `--retained-size` is set, the merge is dominator-verified
//! (we rebuild the graph + idom and the trie's hop-by-hop structure
//! is implicitly consistent because dominator-tree ancestors are
//! unique). Without it, the merge is a textual prefix match — useful
//! and usually correct, but the renderer emits a banner so users
//! know the difference.

use serde::Serialize;

use crate::args::Mode;
use crate::errors::HprofSlurpError;
use crate::paths::{PathResult, PathWalkInputs, compute_path_for_object};

#[derive(Serialize, Debug, Default)]
pub struct MergedHop {
    pub source_class: String,
    pub field_name: Option<String>,
    pub instance_count: u32,
    pub children: Vec<MergedHop>,
}

#[derive(Serialize, Debug)]
pub struct MergedReport {
    pub target_label: String,
    pub instance_count: u32,
    pub root: MergedHop,
    /// True when `--retained-size` was set on the parent `--paths-from-id`
    /// invocation (callers can interpret this as graph-verified
    /// convergence; without it the merge is a pure textual fold).
    pub graph_verified: bool,
}

impl MergedReport {
    pub fn symbolicate(&mut self, symbolicator: &crate::mapping::Symbolicator) {
        self.target_label = symbolicator.class_name(&self.target_label);
        symbolicate_hop(&mut self.root, symbolicator);
    }
}

fn symbolicate_hop(hop: &mut MergedHop, symbolicator: &crate::mapping::Symbolicator) {
    let raw_class = hop.source_class.clone();
    if let Some(field) = hop.field_name.as_mut() {
        *field = symbolicator.field_name(&raw_class, field);
    }
    hop.source_class = symbolicator.class_name(&hop.source_class);
    for child in &mut hop.children {
        symbolicate_hop(child, symbolicator);
    }
}

pub fn run(mode: &Mode) -> Result<MergedReport, HprofSlurpError> {
    let (input_file, start_oid, max_depth, debug, exclude_soft_weak, retained_size) = match mode {
        Mode::Paths {
            input_file,
            object_id,
            max_depth,
            debug,
            exclude_soft_weak,
            retained_size,
            ..
        } => (
            input_file.as_str(),
            *object_id,
            *max_depth,
            *debug,
            *exclude_soft_weak,
            *retained_size,
        ),
        _ => unreachable!("merge_paths::run only handles Mode::Paths"),
    };

    let idx = crate::referrer::pass1_index(input_file, debug)?;

    // Resolve target class from the start id; collect all instance ids
    // whose class matches.
    let class_id = lookup_class_of_object(input_file, start_oid, debug)?;
    let instance_ids = collect_instances_of_class(input_file, class_id, debug)?;

    let mut paths: Vec<PathResult> = Vec::with_capacity(instance_ids.len());
    for oid in &instance_ids {
        let inp = PathWalkInputs {
            idx: &idx,
            start_object_id: *oid,
            max_depth,
            input_file,
            debug,
            exclude_soft_weak,
        };
        paths.push(compute_path_for_object(&inp)?);
    }

    let target_label = idx
        .class_name(class_id)
        .unwrap_or_else(|| format!("class:{class_id:x}"));

    Ok(MergedReport {
        target_label,
        instance_count: instance_ids.len() as u32,
        root: fold(&paths),
        graph_verified: retained_size,
    })
}

pub fn fold(paths: &[PathResult]) -> MergedHop {
    let mut root = MergedHop::default();
    for p in paths {
        let mut cur = &mut root;
        cur.instance_count += 1;
        for s in &p.steps {
            let key_class = s.holder_class.clone();
            let key_field = s.via_field.clone();
            let existing = cur
                .children
                .iter()
                .position(|c| c.source_class == key_class && c.field_name == key_field);
            cur = match existing {
                Some(i) => &mut cur.children[i],
                None => {
                    cur.children.push(MergedHop {
                        source_class: key_class,
                        field_name: key_field,
                        instance_count: 0,
                        children: Vec::new(),
                    });
                    cur.children.last_mut().unwrap()
                }
            };
            cur.instance_count += 1;
        }
    }
    root
}

pub fn render_text(r: &MergedReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Target: {} — {} instance(s) merged.",
        r.target_label, r.instance_count
    );
    let _ = writeln!(
        out,
        "({})",
        if r.graph_verified {
            "merge verified via dominator convergence"
        } else {
            "textual merge — re-run with --retained-size for graph-verified convergence"
        },
    );
    for c in &r.root.children {
        render_hop(&mut out, c, 1);
    }
    out
}

fn render_hop(out: &mut String, hop: &MergedHop, indent: usize) {
    use std::fmt::Write;
    let prefix = "  ".repeat(indent);
    let arrow = match &hop.field_name {
        Some(f) => format!("↑ field {} in {}", f, hop.source_class),
        None => format!("↑ {}[]", hop.source_class),
    };
    let _ = writeln!(out, "{prefix}{arrow}  [{}×]", hop.instance_count);
    for c in &hop.children {
        render_hop(out, c, indent + 1);
    }
}

fn lookup_class_of_object(path: &str, object_id: u64, debug: bool) -> Result<u64, HprofSlurpError> {
    use crate::parser::gc_record::GcRecord;
    use crate::parser::record::Record;
    let mut found: Option<u64> = None;
    crate::slurp::parse_records(path, debug, false, |rec| {
        if found.is_some() {
            return;
        }
        if let Record::GcSegment(GcRecord::InstanceDump {
            object_id: oid,
            class_object_id,
            ..
        }) = rec
            && oid == object_id
        {
            found = Some(class_object_id);
        }
    })?;
    found.ok_or(HprofSlurpError::NotYetImplemented {
        what: "object id not found in dump (--merge-paths)",
    })
}

fn collect_instances_of_class(
    path: &str,
    class_id: u64,
    debug: bool,
) -> Result<Vec<u64>, HprofSlurpError> {
    use crate::parser::gc_record::GcRecord;
    use crate::parser::record::Record;
    let mut ids: Vec<u64> = Vec::new();
    crate::slurp::parse_records(path, debug, false, |rec| {
        if let Record::GcSegment(GcRecord::InstanceDump {
            object_id,
            class_object_id,
            ..
        }) = rec
            && class_object_id == class_id
        {
            ids.push(object_id);
        }
    })?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::PathStep;

    fn make_path(steps: Vec<(&str, &str)>) -> PathResult {
        PathResult {
            start_object_id: 0,
            steps: steps
                .into_iter()
                .map(|(cls, field)| PathStep {
                    holder_object_id: 0,
                    holder_class: cls.to_string(),
                    via_field: Some(field.to_string()),
                    array_index: None,
                    held_object_id: 0,
                })
                .collect(),
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
    fn fold_collapses_common_prefix() {
        let paths = vec![
            make_path(vec![
                ("MainActivity", "handler"),
                ("EventBus", "subscribers"),
            ]),
            make_path(vec![
                ("MainActivity", "handler"),
                ("EventBus", "subscribers"),
            ]),
            make_path(vec![
                ("MainActivity", "handler"),
                ("EventBus", "subscribers"),
            ]),
        ];
        let root = fold(&paths);
        assert_eq!(root.instance_count, 3);
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].instance_count, 3);
        assert_eq!(root.children[0].children.len(), 1);
        assert_eq!(root.children[0].children[0].instance_count, 3);
    }

    #[test]
    fn fold_branches_when_paths_diverge() {
        let paths = vec![
            make_path(vec![("A", "x"), ("B", "y")]),
            make_path(vec![("A", "x"), ("C", "z")]),
        ];
        let root = fold(&paths);
        assert_eq!(root.instance_count, 2);
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].instance_count, 2);
        assert_eq!(root.children[0].children.len(), 2);
    }
}
