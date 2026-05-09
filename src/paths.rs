//! `--paths-from-id` — walk holders from a single object id toward a GC root.
//!
//! Each iteration scans the dump in retain-bodies mode looking for any record
//! whose body / element list contains the current id. The first holder found
//! is appended to the chain and becomes the new target. Loop stops when:
//!   * `current_id` is in `gc_root_ids` → success (terminated_at_root = true)
//!   * `--max-depth` is reached         → bail (max_depth_reached = true)
//!   * no holder is found               → bail (orphan; chain ends short)
//!
//! Wall cost: O(depth × file_size).

use serde::Serialize;

use crate::args::Mode;
use crate::errors::HprofSlurpError;
use crate::parser::gc_record::{FieldType, GcRecord};
use crate::parser::record::Record;
use crate::referrer::{Pass1Index, field_byte_size, flatten_fields, pass1_index, read_id};
use crate::slurp::parse_records;

#[derive(Serialize, Debug, Clone)]
pub struct PathStep {
    /// Object id of the holder (the object that points at the current id).
    pub holder_object_id: u64,
    pub holder_class: String,
    /// Field name when the holder is an instance, `None` when it is an Object[].
    pub via_field: Option<String>,
    /// The id we were tracing on this hop.
    pub held_object_id: u64,
}

#[derive(Serialize, Debug)]
pub struct PathResult {
    pub start_object_id: u64,
    pub steps: Vec<PathStep>,
    pub terminated_at_root: bool,
    pub root_kind: Option<&'static str>,
    pub max_depth_reached: bool,
    pub depth: u8,
}

pub fn run(mode: &Mode) -> Result<PathResult, HprofSlurpError> {
    let (input_file, start_object_id, max_depth, debug) = match mode {
        Mode::Paths {
            input_file,
            object_id,
            max_depth,
            debug,
            ..
        } => (input_file.as_str(), *object_id, *max_depth, *debug),
        _ => {
            return Err(HprofSlurpError::NotYetImplemented {
                what: "paths::run only handles Mode::Paths",
            });
        }
    };

    let idx = pass1_index(input_file, debug)?;

    let mut steps: Vec<PathStep> = Vec::new();
    let mut current = start_object_id;
    let mut depth: u8 = 0;
    let mut max_depth_reached = false;
    let mut terminated_at_root = false;
    let mut root_kind: Option<&'static str> = None;

    loop {
        if let Some(kind) = idx.gc_root_kind_by_id.get(&current).copied() {
            terminated_at_root = true;
            root_kind = Some(kind);
            break;
        }
        if depth >= max_depth {
            max_depth_reached = true;
            break;
        }
        match find_first_holder(input_file, &idx, current, debug)? {
            Some(step) => {
                let next = step.holder_object_id;
                steps.push(step);
                if next == current {
                    // self-cycle (shouldn't happen but bail rather than loop)
                    break;
                }
                current = next;
                depth += 1;
            }
            None => break,
        }
    }

    Ok(PathResult {
        start_object_id,
        steps,
        terminated_at_root,
        root_kind,
        max_depth_reached,
        depth,
    })
}

/// One streaming pass: scan every InstanceDump body and ObjectArrayDump
/// element list for a reference to `target`. Returns the first hit (file
/// order). `None` if no holder exists in the dump (orphan / unreachable).
fn find_first_holder(
    path: &str,
    idx: &Pass1Index,
    target: u64,
    debug: bool,
) -> Result<Option<PathStep>, HprofSlurpError> {
    use std::cell::RefCell;
    let id_size = idx.id_size as usize;
    let found: RefCell<Option<PathStep>> = RefCell::new(None);

    parse_records(path, debug, true, |rec| {
        if found.borrow().is_some() {
            return; // already found; ignore the rest
        }
        if let Record::GcSegment(gc) = rec {
            match gc {
                GcRecord::InstanceDump {
                    object_id,
                    class_object_id,
                    body: Some(body),
                    ..
                } => {
                    let layout = flatten_fields(idx, class_object_id);
                    let mut input: &[u8] = &body;
                    for fi in layout {
                        let consume = field_byte_size(fi.field_type, id_size);
                        if input.len() < consume {
                            break;
                        }
                        if fi.field_type == FieldType::Object {
                            let rid = read_id(&input[..id_size], id_size);
                            if rid == target {
                                let holder_class = idx
                                    .class_name(class_object_id)
                                    .unwrap_or_else(|| format!("(class_id={class_object_id})"));
                                let via_field = idx
                                    .utf8_by_id
                                    .get(&fi.name_id)
                                    .map(|b| b.as_ref().to_string());
                                *found.borrow_mut() = Some(PathStep {
                                    holder_object_id: object_id,
                                    holder_class,
                                    via_field,
                                    held_object_id: target,
                                });
                                return;
                            }
                        }
                        input = &input[consume..];
                    }
                }
                GcRecord::ObjectArrayDump {
                    object_id,
                    array_class_id,
                    elements: Some(elems),
                    ..
                } => {
                    if elems.iter().any(|&rid| rid == target) {
                        let holder_class = idx
                            .class_name(array_class_id)
                            .unwrap_or_else(|| format!("(class_id={array_class_id})"));
                        *found.borrow_mut() = Some(PathStep {
                            holder_object_id: object_id,
                            holder_class,
                            via_field: None,
                            held_object_id: target,
                        });
                    }
                }
                _ => {}
            }
        }
    })?;
    Ok(found.into_inner())
}

