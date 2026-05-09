//! Referrer (reverse-reference) tracing — replaces the standalone
//! `hprof-analyze-rust` tool.
//!
//! The streaming parser drops instance bodies and array element ids by
//! default (see `src/parser/record_parser.rs`), so referrer tracing runs
//! in multiple synchronous passes:
//!
//!   pass 1A — index utf8, classes (name / super / instance fields /
//!             object-typed statics), and GC root ids
//!   pass 1B — when the user targets a class FQ-name: stream instance
//!             dumps and collect the object ids whose class matches.
//!             Skipped when the target is `id:<N>` / a bare numeric id.
//!   pass 2  — retain-bodies stream: scan each instance's body and each
//!             object array's element ids for hits against the target
//!             id set; record (holder_class_id, field_name_id) -> count.
//!   pass 3  — when --hops >= 2: scan again, using the hop-1 object
//!             array holders as the new target set.
//!   pass 4  — when --hops >= 3: scan again, using the hop-2 instance
//!             holders as the target set.
//!
//! Each pass is a streaming pre-fetched read, so the working memory is
//! bounded by the index and the per-record body buffer (released as soon
//! as the record is consumed). Multi-hop cost is `O(hops × file_size)`.

use ahash::{AHashMap, AHashSet};
use serde::Serialize;
use std::cmp::Reverse;

use crate::args::Mode;
use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::TargetClassNotFound;
use crate::parser::gc_record::{FieldInfo, FieldType, FieldValue, GcRecord};
use crate::parser::record::{LoadClassData, Record};
use crate::slurp::parse_records;

/// Output of pass 1A. Holds enough metadata to:
/// (a) resolve the user's target class FQ-name to a `class_object_id`,
/// (b) flatten an instance's full field layout (own + super chain), and
/// (c) attribute holders to readable class / field names.
#[derive(Default)]
pub struct Pass1Index {
    pub utf8_by_id: AHashMap<u64, Box<str>>,
    /// `class_object_id -> class_name_id` (resolves via `utf8_by_id` to a
    /// readable FQ-name, e.g. `java.util.LinkedList`).
    pub class_name_id_by_class_id: AHashMap<u64, u64>,
    pub super_class_by_id: AHashMap<u64, u64>,
    /// Own-class instance fields in HPROF order (own class first; super
    /// fields appended at lookup time).
    pub fields_by_class_id: AHashMap<u64, Vec<FieldInfo>>,
    /// Object-typed static fields per class. Captured eagerly so `--include-statics`
    /// can do a hash-set membership check in pass 2 without re-streaming.
    pub static_object_fields_by_class_id: AHashMap<u64, Vec<(u64, u64)>>, // (name_id, target_obj_id)
    pub gc_root_ids: AHashSet<u64>,
    /// `object_id -> root tag label` (e.g. "RootJniGlobal", "RootJavaFrame").
    /// Used by `--paths-from-id` to label the chain terminator.
    pub gc_root_kind_by_id: AHashMap<u64, &'static str>,
    pub id_size: u32,
}

impl Pass1Index {
    /// Returns the dotted display form (`java.util.LinkedList`). HPROF
    /// stores class names slash-delimited; mirrors the conversion done by
    /// `ResultRecorder::get_class_name_string`.
    pub(crate) fn class_name(&self, class_id: u64) -> Option<String> {
        let name_id = *self.class_name_id_by_class_id.get(&class_id)?;
        self.utf8_by_id
            .get(&name_id)
            .map(|b| b.as_ref().replace('/', "."))
    }

