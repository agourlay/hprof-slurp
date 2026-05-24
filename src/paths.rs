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
    /// Element slot when the holder is an `Object[]`; `None` for
    /// instance-field hops. Always `Some(_)` when `via_field` is `None`.
    pub array_index: Option<u32>,
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
    /// Preview bodies (when --preview-bytes > 0) keyed by object_id.
    /// Used by `render_text` to display content under primitive-array
    /// hops or the start id. Skipped from JSON since the truncated
    /// blob isn't useful structured.
    #[serde(skip)]
    pub array_previews: ahash::AHashMap<u64, crate::result_recorder::ArrayPreview>,
    /// Per-object retained bytes from the dominator tree (when
    /// `--retained-size` was set). `None` otherwise. Skipped from JSON
    /// because each value is per-instance (the JSON consumer should
    /// run summary's --retained-size for class-level rollups).
    #[serde(skip)]
    pub retained_by_oid: Option<ahash::AHashMap<u64, u64>>,
    /// v1.1.0 (--exclude-soft-weak): set when the walk terminated
    /// because the only candidate holder was a Reference subclass and
    /// the user asked to exclude weak holders. Surfaced as a
    /// `[soft/weak/phantom — excluded]` annotation on the orphan line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminated_by_soft_weak: Option<()>,
}

/// Inputs to the per-instance path walker. `idx` is borrowed; the
/// walker re-streams the dump on every call so callers like
/// `merge_paths` (PR 6) pay file I/O proportional to the target count,
/// not the dump size. Returned `PathResult.array_previews` and
/// `retained_by_oid` are empty/None — the caller fills them after a
/// fan-out to `paths::run` is unnecessary.
pub struct PathWalkInputs<'a> {
    pub idx: &'a Pass1Index,
    pub start_object_id: u64,
    pub max_depth: u8,
    pub input_file: &'a str,
    pub debug: bool,
    pub exclude_soft_weak: bool,
}

/// Walk a single object id toward a GC root. Pure walker — no pass1
/// indexing, no preview collection, no dominator pipeline. Suitable
/// for batch invocation (e.g. `merge_paths` calling once per target
/// instance).
pub fn compute_path_for_object(inp: &PathWalkInputs) -> Result<PathResult, HprofSlurpError> {
    let idx = inp.idx;
    let mut steps: Vec<PathStep> = Vec::new();
    let mut current = inp.start_object_id;
    let mut depth: u8 = 0;
    let mut max_depth_reached = false;
    let mut terminated_at_root = false;
    let mut root_kind: Option<&'static str> = None;
    let mut root_thread_name: Option<String> = None;
    let mut root_frame: Option<crate::referrer::ResolvedFrame> = None;
    let mut terminated_by_soft_weak: Option<()> = None;

    loop {
        if let Some(kind) = idx.gc_root_kind_by_id.get(&current).copied() {
            terminated_at_root = true;
            root_kind = Some(kind);
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
                if let Some(idx_in_trace) = m.frame_idx
                    && let Some(frames) = idx.stack_trace_by_serial.get(&m.thread_serial)
                    && let Some(&frame_id) = frames.get(idx_in_trace as usize)
                {
                    root_frame = idx.resolve_frame(frame_id);
                }
            }
            break;
        }
        if depth >= inp.max_depth {
            max_depth_reached = true;
            break;
        }
        match find_first_holder(
            inp.input_file,
            idx,
            current,
            inp.debug,
            inp.exclude_soft_weak,
        )? {
            HolderResult::Holder(step) => {
                let next = step.holder_object_id;
                steps.push(step);
                if next == current {
                    break;
                }
                current = next;
                depth += 1;
            }
            HolderResult::None => break,
            HolderResult::WeakRejected => {
                terminated_by_soft_weak = Some(());
                break;
            }
        }
    }

    Ok(PathResult {
        start_object_id: inp.start_object_id,
        steps,
        terminated_at_root,
        root_kind,
        root_thread_name,
        root_frame,
        max_depth_reached,
        depth,
        array_previews: ahash::AHashMap::new(),
        retained_by_oid: None,
        terminated_by_soft_weak,
    })
}

