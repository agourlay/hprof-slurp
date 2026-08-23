//! `hprof-slurp diff <FROM> <TO>` — per-class delta in instance count and
//! shallow bytes between two snapshots of the same process. Classes whose
//! footprint grew the most between two captures are the leak suspects a
//! single static dump can't reveal.

use std::cmp::Reverse;
use std::fmt::Write;

use ahash::AHashMap;
use serde::Serialize;

use crate::rendered_result::{ClassAllocationStats, DumpInfo, JSON_SCHEMA_VERSION, ToolInfo};
use crate::utils::{matches_class_filter, pretty_bytes_size, pretty_signed_bytes_size};

pub struct DiffEntry {
    pub class_name: String,
    pub instances_from: u64,
    pub instances_to: u64,
    pub bytes_from: u64,
    pub bytes_to: u64,
}

impl DiffEntry {
    pub fn delta_bytes(&self) -> i64 {
        self.bytes_to as i64 - self.bytes_from as i64
    }

    pub fn delta_instances(&self) -> i64 {
        self.instances_to as i64 - self.instances_from as i64
    }
}

// Sums stats per class name: the same class name can appear several times
// in a dump (same class loaded by multiple classloaders), each occurrence
// keyed by a different class id.
fn totals_by_class_name(stats: &[ClassAllocationStats]) -> AHashMap<&str, (u64, u64)> {
    let mut totals: AHashMap<&str, (u64, u64)> = AHashMap::new();
    for s in stats {
        let (instances, bytes) = totals.entry(s.class_name.as_str()).or_default();
        *instances += s.instance_count;
        *bytes += s.allocation_size_bytes;
    }
    totals
}

// Per-class deltas between two snapshots, sorted by shallow size growth.
// Classes with identical stats on both sides are omitted.
pub fn compute(from: &[ClassAllocationStats], to: &[ClassAllocationStats]) -> Vec<DiffEntry> {
    let from_by_name = totals_by_class_name(from);
    let to_by_name = totals_by_class_name(to);

    let mut class_names: Vec<&str> = from_by_name
        .keys()
        .chain(to_by_name.keys())
        .copied()
        .collect();
    class_names.sort_unstable();
    class_names.dedup();

    let mut entries: Vec<DiffEntry> = class_names
        .into_iter()
        .map(|class_name| {
            let (instances_from, bytes_from) =
                from_by_name.get(class_name).copied().unwrap_or_default();
            let (instances_to, bytes_to) = to_by_name.get(class_name).copied().unwrap_or_default();
            DiffEntry {
                class_name: class_name.to_string(),
                instances_from,
                instances_to,
                bytes_from,
                bytes_to,
            }
        })
        .filter(|e| e.delta_bytes() != 0 || e.delta_instances() != 0)
        .collect();

    entries.sort_by_key(|e| Reverse(e.delta_bytes()));
    entries
}

// Drops the deltas of the classes the filter does not select. The totals
// rendered next to them keep covering the whole dumps. A `None` filter is a
// no-op, so callers do not have to branch.
pub fn filter_entries(entries: &mut Vec<DiffEntry>, filter: Option<&str>) {
    entries.retain(|entry| matches_class_filter(&entry.class_name, filter));
}

pub fn render(
    from_label: &str,
    to_label: &str,
    from: &[ClassAllocationStats],
    to: &[ClassAllocationStats],
    entries: &[DiffEntry],
    top: usize,
    filter: Option<&str>,
) -> String {
    let total_from = total_shallow_bytes(from);
    let total_to = total_shallow_bytes(to);
    let net = total_to as i64 - total_from as i64;

    let mut out = String::new();
    let _ = writeln!(out, "\nHeap diff of raw shallow sizes:");
    let _ = writeln!(
        out,
        "  from: {from_label} ({})",
        pretty_bytes_size(total_from)
    );
    let _ = writeln!(out, "  to:   {to_label} ({})", pretty_bytes_size(total_to));
    let _ = writeln!(out, "  net:  {}", pretty_signed_bytes_size(net));

    if entries.is_empty() {
        match filter {
            Some(pattern) => {
                let _ = writeln!(
                    out,
                    "\nNo per-class differences matching '{pattern}' between the two dumps."
                );
            }
            None => {
                let _ = writeln!(out, "\nNo per-class differences between the two dumps.");
            }
        }
        return out;
    }

    if let Some(pattern) = filter {
        // The net above covers the whole dumps; this is the selection's own
        // growth, which is what `--fail-over` compares against.
        let _ = writeln!(
            out,
            "  filter: '{pattern}' ({} over {} classes)",
            pretty_signed_bytes_size(net_delta_bytes(entries)),
            entries.len()
        );
    }

    let shown = entries.len().min(top);
    let _ = writeln!(
        out,
        "\nTop {shown} of {} class deltas (by shallow size growth):\n",
        entries.len()
    );
    let _ = writeln!(
        out,
        "{:>12} {:>12} {:>23} {:>21}  Class name",
        "Δ size", "Δ instances", "size (from → to)", "instances (from → to)"
    );
    for entry in entries.iter().take(top) {
        let size_from_to = format!(
            "{} → {}",
            pretty_bytes_size(entry.bytes_from),
            pretty_bytes_size(entry.bytes_to)
        );
        let instances_from_to = format!("{} → {}", entry.instances_from, entry.instances_to);
        // explicit sign, like the size delta next to it
        let delta_instances = format!("{:+}", entry.delta_instances());
        let _ = writeln!(
            out,
            "{:>12} {:>12} {:>23} {:>21}  {}",
            pretty_signed_bytes_size(entry.delta_bytes()),
            delta_instances,
            size_from_to,
            instances_from_to,
            entry.class_name
        );
    }
    out
}