    fn field_name(&self, name_id: u64) -> Option<&str> {
        self.utf8_by_id.get(&name_id).map(|b| b.as_ref())
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct ReferrerEntry {
    pub holder_class: String,
    /// `None` means the holder is an Object[] (no field name).
    pub field_name: Option<String>,
    pub ref_count: u64,
}

#[derive(Serialize, Debug)]
pub struct ReferrerResult {
    pub target_label: String,
    pub target_instance_count: u64,
    pub hop1: Vec<ReferrerEntry>,
    pub hop2: Vec<ReferrerEntry>,
    pub hop3: Vec<ReferrerEntry>,
}

pub fn run(mode: &Mode) -> Result<ReferrerResult, HprofSlurpError> {
    let (input_file, target, hops, top, include_statics, debug) = match mode {
        Mode::FindReferrers {
            input_file,
            target,
            hops,
            top,
            include_statics,
            debug,
            ..
        } => (
            input_file.as_str(),
            target.as_str(),
            *hops,
            *top,
            *include_statics,
            *debug,
        ),
        _ => {
            return Err(HprofSlurpError::NotYetImplemented {
                what: "referrer::run only handles Mode::FindReferrers",
            });
        }
    };

    let idx = pass1_index(input_file, debug)?;
    let (target_label, target_ids) = resolve_target_ids(input_file, &idx, target, debug)?;

    // ---- pass 2: hop 1 ----
    let mut by_field_hop1: AHashMap<(u64, Option<u64>), u64> = AHashMap::new();
    let mut hop1_array_holders: AHashSet<u64> = AHashSet::new();
    let mut hop1_instance_holders: AHashSet<u64> = AHashSet::new();

    if include_statics {
        for (class_id, statics) in &idx.static_object_fields_by_class_id {
            for (name_id, rid) in statics {
                if target_ids.contains(rid) {
                    *by_field_hop1
                        .entry((*class_id, Some(*name_id)))
                        .or_default() += 1;
                }
            }
        }
    }

    let mut field_layout_cache: AHashMap<u64, Vec<FieldInfo>> = AHashMap::new();
    scan_holders(
        input_file,
        debug,
        &idx,
        &target_ids,
        &mut field_layout_cache,
        &mut by_field_hop1,
        Some(&mut hop1_instance_holders),
        Some(&mut hop1_array_holders),
    )?;

    // ---- pass 3: hop 2 (object-array holders held by something) ----
    let mut by_field_hop2: AHashMap<(u64, Option<u64>), u64> = AHashMap::new();
    let mut hop2_instance_holders: AHashSet<u64> = AHashSet::new();
    if hops >= 2 && !hop1_array_holders.is_empty() {
        scan_holders(
            input_file,
            debug,
            &idx,
            &hop1_array_holders,
            &mut field_layout_cache,
            &mut by_field_hop2,
            Some(&mut hop2_instance_holders),
            None,
        )?;
    }

    // ---- pass 4: hop 3 (holders of the hop-2 instance holders) ----
    let mut by_field_hop3: AHashMap<(u64, Option<u64>), u64> = AHashMap::new();
    if hops >= 3 && !hop2_instance_holders.is_empty() {
        scan_holders(
            input_file,
            debug,
            &idx,
            &hop2_instance_holders,
            &mut field_layout_cache,
            &mut by_field_hop3,
            None,
            None,
        )?;
    }

    Ok(ReferrerResult {
        target_label,
        target_instance_count: target_ids.len() as u64,
        hop1: top_n(&idx, by_field_hop1, top),
        hop2: top_n(&idx, by_field_hop2, top),
        hop3: top_n(&idx, by_field_hop3, top),
    })
}

/// Resolve the user's `--find-referrers <target>` value into a concrete
/// id set. Accepts:
///   * `id:<u64>`   — a single object id
///   * `<u64>`      — also a single object id (bare digits)
///   * `<FQ name>`  — a class; pass 1B streams the file again to collect
///                    every instance id whose `class_object_id` matches.
fn resolve_target_ids(
    path: &str,
    idx: &Pass1Index,
    target: &str,
    debug: bool,
) -> Result<(String, AHashSet<u64>), HprofSlurpError> {
    if let Some(rest) = target.strip_prefix("id:") {
        let oid: u64 = rest.parse().map_err(|_| TargetClassNotFound {
            name: target.to_string(),
        })?;
        let mut ids = AHashSet::new();
        ids.insert(oid);
        return Ok((format!("id:{oid}"), ids));
    }
    if let Ok(oid) = target.parse::<u64>() {
        let mut ids = AHashSet::new();
        ids.insert(oid);
        return Ok((format!("id:{oid}"), ids));
    }

    // Class FQ-name: scan utf8_by_id for a name match (HPROF stores names
    // slash-delimited; compare in dotted form to match user input style).
    let target_class_id = idx
        .class_name_id_by_class_id
        .iter()
        .find_map(|(class_id, name_id)| {
            let raw = idx.utf8_by_id.get(name_id)?.as_ref();
            let dotted = raw.replace('/', ".");
            if dotted == target {
                Some(*class_id)
            } else {
                None
            }
        })
        .ok_or_else(|| TargetClassNotFound {
            name: target.to_string(),
        })?;

    let mut ids = AHashSet::new();
    parse_records(path, debug, false, |rec| {
        if let Record::GcSegment(GcRecord::InstanceDump {
            object_id,
            class_object_id,
            ..
        }) = rec
        {
            if class_object_id == target_class_id {
                ids.insert(object_id);
            }
        }
    })?;
    Ok((target.to_string(), ids))
}

pub(crate) fn pass1_index(path: &str, debug: bool) -> Result<Pass1Index, HprofSlurpError> {
    let mut idx = Pass1Index::default();
    let id_size = parse_records(path, debug, false, |rec| match rec {
        Record::Utf8String { id, str } => {
            idx.utf8_by_id.insert(id, str);
        }
        Record::LoadClass(LoadClassData {
            class_object_id,
            class_name_id,
            ..
        }) => {
            idx.class_name_id_by_class_id
                .insert(class_object_id, class_name_id);
        }
        Record::GcSegment(gc) => match gc {
            GcRecord::ClassDump(boxed) => {
                let cd = *boxed;
                idx.super_class_by_id
                    .insert(cd.class_object_id, cd.super_class_object_id);

                if !cd.instance_fields.is_empty() {
                    idx.fields_by_class_id
                        .insert(cd.class_object_id, cd.instance_fields);
                }

                let mut statics = Vec::new();
                for (fi, fv) in cd.static_fields {
                    if let FieldValue::Object(rid) = fv {
                        if rid != 0 {
                            statics.push((fi.name_id, rid));
                        }
                    }
                }
                if !statics.is_empty() {
                    idx.static_object_fields_by_class_id
                        .insert(cd.class_object_id, statics);
                }
            }
            GcRecord::RootJniGlobal { object_id, .. } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootJniGlobal");
            }
            GcRecord::RootJniLocal { object_id, .. } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootJniLocal");
            }
            GcRecord::RootJavaFrame { object_id, .. } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootJavaFrame");
            }
            GcRecord::RootNativeStack { object_id, .. } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootNativeStack");
            }
            GcRecord::RootStickyClass { object_id } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootStickyClass");
            }
            GcRecord::RootThreadBlock { object_id, .. } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootThreadBlock");
            }
            GcRecord::RootMonitorUsed { object_id } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootMonitorUsed");
            }
            GcRecord::RootUnknown { object_id } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootUnknown");
            }
            GcRecord::RootThreadObject {
                thread_object_id, ..
            } => {
                idx.gc_root_ids.insert(thread_object_id);
                idx.gc_root_kind_by_id
                    .insert(thread_object_id, "RootThreadObject");
            }
            _ => {}
        },
        _ => {}
    })?;
    idx.id_size = id_size;
    Ok(idx)
}