pub fn run(mode: &Mode) -> Result<PathResult, HprofSlurpError> {
    let (
        input_file,
        start_object_id,
        max_depth,
        debug,
        preview_bytes,
        retained_size,
        exclude_soft_weak,
    ) = match mode {
        Mode::Paths {
            input_file,
            object_id,
            max_depth,
            debug,
            preview_bytes,
            retained_size,
            exclude_soft_weak,
            ..
        } => (
            input_file.as_str(),
            *object_id,
            *max_depth,
            *debug,
            *preview_bytes,
            *retained_size,
            *exclude_soft_weak,
        ),
        _ => {
            return Err(HprofSlurpError::NotYetImplemented {
                what: "paths::run only handles Mode::Paths",
            });
        }
    };

    let idx = pass1_index(input_file, debug)?;
    let array_previews: ahash::AHashMap<u64, crate::result_recorder::ArrayPreview> =
        if preview_bytes > 0 {
            collect_primitive_array_previews(input_file, debug, preview_bytes)?
        } else {
            ahash::AHashMap::new()
        };

    let retained_by_oid: Option<ahash::AHashMap<u64, u64>> = if retained_size {
        let graph = crate::reference_graph::build_from_pass1_with(
            input_file,
            &idx,
            debug,
            crate::reference_graph::BuildOptions { exclude_soft_weak },
        )?;
        let idom = crate::dominators::lengauer_tarjan(&graph);
        let analysis = crate::retained::compute(&graph, &idom, 0);
        let mut map: ahash::AHashMap<u64, u64> = ahash::AHashMap::with_capacity(graph.node_count());
        for i in 0..graph.node_count() {
            if i as u32 == graph.super_root {
                continue;
            }
            map.insert(graph.node_ids[i], analysis.retained[i]);
        }
        Some(map)
    } else {
        None
    };

    let inp = PathWalkInputs {
        idx: &idx,
        start_object_id,
        max_depth,
        input_file,
        debug,
        exclude_soft_weak,
    };
    let mut path = compute_path_for_object(&inp)?;
    path.array_previews = array_previews;
    path.retained_by_oid = retained_by_oid;
    Ok(path)
}

/// Run an extra streaming pass with retain_primitive_bodies=true and
/// `preview_bytes_limit=N` to collect truncated bodies of every
/// primitive array in the dump, keyed by object_id. Used by
/// `--paths-from-id --preview-bytes N` and `--find-referrers id:N
/// --preview-bytes N`.
pub(crate) fn collect_primitive_array_previews(
    path: &str,
    debug: bool,
    preview_bytes: u32,
) -> Result<ahash::AHashMap<u64, crate::result_recorder::ArrayPreview>, HprofSlurpError> {
    use crate::parser::record::Record;
    let mut previews: ahash::AHashMap<u64, crate::result_recorder::ArrayPreview> =
        ahash::AHashMap::new();
    crate::slurp::parse_records_with_modes(path, debug, false, true, preview_bytes, |rec| {
        if let Record::GcSegment(GcRecord::PrimitiveArrayDump {
            object_id,
            number_of_elements,
            element_type,
            body: Some(b),
            ..
        }) = rec
        {
            let elem_size = field_byte_size(element_type, 1);
            let total = u64::from(number_of_elements) * (elem_size as u64);
            previews.insert(
                object_id,
                crate::result_recorder::ArrayPreview {
                    element_type,
                    bytes: b,
                    total_bytes: total,
                },
            );
        }
    })?;
    Ok(previews)
}

/// One streaming pass: scan every InstanceDump body and ObjectArrayDump
/// element list for a reference to `target`. Returns the first hit (file
/// order). `None` if no holder exists in the dump (orphan / unreachable).
/// Outcome of a single-target holder scan. `WeakRejected` means we saw
/// a candidate holder whose class is in the Reference subclass set and
/// `--exclude-soft-weak` is on; the walker treats this as a path
/// terminator with the `[soft/weak/phantom — excluded]` annotation.
pub(crate) enum HolderResult {
    Holder(PathStep),
    None,
    WeakRejected,
}

#[allow(clippy::too_many_arguments)]
fn find_first_holder(
    path: &str,
    idx: &Pass1Index,
    target: u64,
    debug: bool,
    exclude_soft_weak: bool,
) -> Result<HolderResult, HprofSlurpError> {
    use std::cell::RefCell;
    let id_size = idx.id_size as usize;
    let found: RefCell<Option<PathStep>> = RefCell::new(None);
    let weak_rejected: RefCell<bool> = RefCell::new(false);

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
                                if exclude_soft_weak
                                    && idx.reference_subclass_set.contains(&class_object_id)
                                {
                                    *weak_rejected.borrow_mut() = true;
                                    return;
                                }
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
                                    array_index: None,
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
                    let array_index = elems
                        .iter()
                        .position(|&rid| rid == target)
                        .map(|p| p as u32);
                    let holder_class = idx
                        .class_name(array_class_id)
                        .unwrap_or_else(|| format!("(class_id={array_class_id})"));
                    *found.borrow_mut() = Some(PathStep {
                        holder_object_id: object_id,
                        holder_class,
                        via_field: None,
                        array_index,
                        held_object_id: target,
                    });
                }
                _ => {}
            }
        }
    })?;
    Ok(match found.into_inner() {
        Some(s) => HolderResult::Holder(s),
        None if *weak_rejected.borrow() => HolderResult::WeakRejected,
        None => HolderResult::None,
    })
}

