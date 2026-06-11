//! `hprof-slurp diff <FROM> <TO>` — per-class delta in instance count and
//! shallow bytes between two snapshots of the same process. Classes whose
//! footprint grew the most between two captures are the leak suspects a
//! single static dump can't reveal.

use std::cmp::Reverse;
use std::fmt::Write;

use ahash::AHashMap;

use crate::rendered_result::ClassAllocationStats;
use crate::utils::{pretty_bytes_size, pretty_signed_bytes_size};

pub struct DiffEntry {
    pub class_name: String,
    pub instances_from: u64,
    pub instances_to: u64,
    pub bytes_from: u64,
    pub bytes_to: u64,
}

impl DiffEntry {
    fn delta_bytes(&self) -> i64 {
        self.bytes_to as i64 - self.bytes_from as i64
    }

    fn delta_instances(&self) -> i64 {
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

pub fn render(
    from_label: &str,
    to_label: &str,
    from: &[ClassAllocationStats],
    to: &[ClassAllocationStats],
    entries: &[DiffEntry],
    top: usize,
) -> String {
    let total_from: u64 = from.iter().map(|s| s.allocation_size_bytes).sum();
    let total_to: u64 = to.iter().map(|s| s.allocation_size_bytes).sum();
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
        let _ = writeln!(out, "\nNo per-class differences between the two dumps.");
        return out;
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
        let _ = writeln!(
            out,
            "{:>12} {:>12} {:>23} {:>21}  {}",
            pretty_signed_bytes_size(entry.delta_bytes()),
            entry.delta_instances(),
            size_from_to,
            instances_from_to,
            entry.class_name
        );
    }
    out
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
        let rendered = render("a.hprof", "b.hprof", &from, &to, &entries, 20);

        assert!(entries.is_empty());
        assert!(rendered.contains("No per-class differences"));
        assert!(rendered.contains("net:  +0.00bytes"));
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
        );

        let gold = std::fs::read_to_string("test-heap-dumps/hprof-diff-32-to-64-result.txt")
            .expect("gold file not found!");
        assert_eq!(rendered, gold);
    }
}
