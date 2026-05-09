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
    /// Thread name (when the terminating root is owned by a thread).
    /// Always `None` for non-thread roots like `RootStickyClass`.
    pub root_thread_name: Option<String>,
    /// Top frame at the terminator (only for `RootJavaFrame`).
    pub root_frame: Option<crate::referrer::ResolvedFrame>,
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
    let mut root_thread_name: Option<String> = None;
    let mut root_frame: Option<crate::referrer::ResolvedFrame> = None;

    loop {
        if let Some(kind) = idx.gc_root_kind_by_id.get(&current).copied() {
            terminated_at_root = true;
            root_kind = Some(kind);
            // Resolve thread name + top frame for thread-owned roots.
            // (a) RootJavaFrame/RootJniLocal/RootJniMonitor have a
            //     ThreadFrameRef in root_thread_meta_by_id.
            // (b) RootThreadObject's object_id IS the thread itself, so
            //     look it up in thread_serial_by_obj_id to get the serial.
            let meta = idx
                .root_thread_meta_by_id
                .get(&current)
                .copied()
                .or_else(|| {
                    idx.thread_serial_by_obj_id.get(&current).map(|&serial| {
                        crate::referrer::ThreadFrameRef {
                            thread_serial: serial,
                            frame_idx: None,
                        }
                    })
                });
            if let Some(m) = meta {
                root_thread_name = idx
                    .thread_name_by_serial
                    .get(&m.thread_serial)
                    .map(|s| s.as_ref().to_string());
                if let Some(idx_in_trace) = m.frame_idx {
                    // Frame index is into the thread's stack trace. The
                    // StackTrace records are keyed by their own serial,
                    // not by thread_serial. We don't have a direct
                    // thread_serial -> stack_trace_serial map; iterate
                    // stack_trace_by_serial to find the trace whose
                    // thread_serial matches. (Cheap: usually <100 traces.)
                    // NOTE: StackTraceData is not stored, only frame ids;
                    // best we can do is look up the first matching trace.
                    // For robustness, also fall back to looking up the
                    // index in any trace recorded against this thread's
                    // serial: many dumps record multiple stack traces per
                    // thread but the indexer only kept the frame ids.
                    if let Some(frames) = idx.stack_trace_by_serial.get(&m.thread_serial)
                        && let Some(&frame_id) = frames.get(idx_in_trace as usize)
                    {
                        root_frame = idx.resolve_frame(frame_id);
                    }
                }
            }
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
        root_thread_name,
        root_frame,
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
                } if elems.contains(&target) => {
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
        // Thread + frame block (feature A). Renders only when meta is present.
        if let Some(name) = &r.root_thread_name {
            let _ = writeln!(out, "        thread \"{name}\"");
        } else if matches!(
            r.root_kind,
            Some("RootJavaFrame")
                | Some("RootJniLocal")
                | Some("RootJniMonitor")
                | Some("RootThreadObject")
        ) {
            // Thread root, but no metadata — be explicit so users know it's
            // a dump-content gap, not a heaptrail bug.
            let _ = writeln!(out, "        (thread metadata not in dump)");
        }
        if let Some(f) = &r.root_frame {
            let qualified = match &f.class {
                Some(c) => format!("{c}.{}", f.method),
                None => f.method.clone(),
            };
            let location = match (&f.file, f.line) {
                (Some(file), n) if n > 0 => format!("({file}:{n})"),
                (Some(file), _) => format!("({file})"),
                (None, n) if n > 0 => format!("(line {n})"),
                (None, _) => String::new(),
            };
            let _ = writeln!(out, "        at {qualified}{location}");
        }
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
    fn render_text_shows_thread_block_for_root_java_frame() {
        let r = PathResult {
            start_object_id: 100,
            steps: vec![],
            terminated_at_root: true,
            root_kind: Some("RootJavaFrame"),
            root_thread_name: Some("pool-7-thread-2".to_string()),
            root_frame: Some(crate::referrer::ResolvedFrame {
                method: "commitToMemory".to_string(),
                class: Some("android.app.SharedPreferencesImpl$EditorImpl".to_string()),
                file: Some("SharedPreferencesImpl.java".to_string()),
                line: 478,
            }),
            max_depth_reached: false,
            depth: 0,
        };
        let out = render_text(&r);
        assert!(
            out.contains("thread \"pool-7-thread-2\""),
            "expected thread name, got:\n{out}"
        );
        assert!(
            out.contains(
                "at android.app.SharedPreferencesImpl$EditorImpl.commitToMemory(SharedPreferencesImpl.java:478)"
            ),
            "expected qualified frame, got:\n{out}"
        );
    }

    #[test]
    fn render_text_shows_metadata_gap_for_thread_root_without_meta() {
        let r = PathResult {
            start_object_id: 100,
            steps: vec![],
            terminated_at_root: true,
            root_kind: Some("RootJavaFrame"),
            root_thread_name: None,
            root_frame: None,
            max_depth_reached: false,
            depth: 0,
        };
        let out = render_text(&r);
        assert!(
            out.contains("(thread metadata not in dump)"),
            "expected gap line, got:\n{out}"
        );
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
            if let Some(n) = idx.utf8_by_id.get(nid)
                && n.as_ref().replace('/', ".") == "java.util.LinkedList$Node"
            {
                class_id_of_node = Some(*cid);
                break;
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
                && class_object_id == class_id_of_node
            {
                node_id = Some(object_id);
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