pub fn render_text(r: &PathResult) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\nPath from object_id={} (depth {} step(s)):",
        r.start_object_id, r.depth
    );
    let _ = writeln!(
        out,
        "  start  ── id={}{}",
        r.start_object_id,
        retained_suffix(r, r.start_object_id)
    );
    if let Some(preview) = r.array_previews.get(&r.start_object_id) {
        render_preview_block(&mut out, preview);
    }
    for (i, s) in r.steps.iter().enumerate() {
        let arrow = match (&s.via_field, s.array_index) {
            (Some(f), _) => format!("via {}.{}", s.holder_class, f),
            (None, Some(idx)) => format!("via {}[{idx}]", s.holder_class),
            (None, None) => format!("via {}[]", s.holder_class),
        };
        let _ = writeln!(
            out,
            "  hop{:>2} ── id={}{}  ({arrow})",
            i + 1,
            s.holder_object_id,
            retained_suffix(r, s.holder_object_id),
        );
        if let Some(preview) = r.array_previews.get(&s.holder_object_id) {
            render_preview_block(&mut out, preview);
        }
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
    } else if r.terminated_by_soft_weak.is_some() {
        let _ = writeln!(
            out,
            "  → orphan [soft/weak/phantom — excluded]; re-run without --exclude-soft-weak to see the weak chain"
        );
    } else {
        let _ = writeln!(out, "  → orphan: no holder found in dump");
    }
    out
}

/// Format the trailing `(retained=<size>)` annotation for a hop's
/// object id. Returns an empty string when `--retained-size` was off
/// or when the id isn't in the retained map (unreachable nodes).
fn retained_suffix(r: &PathResult, object_id: u64) -> String {
    let Some(map) = r.retained_by_oid.as_ref() else {
        return String::new();
    };
    let Some(&bytes) = map.get(&object_id) else {
        return String::new();
    };
    format!(" (retained={})", crate::utils::pretty_bytes_size(bytes))
}

fn render_preview_block(out: &mut String, preview: &crate::result_recorder::ArrayPreview) {
    use crate::preview::{render_preview, render_short_preview};
    use std::fmt::Write;
    let kind = render_preview(
        &preview.bytes,
        preview.element_type,
        preview.total_bytes as usize,
    );
    let rendered = render_short_preview(&kind, 140);
    let _ = writeln!(out, "         {}", rendered.header);
    let _ = writeln!(out, "         {}", rendered.first_line);
    for line in rendered.extra_lines {
        let _ = writeln!(out, "         {line}");
    }
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
            json_out: None,
            preview_bytes: 0,
            retained_size: false,
            exclude_soft_weak: false,
            merge_paths: false,
        }
    }

    #[test]
    fn render_text_includes_start_preview_when_array_previews_has_start_id() {
        use crate::result_recorder::ArrayPreview;
        let mut previews = ahash::AHashMap::new();
        previews.insert(
            42u64,
            ArrayPreview {
                element_type: crate::parser::gc_record::FieldType::Byte,
                bytes: b"<?xml version=\"1.0\"?>".to_vec().into_boxed_slice(),
                total_bytes: 21,
            },
        );
        let r = PathResult {
            start_object_id: 42,
            steps: vec![],
            terminated_at_root: false,
            root_kind: None,
            root_thread_name: None,
            root_frame: None,
            max_depth_reached: false,
            depth: 0,
            array_previews: previews,
            retained_by_oid: None,
            terminated_by_soft_weak: None,
        };
        let out = render_text(&r);
        assert!(
            out.contains("content: XML"),
            "expected content label, got:\n{out}"
        );
        assert!(out.contains("<?xml"), "expected preview, got:\n{out}");
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
            array_previews: ahash::AHashMap::new(),
            retained_by_oid: None,
            terminated_by_soft_weak: None,
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
    fn render_text_shows_array_index_for_object_array_hop() {
        let r = PathResult {
            start_object_id: 100,
            steps: vec![PathStep {
                holder_object_id: 200,
                holder_class: "java.lang.Object[]".to_string(),
                via_field: None,
                array_index: Some(12),
                held_object_id: 100,
            }],
            terminated_at_root: false,
            root_kind: None,
            root_thread_name: None,
            root_frame: None,
            max_depth_reached: false,
            depth: 1,
            array_previews: ahash::AHashMap::new(),
            retained_by_oid: None,
            terminated_by_soft_weak: None,
        };
        let out = render_text(&r);
        assert!(
            out.contains("via java.lang.Object[][12]"),
            "expected array index in arrow, got:\n{out}"
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
            array_previews: ahash::AHashMap::new(),
            retained_by_oid: None,
            terminated_by_soft_weak: None,
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
