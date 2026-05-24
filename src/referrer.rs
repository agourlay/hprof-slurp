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
use crate::parser::record::{LoadClassData, Record, StackFrameData, StackTraceData};
use crate::slurp::parse_records;

/// A stack frame whose utf8 references have already been chased to readable
/// strings. Built lazily — we only resolve frames the renderer actually
/// asks for, so a dump with 50K frames doesn't pay for resolving any of
/// them unless `--paths-from-id` chases one.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedFrame {
    pub method: String,
    pub class: Option<String>,
    pub file: Option<String>,
    /// HPROF spec: positive = real line, negative sentinel values for
    /// "unknown", "compiled", "native". Surfaced verbatim; renderer
    /// translates sentinels.
    pub line: i32,
}

/// Pointer recorded when the indexer sees a thread-owned GC root. Used by
/// `paths::run` to resolve the chain terminator's thread name + top frame.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct RootMetadata {
    pub object_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_serial: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_trace_serial: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_object_id: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub struct ThreadFrameRef {
    pub thread_serial: u32,
    /// `Some(idx)` for `RootJavaFrame` — index into the thread's stack
    /// trace. `None` for `RootThreadObject` (no frame), `RootJniLocal`,
    /// `RootJniMonitor` (we only have a stack depth, not a frame index).
    pub frame_idx: Option<u32>,
}

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
    // ---- v0.8.0 (feature A) thread + stack frame metadata ----
    /// `thread_serial_number -> thread name`. Populated from `StartThread`.
    pub thread_name_by_serial: AHashMap<u32, Box<str>>,
    /// `thread_object_id -> thread_serial_number`. Used to resolve a
    /// `RootThreadObject { thread_object_id }` back to its name (which is
    /// keyed by serial).
    pub thread_serial_by_obj_id: AHashMap<u64, u32>,
    /// `stack_trace_serial_number -> [stack_frame_id, ...]`. Populated
    /// from `StackTrace`.
    pub stack_trace_by_serial: AHashMap<u32, Vec<u64>>,
    /// `stack_frame_id -> raw StackFrameData`. utf8 resolution happens on
    /// demand via `Pass1Index::resolve_frame()`. We store the raw record
    /// so resolution stays lazy.
    pub stack_frame_by_id: AHashMap<u64, StackFrameData>,
    /// `class_serial_number -> class_name_id`. Captured from `LoadClass`.
    /// Distinct from the existing `class_name_id_by_class_id` (which is
    /// keyed by `class_object_id`); HPROF references classes by *serial*
    /// in `StackFrame.class_serial_number` and `AllocationSite.class_serial_number`.
    pub class_name_id_by_serial: AHashMap<u32, u64>,
    /// Root object id -> thread metadata. Captured at index time so the
    /// `paths` walker doesn't need to re-scan to find which thread owns a
    /// terminating root.
    pub root_thread_meta_by_id: AHashMap<u64, ThreadFrameRef>,
    pub root_metadata_by_id: AHashMap<u64, RootMetadata>,
    pub id_size: u32,
    // ---- v1.1.0 derivations (populated post-build by reference_classes::derive) ----
    /// Transitive subclasses of `java.lang.ref.{Soft,Weak,Phantom}Reference`.
    /// Empty when none of the three marker classes were loaded.
    pub reference_subclass_set: AHashSet<u64>,
    /// `android.graphics.Bitmap` class metadata, when present in the
    /// dump. `None` on JVM dumps and on Android dumps where Bitmap was
    /// not loaded.
    pub bitmap_class_info: Option<crate::reference_classes::BitmapClassInfo>,
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

    /// Resolve a `stack_frame_id` to a `ResolvedFrame` if all the utf8
    /// references in the underlying `StackFrame` record are reachable.
    /// Returns `None` if the frame id isn't known.
    pub(crate) fn resolve_frame(&self, frame_id: u64) -> Option<ResolvedFrame> {
        let f = self.stack_frame_by_id.get(&frame_id)?;
        let method = self
            .utf8_by_id
            .get(&f.method_name_id)
            .map(|s| s.as_ref().to_string())
            .unwrap_or_else(|| format!("(method_name_id={})", f.method_name_id));
        let class = self
            .class_name_id_by_serial
            .get(&f.class_serial_number)
            .and_then(|nid| self.utf8_by_id.get(nid))
            .map(|s| s.as_ref().replace('/', "."));
        let file = self
            .utf8_by_id
            .get(&f.source_file_name_id)
            .map(|s| s.as_ref().to_string());
        Some(ResolvedFrame {
            method,
            class,
            file,
            line: f.line_number,
        })
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct ReferrerEntry {
    pub holder_class: String,
    /// `None` means the holder is an Object[] (no field name).
    pub field_name: Option<String>,
    pub ref_count: u64,
}

#[derive(Serialize, Debug, Clone)]
pub struct MatchedClass {
    pub class_name: String,
    pub instance_count: u64,
}

#[derive(Serialize, Debug)]
pub struct ReferrerResult {
    pub target_label: String,
    pub target_instance_count: u64,
    /// Per-class breakdown when targeting via `--target-glob`. Empty for
    /// exact-match targets.
    pub matched_classes: Vec<MatchedClass>,
    pub hop1: Vec<ReferrerEntry>,
    pub hop2: Vec<ReferrerEntry>,
    pub hop3: Vec<ReferrerEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grouped_holders: Vec<crate::holder_grouping::GroupedHolder>,
    /// v0.9.0: preview bodies (when --preview-bytes > 0) keyed by
    /// object_id. Used by `render_text` to display content above the
    /// hop tables when the target is a primitive array. Skipped from
    /// JSON since the truncated blob isn't useful structured.
    #[serde(skip)]
    pub array_previews: AHashMap<u64, crate::result_recorder::ArrayPreview>,
    /// v1.0.0 (feature E): `class_name -> retained_bytes` from the
    /// dominator tree (when `--retained-size` was set). `None`
    /// otherwise. Skipped from JSON to keep the schema stable;
    /// consumers that want class-level retained should run
    /// `summary --retained-size --json`.
    #[serde(skip)]
    pub class_retained_by_name: Option<AHashMap<String, u64>>,
}

impl ReferrerResult {
    pub fn symbolicate(&mut self, symbolicator: &crate::mapping::Symbolicator) {
        self.target_label = deobfuscate_target_label(&self.target_label, symbolicator);
        for matched in &mut self.matched_classes {
            matched.class_name = symbolicator.class_name(&matched.class_name);
        }
        for row in self
            .hop1
            .iter_mut()
            .chain(self.hop2.iter_mut())
            .chain(self.hop3.iter_mut())
        {
            let raw_holder = row.holder_class.clone();
            if let Some(field) = row.field_name.as_mut() {
                *field = symbolicator.field_name(&raw_holder, field);
            }
            row.holder_class = symbolicator.class_name(&row.holder_class);
        }
        if let Some(retained) = self.class_retained_by_name.take() {
            let mut remapped = AHashMap::new();
            for (class_name, bytes) in retained {
                *remapped
                    .entry(symbolicator.class_name(&class_name))
                    .or_insert(0) += bytes;
            }
            self.class_retained_by_name = Some(remapped);
        }
    }

    pub fn refresh_grouped_holders(&mut self, retained_by_name: Option<&AHashMap<String, u64>>) {
        self.grouped_holders = crate::holder_grouping::group_entries(
            self.hop1
                .iter()
                .chain(self.hop2.iter())
                .chain(self.hop3.iter())
                .cloned(),
            retained_by_name,
        );
    }
}

fn deobfuscate_target_label(label: &str, symbolicator: &crate::mapping::Symbolicator) -> String {
    if label.starts_with("id:") {
        label.to_string()
    } else {
        symbolicator.class_name(label)
    }
}

pub fn run(mode: &Mode) -> Result<ReferrerResult, HprofSlurpError> {
    let (
        input_file,
        target,
        hops,
        top,
        include_statics,
        debug,
        preview_bytes,
        retained_size,
        exclude_soft_weak,
        group_holders,
    ) = match mode {
        Mode::FindReferrers {
            input_file,
            target,
            hops,
            top,
            include_statics,
            debug,
            preview_bytes,
            retained_size,
            exclude_soft_weak,
            group_holders,
            ..
        } => (
            input_file.as_str(),
            target.clone(),
            *hops,
            *top,
            *include_statics,
            *debug,
            *preview_bytes,
            *retained_size,
            *exclude_soft_weak,
            *group_holders,
        ),
        _ => {
            return Err(HprofSlurpError::NotYetImplemented {
                what: "referrer::run only handles Mode::FindReferrers",
            });
        }
    };

    let idx = pass1_index(input_file, debug)?;
    let array_previews: AHashMap<u64, crate::result_recorder::ArrayPreview> = if preview_bytes > 0 {
        crate::paths::collect_primitive_array_previews(input_file, debug, preview_bytes)?
    } else {
        AHashMap::new()
    };

    // v1.0.0 (feature E): build the dominator tree once and roll up
    // retained sizes per class so the renderer can add a `class
    // retained` column to the holder tables.
    let class_retained_by_name: Option<AHashMap<String, u64>> = if retained_size {
        let graph = crate::reference_graph::build_from_pass1_with(
            input_file,
            &idx,
            debug,
            crate::reference_graph::BuildOptions { exclude_soft_weak },
        )?;
        let idom = crate::dominators::lengauer_tarjan(&graph);
        let analysis = crate::retained::compute(&graph, &idom, 0);
        let mut by_name: AHashMap<String, u64> = AHashMap::new();
        for (&cid, &bytes) in &analysis.class_retained {
            by_name.insert(class_label_for_id(&idx, cid), bytes);
        }
        Some(by_name)
    } else {
        None
    };

    let (target_label, target_ids, matched_classes) =
        resolve_target_ids(input_file, &idx, &target, debug)?;

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
        exclude_soft_weak,
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
            exclude_soft_weak,
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
            exclude_soft_weak,
        )?;
    }

    let mut result = ReferrerResult {
        target_label,
        target_instance_count: target_ids.len() as u64,
        matched_classes,
        array_previews,
        class_retained_by_name,
        hop1: top_n(&idx, by_field_hop1, top),
        hop2: top_n(&idx, by_field_hop2, top),
        hop3: top_n(&idx, by_field_hop3, top),
        grouped_holders: Vec::new(),
    };
    if group_holders {
        let retained = result.class_retained_by_name.clone();
        result.refresh_grouped_holders(retained.as_ref());
    }
    Ok(result)
}