pub fn render_text(r: &PathResult) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\nPath from object_id={} (depth {} step(s)):",
        r.start_object_id, r.depth
    );
    let _ = writeln!(out, "  start  ── id={}", r.start_object_id);
    for (i, s) in r.steps.iter().enumerate() {
        let arrow = match &s.via_field {
            Some(f) => format!("via {}.{}", s.holder_class, f),
            None => format!("via {}[]", s.holder_class),
        };
        let _ = writeln!(
            out,
            "  hop{:>2} ── id={}  ({arrow})",
            i + 1,
            s.holder_object_id,
        );
    }
    if r.terminated_at_root {
        let _ = writeln!(
            out,
            "  → reached GC root: {}",
            r.root_kind.unwrap_or("(unknown)")
        );
    } else if r.max_depth_reached {
        let _ = writeln!(out, "  → stopped at --max-depth (chain may continue)");
    } else {
        let _ = writeln!(out, "  → orphan: no holder found in dump");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_args(object_id: u64, max_depth: u8) -> Mode {
        Mode::Paths {
            input_file: "test-heap-dumps/hprof-64.bin".to_string(),
            object_id,
            max_depth,
            debug: false,
            json: false,
        }
    }

    #[test]
    fn paths_for_nonexistent_id_orphan() {
        let r = run(&fixture_args(999_999_999_999, 4)).unwrap();
        assert!(r.steps.is_empty());
        assert!(!r.terminated_at_root);
        assert!(!r.max_depth_reached);
    }

    #[test]
    fn paths_for_a_known_object_reaches_a_root() {
        // pick a real object id from the fixture: a LinkedList$Node.
        // We can't hard-code one because ids vary; instead, find any
        // LinkedList$Node id via the index, then walk it.
        let idx = pass1_index("test-heap-dumps/hprof-64.bin", false).unwrap();
        // grab the class id of LinkedList$Node
        let mut class_id_of_node: Option<u64> = None;
        for (cid, nid) in &idx.class_name_id_by_class_id {
            if let Some(n) = idx.utf8_by_id.get(nid) {
                if n.as_ref().replace('/', ".") == "java.util.LinkedList$Node" {
                    class_id_of_node = Some(*cid);
                    break;
                }
            }
        }
        let class_id_of_node = class_id_of_node.expect("LinkedList$Node class id");

        // find an instance id of that class
        let mut node_id: Option<u64> = None;
        parse_records("test-heap-dumps/hprof-64.bin", false, false, |rec| {
            if node_id.is_some() {
                return;
            }
            if let Record::GcSegment(GcRecord::InstanceDump {
                object_id,
                class_object_id,
                ..
            }) = rec
            {
                if class_object_id == class_id_of_node {
                    node_id = Some(object_id);
                }
            }
        })
        .unwrap();
        let node_id = node_id.expect("at least one LinkedList$Node instance");

        let r = run(&fixture_args(node_id, 16)).unwrap();
        // Either we reach a root, or we give up at max-depth — but we should
        // have at least one step (some other object holds a Node).
        assert!(
            !r.steps.is_empty(),
            "expected a non-empty holder chain for node id {node_id}"
        );
    }
}