// Total shallow footprint of a whole dump, whatever the filter selects.
pub fn total_shallow_bytes(stats: &[ClassAllocationStats]) -> u64 {
    stats.iter().map(|s| s.allocation_size_bytes).sum()
}

// Net growth of the classes the report actually covers. With nothing filtered
// out this is the whole dump delta, since every class with a delta is listed;
// with a filter it is the growth of the selection, which is what a threshold
// on a filtered run has to gate. Gating the whole dump instead would fail a
// run because of growth the user explicitly filtered away.
pub fn net_delta_bytes(entries: &[DiffEntry]) -> i64 {
    entries.iter().map(DiffEntry::delta_bytes).sum()
}

// One side of the comparison as the caller has it: the dump metadata and the
// per class stats it was built from.
pub struct DumpSide<'a> {
    pub dump: DumpInfo,
    pub stats: &'a [ClassAllocationStats],
}

impl DumpSide<'_> {
    fn into_json(self) -> JsonDiffSide {
        JsonDiffSide {
            total_shallow_bytes: total_shallow_bytes(self.stats),
            class_count: self.stats.len(),
            dump: self.dump,
        }
    }
}

#[derive(Serialize)]
struct JsonDiffSide {
    #[serde(flatten)]
    dump: DumpInfo,
    total_shallow_bytes: u64,
    class_count: usize,
}

#[derive(Serialize)]
struct JsonDiffEntry {
    class_name: String,
    instances_from: u64,
    instances_to: u64,
    delta_instances: i64,
    bytes_from: u64,
    bytes_to: u64,
    delta_bytes: i64,
}

// An object rather than a bare string, so that the key has the same shape as
// `heap.filter` in the analysis document.
#[derive(Serialize)]
struct JsonDiffFilter {
    pattern: String,
}

#[derive(Serialize)]
struct JsonDiffBody {
    from: JsonDiffSide,
    to: JsonDiffSide,
    // over the reported deltas, so the threshold gates the number named here
    net_shallow_bytes_delta: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<JsonDiffFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fail_over_bytes: Option<u64>,
    over_threshold: bool,
    // number of differing classes, of which only the top ones are listed
    class_delta_count: usize,
    top_class_deltas: Vec<JsonDiffEntry>,
}

#[derive(Serialize)]
pub struct JsonDiffResult {
    schema_version: u32,
    tool: ToolInfo,
    diff: JsonDiffBody,
}

impl JsonDiffResult {
    pub fn new(
        from: DumpSide,
        to: DumpSide,
        entries: &[DiffEntry],
        top: usize,
        filter: Option<&str>,
        fail_over: Option<u64>,
    ) -> Self {
        let net_shallow_bytes_delta = net_delta_bytes(entries);
        Self {
            schema_version: JSON_SCHEMA_VERSION,
            tool: ToolInfo::current(),
            diff: JsonDiffBody {
                from: from.into_json(),
                to: to.into_json(),
                net_shallow_bytes_delta,
                filter: filter.map(|pattern| JsonDiffFilter {
                    pattern: pattern.to_string(),
                }),
                fail_over_bytes: fail_over,
                over_threshold: exceeds_threshold(net_shallow_bytes_delta, fail_over),
                class_delta_count: entries.len(),
                top_class_deltas: entries
                    .iter()
                    .take(top)
                    .map(|entry| JsonDiffEntry {
                        class_name: entry.class_name.clone(),
                        instances_from: entry.instances_from,
                        instances_to: entry.instances_to,
                        delta_instances: entry.delta_instances(),
                        bytes_from: entry.bytes_from,
                        bytes_to: entry.bytes_to,
                        delta_bytes: entry.delta_bytes(),
                    })
                    .collect(),
            },
        }
    }
}