/// Class-name label for retained-size lookups. Mirrors
/// `slurp::class_label` (kept private there) but adapted for
/// `Pass1Index`'s `class_name` accessor and the synthetic
/// primitive-array sentinel scheme used by `reference_graph`.
pub(crate) fn class_label_for_id(idx: &Pass1Index, class_object_id: u64) -> String {
    if class_object_id >> 8 == 0x00FF_FFFF_FFFF_FFFFu64 {
        return match class_object_id & 0xFF {
            1 => "bool[]".to_string(),
            2 => "byte[]".to_string(),
            3 => "char[]".to_string(),
            4 => "short[]".to_string(),
            5 => "int[]".to_string(),
            6 => "float[]".to_string(),
            7 => "long[]".to_string(),
            8 => "double[]".to_string(),
            _ => "primitive[]".to_string(),
        };
    }
    if let Some(name) = idx.class_name(class_object_id) {
        if let Some(stripped) = name.strip_prefix("[[L").and_then(|s| s.strip_suffix(';')) {
            return format!("{stripped}[]");
        }
        if let Some(stripped) = name.strip_prefix("[L").and_then(|s| s.strip_suffix(';')) {
            return format!("{stripped}[]");
        }
        return name;
    }
    format!("class:{class_object_id:x}")
}