/// Single hop scan: stream the file with retain-bodies, and for each
/// instance / object-array record bump `by_field` whenever a contained
/// reference points into `target_set`. Optionally records which records
/// were holders into `instance_holders_out` / `array_holders_out` so the
/// caller can chain another hop.
#[allow(clippy::too_many_arguments)]
fn scan_holders(
    path: &str,
    debug: bool,
    idx: &Pass1Index,
    target_set: &AHashSet<u64>,
    field_layout_cache: &mut AHashMap<u64, Vec<FieldInfo>>,
    by_field: &mut AHashMap<(u64, Option<u64>), u64>,
    mut instance_holders_out: Option<&mut AHashSet<u64>>,
    mut array_holders_out: Option<&mut AHashSet<u64>>,
) -> Result<(), HprofSlurpError> {
    let id_size = idx.id_size as usize;
    parse_records(path, debug, true, |rec| {
        if let Record::GcSegment(gc) = rec {
            match gc {
                GcRecord::InstanceDump {
                    object_id,
                    class_object_id,
                    body: Some(body),
                    ..
                } => {
                    let layout = field_layout_cache
                        .entry(class_object_id)
                        .or_insert_with(|| flatten_fields(idx, class_object_id));
                    let mut input: &[u8] = &body;
                    let mut hit = false;
                    for fi in layout {
                        let consume = field_byte_size(fi.field_type, id_size);
                        if input.len() < consume {
                            break;
                        }
                        if fi.field_type == FieldType::Object {
                            let rid = read_id(&input[..id_size], id_size);
                            if rid != 0 && target_set.contains(&rid) {
                                *by_field
                                    .entry((class_object_id, Some(fi.name_id)))
                                    .or_default() += 1;
                                hit = true;
                            }
                        }
                        input = &input[consume..];
                    }
                    if hit {
                        if let Some(out) = instance_holders_out.as_deref_mut() {
                            out.insert(object_id);
                        }
                    }
                }
                GcRecord::ObjectArrayDump {
                    object_id,
                    array_class_id,
                    elements: Some(elems),
                    ..
                } => {
                    let mut hit = false;
                    for &rid in elems.iter() {
                        if rid != 0 && target_set.contains(&rid) {
                            *by_field.entry((array_class_id, None)).or_default() += 1;
                            hit = true;
                        }
                    }
                    if hit {
                        if let Some(out) = array_holders_out.as_deref_mut() {
                            out.insert(object_id);
                        }
                    }
                }
                _ => {}
            }
        }
    })?;
    Ok(())
}