// A shrinking heap never fails the threshold, and the comparison is strict so
// that `--fail-over 0` reports only actual growth.
pub fn exceeds_threshold(net_delta: i64, fail_over: Option<u64>) -> bool {
    fail_over.is_some_and(|limit| net_delta > 0 && net_delta.unsigned_abs() > limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slurp::slurp_file;

    fn stats(class_name: &str, instances: u64, bytes: u64) -> ClassAllocationStats {
        ClassAllocationStats::new(class_name.to_string(), instances, 0, bytes)
    }

    #[test]
    fn compute_reports_growth_shrinkage_added_and_removed() {
        let from = vec![
            stats("Grower", 10, 100),
            stats("Shrinker", 20, 200),
            stats("Stable", 5, 50),
            stats("Removed", 1, 10),
        ];
        let to = vec![
            stats("Grower", 30, 300),
            stats("Shrinker", 10, 100),
            stats("Stable", 5, 50),
            stats("Added", 2, 20),
        ];

        let entries = compute(&from, &to);

        // sorted by byte growth; Stable is omitted
        let names: Vec<&str> = entries.iter().map(|e| e.class_name.as_str()).collect();
        assert_eq!(names, vec!["Grower", "Added", "Removed", "Shrinker"]);

        let grower = &entries[0];
        assert_eq!(grower.delta_bytes(), 200);
        assert_eq!(grower.delta_instances(), 20);

        let removed = &entries[2];
        assert_eq!(removed.bytes_to, 0);
        assert_eq!(removed.delta_bytes(), -10);
    }

    #[test]
    fn render_reports_identical_dumps() {
        let from = vec![stats("Same", 1, 10)];
        let to = vec![stats("Same", 1, 10)];

        let entries = compute(&from, &to);
        let rendered = render("a.hprof", "b.hprof", &from, &to, &entries, 20, None);

        assert!(entries.is_empty());
        assert!(rendered.contains("No per-class differences"));
        assert!(rendered.contains("net:  +0.00bytes"));
    }

    #[test]
    fn render_signs_both_delta_columns() {
        let from = vec![stats("Shrinker", 20, 200)];
        let to = vec![stats("Shrinker", 8, 80)];

        let entries = compute(&from, &to);
        let rendered = render("a.hprof", "b.hprof", &from, &to, &entries, 20, None);

        assert!(rendered.contains("-120.00bytes"));
        assert!(rendered.contains("-12"));

        let entries = compute(&to, &from);
        let rendered = render("b.hprof", "a.hprof", &to, &from, &entries, 20, None);

        assert!(rendered.contains("+120.00bytes"));
        assert!(rendered.contains("+12"));
    }

    // Regression: the same class name appears once per classloader in real
    // dumps, and the recorder emits those duplicates in nondeterministic
    // order. Diffing a dump against itself used to report phantom deltas
    // because only one arbitrary duplicate per name was retained.
    #[test]
    fn duplicate_class_names_are_summed_per_side() {
        let from = vec![stats("Dup", 1, 10), stats("Dup", 2, 20)];
        let to = vec![stats("Dup", 2, 20), stats("Dup", 1, 10)];

        assert!(compute(&from, &to).is_empty());

        let grown = vec![stats("Dup", 1, 10), stats("Dup", 2, 30)];
        let entries = compute(&from, &grown);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].bytes_from, 30);
        assert_eq!(entries[0].bytes_to, 40);
        assert_eq!(entries[0].delta_bytes(), 10);
    }

    #[test]
    fn filter_entries_keeps_only_matching_class_names() {
        let from = vec![stats("com.example.Grower", 1, 10), stats("char[]", 1, 10)];
        let to = vec![stats("com.example.Grower", 2, 30), stats("char[]", 5, 50)];

        let mut entries = compute(&from, &to);
        assert_eq!(entries.len(), 2);
        filter_entries(&mut entries, Some("com.example"));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].class_name, "com.example.Grower");

        let rendered = render(
            "a.hprof",
            "b.hprof",
            &from,
            &to,
            &entries,
            20,
            Some("com.example"),
        );
        // the from/to/net totals keep covering the whole dumps
        assert!(rendered.contains("from: a.hprof (20.00bytes)"));
        assert!(rendered.contains("filter: 'com.example'"));
        assert!(rendered.contains("Top 1 of 1 class deltas"));
        assert!(!rendered.contains("char[]"));
    }

    #[test]
    fn render_reports_a_filter_matching_no_delta() {
        let from = vec![stats("char[]", 1, 10)];
        let to = vec![stats("char[]", 5, 50)];

        let mut entries = compute(&from, &to);
        filter_entries(&mut entries, Some("com.example"));
        let rendered = render(
            "a.hprof",
            "b.hprof",
            &from,
            &to,
            &entries,
            20,
            Some("com.example"),
        );

        assert!(rendered.contains("No per-class differences matching 'com.example'"));
    }

    #[test]
    fn threshold_only_trips_on_growth_over_the_limit() {
        // no threshold asked for
        assert!(!exceeds_threshold(1_000, None));
        // strictly over, so an exactly-at-the-limit growth passes
        assert!(!exceeds_threshold(100, Some(100)));
        assert!(exceeds_threshold(101, Some(100)));
        // a shrinking heap never fails, however far it shrank
        assert!(!exceeds_threshold(-1_000_000, Some(0)));
        assert!(!exceeds_threshold(0, Some(0)));
        assert!(exceeds_threshold(1, Some(0)));
    }

    #[test]
    fn json_diff_carries_totals_deltas_and_threshold() {
        let from = vec![stats("Grower", 1, 10), stats("Stable", 1, 5)];
        let to = vec![stats("Grower", 3, 40), stats("Stable", 1, 5)];
        let entries = compute(&from, &to);
        let dump = |name: &str| DumpInfo::new(name.to_string(), 1, "F".to_string(), 8, 0);

        let json_result = JsonDiffResult::new(
            DumpSide {
                dump: dump("a.hprof"),
                stats: &from,
            },
            DumpSide {
                dump: dump("b.hprof"),
                stats: &to,
            },
            &entries,
            20,
            None,
            Some(10),
        );
        let json = serde_json::to_value(&json_result).expect("should serialize");

        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["tool"]["name"], "hprof-slurp");
        // the dump metadata is flattened next to the per side totals
        assert_eq!(json["diff"]["from"]["file"], "a.hprof");
        assert_eq!(json["diff"]["from"]["total_shallow_bytes"], 15);
        assert_eq!(json["diff"]["to"]["total_shallow_bytes"], 45);
        // net over the reported deltas, which with no filter is the whole dump
        assert_eq!(json["diff"]["net_shallow_bytes_delta"], 30);
        assert_eq!(
            json["diff"]["to"]["total_shallow_bytes"].as_i64().unwrap()
                - json["diff"]["from"]["total_shallow_bytes"]
                    .as_i64()
                    .unwrap(),
            30
        );
        // 30 bytes of growth over a 10 byte budget
        assert_eq!(json["diff"]["fail_over_bytes"], 10);
        assert_eq!(json["diff"]["over_threshold"], true);
        // "Stable" is not a delta
        assert_eq!(json["diff"]["class_delta_count"], 1);
        let deltas = json["diff"]["top_class_deltas"]
            .as_array()
            .expect("should be an array");
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["class_name"], "Grower");
        assert_eq!(deltas[0]["delta_bytes"], 30);
        assert_eq!(deltas[0]["delta_instances"], 2);
        assert_eq!(deltas[0]["bytes_from"], 10);
    }

    // Regression: the threshold used to be computed on the whole dump, so a
    // filtered run failed on growth the user had explicitly filtered away.
    #[test]
    fn threshold_gates_the_filtered_selection_not_the_whole_dump() {
        let from = vec![stats("com.example.Small", 1, 10), stats("byte[]", 1, 1_000)];
        let to = vec![
            stats("com.example.Small", 1, 30),
            stats("byte[]", 1, 5_000_000),
        ];

        // unfiltered: the whole dump grew, so a small budget trips
        let entries = compute(&from, &to);
        assert_eq!(net_delta_bytes(&entries), 4_999_020);
        assert!(exceeds_threshold(net_delta_bytes(&entries), Some(1_000)));

        // filtered to a class that grew by 20 bytes: a 1KB budget must hold
        let mut entries = compute(&from, &to);
        filter_entries(&mut entries, Some("com.example"));
        assert_eq!(net_delta_bytes(&entries), 20);
        assert!(!exceeds_threshold(net_delta_bytes(&entries), Some(1_000)));

        let dump = |name: &str| DumpInfo::new(name.to_string(), 1, "F".to_string(), 8, 0);
        let json_result = JsonDiffResult::new(
            DumpSide {
                dump: dump("a"),
                stats: &from,
            },
            DumpSide {
                dump: dump("b"),
                stats: &to,
            },
            &entries,
            20,
            Some("com.example"),
            Some(1_000),
        );
        let json = serde_json::to_value(&json_result).expect("should serialize");

        // the threshold gates the number reported next to it
        assert_eq!(json["diff"]["net_shallow_bytes_delta"], 20);
        assert_eq!(json["diff"]["over_threshold"], false);
        // an object, like `heap.filter` in the analysis document
        assert_eq!(json["diff"]["filter"]["pattern"], "com.example");
        // the whole dump totals are still there, untouched by the filter
        assert_eq!(json["diff"]["to"]["total_shallow_bytes"], 5_000_030);

        // and the text report names the selection's own growth
        let rendered = render("a", "b", &from, &to, &entries, 20, Some("com.example"));
        assert!(rendered.contains("net:  +4.77MiB"));
        assert!(rendered.contains("filter: 'com.example' (+20.00bytes over 1 classes)"));
    }

    #[test]
    fn json_diff_omits_absent_filter_and_threshold() {
        let from = vec![stats("A", 1, 10)];
        let to = vec![stats("A", 2, 20)];
        let entries = compute(&from, &to);
        let dump = |name: &str| DumpInfo::new(name.to_string(), 1, "F".to_string(), 8, 0);

        let json_result = JsonDiffResult::new(
            DumpSide {
                dump: dump("a"),
                stats: &from,
            },
            DumpSide {
                dump: dump("b"),
                stats: &to,
            },
            &entries,
            20,
            None,
            None,
        );
        let json = serde_json::to_value(&json_result).expect("should serialize");

        assert!(json["diff"].get("filter").is_none());
        assert!(json["diff"].get("fail_over_bytes").is_none());
        assert_eq!(json["diff"]["over_threshold"], false);
    }

    // The listing is capped by --top while the count covers every delta, so a
    // consumer can tell a truncated list from a complete one.
    #[test]
    fn json_diff_reports_the_full_delta_count_with_a_capped_listing() {
        let from = vec![stats("A", 1, 10), stats("B", 1, 10), stats("C", 1, 10)];
        let to = vec![stats("A", 2, 20), stats("B", 2, 20), stats("C", 2, 20)];
        let entries = compute(&from, &to);
        let dump = |name: &str| DumpInfo::new(name.to_string(), 1, "F".to_string(), 8, 0);

        let json_result = JsonDiffResult::new(
            DumpSide {
                dump: dump("a"),
                stats: &from,
            },
            DumpSide {
                dump: dump("b"),
                stats: &to,
            },
            &entries,
            2,
            None,
            None,
        );
        let json = serde_json::to_value(&json_result).expect("should serialize");

        assert_eq!(json["diff"]["class_delta_count"], 3);
        assert_eq!(
            json["diff"]["top_class_deltas"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn diff_of_identical_dumps_is_empty() {
        let (_, from) = slurp_file("test-heap-dumps/hprof-32.bin", false, false).unwrap();
        let (_, to) = slurp_file("test-heap-dumps/hprof-32.bin", false, false).unwrap();

        assert!(compute(&from.memory_usage, &to.memory_usage).is_empty());
    }

    // End-to-end gold test pinning the full rendered diff of the two JVM
    // test dumps, like the gold tests of the analysis output.
    #[test]
    fn diff_of_different_dumps_matches_gold() {
        let from_path = "test-heap-dumps/hprof-32.bin";
        let to_path = "test-heap-dumps/hprof-64.bin";
        let (_, from) = slurp_file(from_path, false, false).unwrap();
        let (_, to) = slurp_file(to_path, false, false).unwrap();

        let entries = compute(&from.memory_usage, &to.memory_usage);
        let rendered = render(
            from_path,
            to_path,
            &from.memory_usage,
            &to.memory_usage,
            &entries,
            20,
            None,
        );

        let gold = std::fs::read_to_string("test-heap-dumps/hprof-diff-32-to-64-result.txt")
            .expect("gold file not found!");
        assert_eq!(rendered, gold);
    }
}