/// Resolve the user's target spec into a concrete id set. Returns
/// `(label, ids, matched_classes)`. `matched_classes` is empty for
/// `Exact` targets and populated for `Glob` targets.
fn resolve_target_ids(
    path: &str,
    idx: &Pass1Index,
    target: &crate::args::ReferrersTarget,
    debug: bool,
) -> Result<(String, AHashSet<u64>, Vec<MatchedClass>), HprofSlurpError> {
    match target {
        crate::args::ReferrersTarget::Exact(s) => resolve_exact(path, idx, s, debug),
        crate::args::ReferrersTarget::Glob(pattern) => resolve_glob(path, idx, pattern, debug),
    }
}

fn resolve_glob(
    path: &str,
    idx: &Pass1Index,
    pattern: &str,
    debug: bool,
) -> Result<(String, AHashSet<u64>, Vec<MatchedClass>), HprofSlurpError> {
    use globset::GlobBuilder;
    let matcher = GlobBuilder::new(pattern)
        .literal_separator(true) // `*` doesn't cross `.`; `**` does
        .build()
        .map_err(|e| HprofSlurpError::InvalidHprofFile {
            message: format!("bad glob pattern '{pattern}': {e}"),
        })?
        .compile_matcher();

    // Find every class whose dotted FQ-name matches the glob.
    let mut matched_class_ids: Vec<(u64, String)> = Vec::new();
    for (class_id, name_id) in &idx.class_name_id_by_class_id {
        if let Some(raw) = idx.utf8_by_id.get(name_id) {
            let dotted = raw.as_ref().replace('/', ".");
            if matcher.is_match(&dotted) {
                matched_class_ids.push((*class_id, dotted));
            }
        }
    }
    if matched_class_ids.is_empty() {
        return Err(TargetClassNotFound {
            name: format!(
                "glob '{pattern}' matched no classes; check available classes with: heaptrail -i <file> -t 1000"
            ),
        });
    }

    // Pass 1B: collect instance ids of every matched class.
    let class_id_set: AHashSet<u64> = matched_class_ids.iter().map(|(c, _)| *c).collect();
    let mut ids = AHashSet::new();
    let mut count_by_class: AHashMap<u64, u64> = AHashMap::new();
    parse_records(path, debug, false, |rec| {
        if let Record::GcSegment(GcRecord::InstanceDump {
            object_id,
            class_object_id,
            ..
        }) = rec
            && class_id_set.contains(&class_object_id)
        {
            ids.insert(object_id);
            *count_by_class.entry(class_object_id).or_default() += 1;
        }
    })?;

    let mut matched: Vec<MatchedClass> = matched_class_ids
        .into_iter()
        .map(|(cid, name)| MatchedClass {
            class_name: name,
            instance_count: count_by_class.get(&cid).copied().unwrap_or(0),
        })
        // Suppress glob-matches with zero live instances — they're loaded
        // classes nobody allocated, which is just noise in the header.
        // (The class is still counted by the glob; it's just not listed.)
        .filter(|m| m.instance_count > 0)
        .collect();
    matched.sort_by_key(|m| Reverse(m.instance_count));

    Ok((format!("glob:{pattern}"), ids, matched))
}