pub(crate) fn flatten_fields(idx: &Pass1Index, class_id: u64) -> Vec<FieldInfo> {
    let mut out = Vec::new();
    let mut cur = class_id;
    while cur != 0 {
        if let Some(fields) = idx.fields_by_class_id.get(&cur) {
            for f in fields {
                out.push(FieldInfo {
                    name_id: f.name_id,
                    field_type: f.field_type,
                });
            }
        }
        match idx.super_class_by_id.get(&cur) {
            Some(&sup) if sup != 0 => cur = sup,
            _ => break,
        }
    }
    out
}

pub(crate) const fn field_byte_size(t: FieldType, id_size: usize) -> usize {
    match t {
        FieldType::Object => id_size,
        FieldType::Bool | FieldType::Byte => 1,
        FieldType::Char | FieldType::Short => 2,
        FieldType::Int | FieldType::Float => 4,
        FieldType::Long | FieldType::Double => 8,
    }
}

pub(crate) fn read_id(b: &[u8], id_size: usize) -> u64 {
    match id_size {
        4 => u32::from_be_bytes(b[..4].try_into().expect("4-byte id")) as u64,
        8 => u64::from_be_bytes(b[..8].try_into().expect("8-byte id")),
        x => panic!("unsupported id_size {x}"),
    }
}

fn top_n(
    idx: &Pass1Index,
    by_field: AHashMap<(u64, Option<u64>), u64>,
    top: usize,
) -> Vec<ReferrerEntry> {
    let mut entries: Vec<((u64, Option<u64>), u64)> = by_field.into_iter().collect();
    entries.sort_by_key(|(_, c)| Reverse(*c));
    entries
        .into_iter()
        .take(top)
        .map(|((class_id, maybe_name), ref_count)| {
            let holder_class = idx
                .class_name(class_id)
                .unwrap_or_else(|| format!("(class_id={class_id})"));
            let field_name = maybe_name.and_then(|nid| idx.field_name(nid).map(String::from));
            ReferrerEntry {
                holder_class,
                field_name,
                ref_count,
            }
        })
        .collect()
}

/// Render a result block as ASCII text for stdout. A full reusable table
/// renderer lands in Task 5; this is a focused one-off until then.
pub fn render_text(r: &ReferrerResult) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\nFound {} target instance(s) for {}",
        r.target_instance_count, r.target_label
    );
    render_section(&mut out, "Direct referrers (1-hop)", &r.hop1);
    if !r.hop2.is_empty() {
        render_section(
            &mut out,
            "2-hop referrers (X holds Object[] which holds target)",
            &r.hop2,
        );
    }
    if !r.hop3.is_empty() {
        render_section(
            &mut out,
            "3-hop referrers (X holds Y which holds Object[] which holds target)",
            &r.hop3,
        );
    }
    out
}