/// Exact-match target resolution. Accepts:
/// * `id:<u64>`   — a single object id
/// * `<u64>`      — also a single object id (bare digits)
/// * `<FQ name>`  — a class; pass 1B streams the file again to collect
///   every instance id whose `class_object_id` matches.
fn resolve_exact(
    path: &str,
    idx: &Pass1Index,
    target: &str,
    debug: bool,
) -> Result<(String, AHashSet<u64>, Vec<MatchedClass>), HprofSlurpError> {
    if let Some(rest) = target.strip_prefix("id:") {
        let oid: u64 = rest.parse().map_err(|_| TargetClassNotFound {
            name: target.to_string(),
        })?;
        let mut ids = AHashSet::new();
        ids.insert(oid);
        return Ok((format!("id:{oid}"), ids, vec![]));
    }
    if let Ok(oid) = target.parse::<u64>() {
        let mut ids = AHashSet::new();
        ids.insert(oid);
        return Ok((format!("id:{oid}"), ids, vec![]));
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
            && class_object_id == target_class_id
        {
            ids.insert(object_id);
        }
    })?;
    Ok((target.to_string(), ids, vec![]))
}

pub(crate) fn pass1_index(path: &str, debug: bool) -> Result<Pass1Index, HprofSlurpError> {
    let mut idx = Pass1Index::default();
    let id_size = parse_records(path, debug, false, |rec| match rec {
        Record::Utf8String { id, str } => {
            idx.utf8_by_id.insert(id, str);
        }
        Record::LoadClass(LoadClassData {
            serial_number,
            class_object_id,
            class_name_id,
            ..
        }) => {
            idx.class_name_id_by_class_id
                .insert(class_object_id, class_name_id);
            // Feature A (v0.8.0): also index by class serial — StackFrame
            // and AllocationSite reference classes by serial, not obj id.
            idx.class_name_id_by_serial
                .insert(serial_number, class_name_id);
        }
        Record::StartThread {
            thread_serial_number,
            thread_object_id,
            thread_name_id,
            ..
        } => {
            if let Some(name) = idx.utf8_by_id.get(&thread_name_id).cloned() {
                idx.thread_name_by_serial.insert(thread_serial_number, name);
            } else {
                // utf8 record may appear later; record a placeholder and
                // tolerate the gap downstream.
                idx.thread_name_by_serial.insert(
                    thread_serial_number,
                    format!("(name_id={thread_name_id})").into_boxed_str(),
                );
            }
            idx.thread_serial_by_obj_id
                .insert(thread_object_id, thread_serial_number);
        }
        Record::StackTrace(StackTraceData {
            serial_number,
            stack_frame_ids,
            ..
        }) => {
            idx.stack_trace_by_serial
                .insert(serial_number, stack_frame_ids);
        }
        Record::StackFrame(sfd) => {
            idx.stack_frame_by_id.insert(sfd.stack_frame_id, sfd);
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
                    if let FieldValue::Object(rid) = fv
                        && rid != 0
                    {
                        statics.push((fi.name_id, rid));
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
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: None,
                        stack_trace_serial: None,
                        frame_index: None,
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootJniLocal {
                object_id,
                thread_serial_number,
                frame_number_in_stack_trace,
                ..
            } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootJniLocal");
                idx.root_thread_meta_by_id.insert(
                    object_id,
                    ThreadFrameRef {
                        thread_serial: thread_serial_number,
                        frame_idx: None,
                    },
                );
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: Some(thread_serial_number),
                        stack_trace_serial: None,
                        frame_index: Some(frame_number_in_stack_trace),
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootJavaFrame {
                object_id,
                thread_serial_number,
                frame_number_in_stack_trace,
            } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootJavaFrame");
                idx.root_thread_meta_by_id.insert(
                    object_id,
                    ThreadFrameRef {
                        thread_serial: thread_serial_number,
                        frame_idx: Some(frame_number_in_stack_trace),
                    },
                );
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: Some(thread_serial_number),
                        stack_trace_serial: None,
                        frame_index: Some(frame_number_in_stack_trace),
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootNativeStack {
                object_id,
                thread_serial_number,
            } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootNativeStack");
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: Some(thread_serial_number),
                        stack_trace_serial: None,
                        frame_index: None,
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootStickyClass { object_id } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootStickyClass");
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: None,
                        stack_trace_serial: None,
                        frame_index: None,
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootThreadBlock {
                object_id,
                thread_serial_number,
            } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootThreadBlock");
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: Some(thread_serial_number),
                        stack_trace_serial: None,
                        frame_index: None,
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootMonitorUsed { object_id } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootMonitorUsed");
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: None,
                        stack_trace_serial: None,
                        frame_index: None,
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootUnknown { object_id } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootUnknown");
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: None,
                        stack_trace_serial: None,
                        frame_index: None,
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootThreadObject {
                thread_object_id,
                thread_sequence_number,
                stack_sequence_number,
            } => {
                idx.gc_root_ids.insert(thread_object_id);
                idx.gc_root_kind_by_id
                    .insert(thread_object_id, "RootThreadObject");
                idx.root_metadata_by_id.insert(
                    thread_object_id,
                    RootMetadata {
                        object_id: thread_object_id,
                        thread_serial: Some(thread_sequence_number),
                        stack_trace_serial: Some(stack_sequence_number),
                        frame_index: None,
                        thread_object_id: Some(thread_object_id),
                    },
                );
            }
            // Android HPROF 1.0.3 extension roots
            GcRecord::RootInternedString { object_id } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id
                    .insert(object_id, "RootInternedString");
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: None,
                        stack_trace_serial: None,
                        frame_index: None,
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootFinalizing { object_id } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootFinalizing");
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: None,
                        stack_trace_serial: None,
                        frame_index: None,
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootDebugger { object_id } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootDebugger");
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: None,
                        stack_trace_serial: None,
                        frame_index: None,
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootReferenceCleanup { object_id } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id
                    .insert(object_id, "RootReferenceCleanup");
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: None,
                        stack_trace_serial: None,
                        frame_index: None,
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootVmInternal { object_id } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootVmInternal");
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: None,
                        stack_trace_serial: None,
                        frame_index: None,
                        thread_object_id: None,
                    },
                );
            }
            GcRecord::RootJniMonitor {
                object_id,
                thread_serial_number,
                stack_depth,
                ..
            } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootJniMonitor");
                idx.root_thread_meta_by_id.insert(
                    object_id,
                    ThreadFrameRef {
                        thread_serial: thread_serial_number,
                        frame_idx: None,
                    },
                );
                idx.root_metadata_by_id.insert(
                    object_id,
                    RootMetadata {
                        object_id,
                        thread_serial: Some(thread_serial_number),
                        stack_trace_serial: None,
                        frame_index: Some(stack_depth),
                        thread_object_id: None,
                    },
                );
            }
            _ => {}
        },
        _ => {}
    })?;
    idx.id_size = id_size;

    // v1.1.0: derive soft/weak/phantom subclass set + bitmap class info
    // from the now-populated index. Cheap (~10 ms on 200 MiB Android).
    let (refs, bitmap) = crate::reference_classes::derive(&idx);
    idx.reference_subclass_set = refs.soft_weak_phantom;
    idx.bitmap_class_info = bitmap;

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
    exclude_soft_weak: bool,
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
                    // v1.1.0: skip Reference subclass holders entirely
                    // when --exclude-soft-weak is set.
                    if exclude_soft_weak && idx.reference_subclass_set.contains(&class_object_id) {
                        return;
                    }
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
                    if hit && let Some(out) = instance_holders_out.as_deref_mut() {
                        out.insert(object_id);
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
                    if hit && let Some(out) = array_holders_out.as_deref_mut() {
                        out.insert(object_id);
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
    if !r.matched_classes.is_empty() {
        let _ = writeln!(
            out,
            "\nFound {} classes matching {}:",
            r.matched_classes.len(),
            r.target_label
        );
        let max_name_len = r
            .matched_classes
            .iter()
            .map(|m| m.class_name.len())
            .max()
            .unwrap_or(0)
            .min(80);
        for m in &r.matched_classes {
            let _ = writeln!(
                out,
                "  - {:<width$} ({} instances)",
                m.class_name,
                m.instance_count,
                width = max_name_len
            );
        }
    }
    let _ = writeln!(
        out,
        "\nFound {} target instance(s) for {}",
        r.target_instance_count, r.target_label
    );
    render_target_preview(&mut out, &r.target_label, &r.array_previews);
    render_grouped_holders(&mut out, &r.grouped_holders);
    render_section(
        &mut out,
        "Direct referrers (1-hop)",
        &r.hop1,
        r.class_retained_by_name.as_ref(),
    );
    if !r.hop2.is_empty() {
        render_section(
            &mut out,
            "2-hop referrers (X holds Object[] which holds target)",
            &r.hop2,
            r.class_retained_by_name.as_ref(),
        );
    }
    if !r.hop3.is_empty() {
        render_section(
            &mut out,
            "3-hop referrers (X holds Y which holds Object[] which holds target)",
            &r.hop3,
            r.class_retained_by_name.as_ref(),
        );
    }
    out
}

fn render_grouped_holders(out: &mut String, rows: &[crate::holder_grouping::GroupedHolder]) {
    use std::fmt::Write;
    if rows.is_empty() {
        return;
    }
    out.push_str("\n=== Grouped holders ===\n");
    let _ = writeln!(
        out,
        "  {:<28} {:<52} {:<28} {:>9}",
        "owner family", "holder class", "field", "ref count"
    );
    for row in rows {
        let _ = writeln!(
            out,
            "  {:<28} {:<52} {:<28} {:>9}",
            trim(&row.owner_family, 28),
            trim(&row.holder_class, 52),
            trim(&row.field_label, 28),
            row.ref_count
        );
    }
}

fn render_target_preview(
    out: &mut String,
    label: &str,
    previews: &AHashMap<u64, crate::result_recorder::ArrayPreview>,
) {
    use std::fmt::Write;
    // Only id-targeted invocations carry a useful preview here. Class
    // targets (FQ-name or glob) point at many instances; rendering each
    // one's preview would clutter the report. We deliberately limit to
    // the "id:" form.
    let id_str = match label.strip_prefix("id:") {
        Some(s) => s,
        None => return,
    };
    let id: u64 = match id_str.parse() {
        Ok(v) => v,
        Err(_) => return,
    };
    if let Some(preview) = previews.get(&id) {
        use crate::preview::{render_preview, render_short_preview};
        let kind = render_preview(
            &preview.bytes,
            preview.element_type,
            preview.total_bytes as usize,
        );
        let rendered = render_short_preview(&kind, 140);
        let _ = writeln!(out, "  preview: {}", rendered.header);
        let _ = writeln!(out, "    {}", rendered.first_line);
        for line in rendered.extra_lines {
            let _ = writeln!(out, "    {line}");
        }
    }
}

fn render_section(
    out: &mut String,
    title: &str,
    rows: &[ReferrerEntry],
    class_retained_by_name: Option<&AHashMap<String, u64>>,
) {
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
    let width = max_holder.max(36);
    if let Some(class_retained) = class_retained_by_name {
        let _ = writeln!(
            out,
            "  {:<width$} {:>10} {:>15}",
            "holder.field (or class[] for arrays)", "ref count", "class retained",
        );
        for e in rows {
            let key = match &e.field_name {
                None => format!("{}[]", e.holder_class),
                Some(f) => format!("{}.{f}", e.holder_class),
            };
            let retained = class_retained.get(&e.holder_class).copied().unwrap_or(0);
            let _ = writeln!(
                out,
                "  {:<width$} {:>10} {:>15}",
                trim(&key, width),
                e.ref_count,
                crate::utils::pretty_bytes_size(retained),
            );
        }
    } else {
        let _ = writeln!(
            out,
            "  {:<width$} {:>10}",
            "holder.field (or class[] for arrays)", "ref count",
        );
        for e in rows {
            let key = match &e.field_name {
                None => format!("{}[]", e.holder_class),
                Some(f) => format!("{}.{f}", e.holder_class),
            };
            let _ = writeln!(out, "  {:<width$} {:>10}", trim(&key, width), e.ref_count,);
        }
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
            target: crate::args::ReferrersTarget::Exact(target.to_string()),
            hops,
            top: 10,
            include_statics: true,
            debug: false,
            json: false,
            json_out: None,
            preview_bytes: 0,
            retained_size: false,
            exclude_soft_weak: false,
            group_holders: false,
            mapping: crate::args::MappingOptions::default(),
        }
    }

    #[test]
    fn render_target_preview_includes_content_label() {
        let mut previews = AHashMap::new();
        previews.insert(
            42,
            crate::result_recorder::ArrayPreview {
                element_type: FieldType::Byte,
                bytes: br#"{"ok":true}"#.as_slice().into(),
                total_bytes: 11,
            },
        );

        let mut out = String::new();
        render_target_preview(&mut out, "id:42", &previews);

        assert!(out.contains("preview: content: JSON"), "got:\n{out}");
        assert!(out.contains(r#"{"ok":true}"#), "got:\n{out}");
    }

    #[test]
    fn symbolicate_referrer_entries_renames_holder_and_field() {
        let symbolicator = crate::mapping::Symbolicator::parse_text(
            std::path::Path::new("mapping.txt"),
            "com.example.Holder -> a.b:\n    java.lang.String title -> c\n",
        )
        .unwrap();
        let mut result = ReferrerResult {
            target_label: "a.b".to_string(),
            target_instance_count: 1,
            matched_classes: vec![MatchedClass {
                class_name: "a.b".to_string(),
                instance_count: 1,
            }],
            hop1: vec![ReferrerEntry {
                holder_class: "a.b".to_string(),
                field_name: Some("c".to_string()),
                ref_count: 3,
            }],
            hop2: vec![],
            hop3: vec![],
            grouped_holders: Vec::new(),
            array_previews: AHashMap::new(),
            class_retained_by_name: None,
        };

        result.symbolicate(&symbolicator);

        assert_eq!(result.target_label, "com.example.Holder");
        assert_eq!(result.matched_classes[0].class_name, "com.example.Holder");
        assert_eq!(result.hop1[0].holder_class, "com.example.Holder");
        assert_eq!(result.hop1[0].field_name.as_deref(), Some("title"));
    }

    #[test]
    fn glob_resolution_finds_multiple_matching_classes() {
        let idx = pass1_index("test-heap-dumps/hprof-64.bin", false).unwrap();
        let target = crate::args::ReferrersTarget::Glob("java.util.*".to_string());
        let (label, ids, matched) =
            resolve_target_ids("test-heap-dumps/hprof-64.bin", &idx, &target, false).unwrap();
        assert_eq!(label, "glob:java.util.*");
        assert!(
            matched.len() >= 5,
            "expected ≥5 java.util classes matched, got {}",
            matched.len()
        );
        assert!(
            !ids.is_empty(),
            "expected non-zero target instance count for java.util.* glob"
        );
    }

    #[test]
    fn glob_with_no_matches_errors() {
        let idx = pass1_index("test-heap-dumps/hprof-64.bin", false).unwrap();
        let target = crate::args::ReferrersTarget::Glob("nonexistent.does.not.exist.*".to_string());
        let res = resolve_target_ids("test-heap-dumps/hprof-64.bin", &idx, &target, false);
        match res {
            Err(HprofSlurpError::TargetClassNotFound { name }) => {
                assert!(name.contains("nonexistent"), "got: {name}");
            }
            other => panic!("expected TargetClassNotFound, got {other:?}"),
        }
    }

    #[test]
    fn pass1_indexes_thread_names_and_stack_frames() {
        let idx = pass1_index("test-heap-dumps/hprof-64.bin", false).unwrap();
        // The bundled JVM fixture has 10 StackTrace + 20 StackFrame records
        // (per test-heap-dumps/hprof-64-result.txt) but zero StartThread
        // records. So thread_name_by_serial may be empty for this fixture;
        // assert on what we know exists.
        assert!(
            !idx.stack_frame_by_id.is_empty(),
            "expected ≥1 stack frame, got {}",
            idx.stack_frame_by_id.len()
        );
        assert!(
            !idx.stack_trace_by_serial.is_empty(),
            "expected ≥1 stack trace, got {}",
            idx.stack_trace_by_serial.len()
        );
        assert!(
            idx.class_name_id_by_serial.len() >= 100,
            "expected ≥100 class serial entries (one per LoadClass), got {}",
            idx.class_name_id_by_serial.len()
        );
    }

    #[test]
    fn pass1_populates_v1_1_derivations_on_canonical_fixture() {
        let path = "JAVA_PROFILE_1.0.3.hprof";
        if !std::path::Path::new(path).exists() {
            eprintln!("skipping — fixture {path} not present");
            return;
        }
        let idx = pass1_index(path, false).expect("pass1");
        assert!(
            !idx.reference_subclass_set.is_empty(),
            "Android fixture should have loaded WeakReference; subclass set is empty"
        );
        // bitmap_class_info may or may not be present depending on whether
        // android.graphics.Bitmap was loaded; we don't assert presence.
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
        let (label, ids, matched) = resolve_target_ids(
            "test-heap-dumps/hprof-64.bin",
            &idx,
            &crate::args::ReferrersTarget::Exact("java.util.LinkedList".to_string()),
            false,
        )
        .unwrap();
        assert_eq!(label, "java.util.LinkedList");
        assert!(
            matched.is_empty(),
            "exact match should not populate matched_classes"
        );
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