fn render_section(out: &mut String, title: &str, rows: &[ReferrerEntry]) {
    use std::fmt::Write;
    let _ = writeln!(out, "\n=== {title} ===");
    if rows.is_empty() {
        let _ = writeln!(out, "  (none)");
        return;
    }
    let max_holder = rows
        .iter()
        .map(|e| e.holder_class.len() + e.field_name.as_deref().map_or(0, str::len) + 1)
        .max()
        .unwrap_or(40)
        .min(120);
    let _ = writeln!(
        out,
        "  {:<width$} {:>10}",
        "holder.field (or class[] for arrays)",
        "ref count",
        width = max_holder.max(36)
    );
    for e in rows {
        let key = match &e.field_name {
            None => format!("{}[]", e.holder_class),
            Some(f) => format!("{}.{f}", e.holder_class),
        };
        let _ = writeln!(
            out,
            "  {:<width$} {:>10}",
            trim(&key, max_holder.max(36)),
            e.ref_count,
            width = max_holder.max(36)
        );
    }
}

fn trim(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}...", &s[..n.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_args(target: &str, hops: u8) -> Mode {
        Mode::FindReferrers {
            input_file: "test-heap-dumps/hprof-64.bin".to_string(),
            target: target.to_string(),
            hops,
            top: 10,
            include_statics: true,
            debug: false,
            json: false,
        }
    }

    #[test]
    fn pass1_indexes_class_metadata() {
        let idx = pass1_index("test-heap-dumps/hprof-64.bin", false).unwrap();
        assert!(
            idx.class_name_id_by_class_id.len() >= 100,
            "expected ≥100 classes indexed, got {}",
            idx.class_name_id_by_class_id.len()
        );
        assert!(
            idx.gc_root_ids.len() > 10,
            "expected GC roots, got {}",
            idx.gc_root_ids.len()
        );
        assert_eq!(idx.id_size, 8, "fixture is 64-bit");
    }

    #[test]
    fn linkedlist_target_resolves_with_instances() {
        let idx = pass1_index("test-heap-dumps/hprof-64.bin", false).unwrap();
        let (label, ids) = resolve_target_ids(
            "test-heap-dumps/hprof-64.bin",
            &idx,
            "java.util.LinkedList",
            false,
        )
        .unwrap();
        assert_eq!(label, "java.util.LinkedList");
        // gold file shows 190 LinkedList instances
        assert_eq!(ids.len(), 190, "expected 190 LinkedList instances");
    }

    #[test]
    fn hop1_finds_linkedlist_referrers() {
        let r = run(&fixture_args("java.util.LinkedList", 1)).unwrap();
        assert!(
            r.target_instance_count > 0,
            "expected non-zero target instance count"
        );
        assert!(
            !r.hop1.is_empty(),
            "expected at least one hop-1 holder for LinkedList"
        );
        assert!(r.hop2.is_empty(), "hops=1 must not compute hop2");
        assert!(r.hop3.is_empty(), "hops=1 must not compute hop3");
    }

    #[test]
    fn hop1_for_linkedlist_node_finds_self_reference() {
        let r = run(&fixture_args("java.util.LinkedList$Node", 1)).unwrap();
        // LinkedList$Node nodes point at each other via next/prev — the
        // dominant hop1 holder of LinkedList$Node should be LinkedList$Node.
        let holder_classes: Vec<&str> = r.hop1.iter().map(|e| e.holder_class.as_str()).collect();
        assert!(
            holder_classes.contains(&"java.util.LinkedList$Node"),
            "expected LinkedList$Node in hop1 holders, got {holder_classes:?}"
        );
    }

    #[test]
    fn target_class_not_found_errors() {
        let res = run(&fixture_args("com.does.not.Exist", 1));
        match res {
            Err(HprofSlurpError::TargetClassNotFound { name }) => {
                assert_eq!(name, "com.does.not.Exist");
            }
            other => panic!("expected TargetClassNotFound, got {other:?}"),
        }
    }
}
