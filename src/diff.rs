//! `--diff-from a.hprof --diff-to b.hprof` — per-class delta in instance
//! count and shallow bytes between two snapshots. The strongest churn
//! signal a single static dump can't give you: classes whose instance
//! count grew most between two captures of the same process are the
//! short-lived allocation hot-paths.

use ahash::{AHashMap, AHashSet};
use serde::Serialize;
use std::cmp::Reverse;

use crate::args::{DiffSort, Mode};
use crate::errors::HprofSlurpError;
use crate::rendered_result::ClassAllocationStats;
use crate::slurp::slurp_file;
use crate::utils::pretty_bytes_size;

#[derive(Serialize, Debug, Clone)]
pub struct DiffEntry {
    pub class_name: String,
    pub count_a: u64,
    pub count_b: u64,
    pub delta_count: i64,
    pub bytes_a: u64,
    pub bytes_b: u64,
    pub delta_bytes: i64,
}

pub fn run(mode: &Mode) -> Result<Vec<DiffEntry>, HprofSlurpError> {
    let (from, to, by, top) = match mode {
        Mode::Diff {
            from, to, by, top, ..
        } => (from.as_str(), to.as_str(), *by, *top),
        _ => {
            return Err(HprofSlurpError::NotYetImplemented {
                what: "diff::run only handles Mode::Diff",
            });
        }
    };
    let a = slurp_file(from, false, false)?;
    let b = slurp_file(to, false, false)?;
    let mut entries = compute(&a.memory_usage, &b.memory_usage, by);
    entries.truncate(top);
    Ok(entries)
}

pub fn compute(
    a: &[ClassAllocationStats],
    b: &[ClassAllocationStats],
    by: DiffSort,
) -> Vec<DiffEntry> {
    let a_map: AHashMap<&str, &ClassAllocationStats> =
        a.iter().map(|c| (c.class_name.as_str(), c)).collect();
    let b_map: AHashMap<&str, &ClassAllocationStats> =
        b.iter().map(|c| (c.class_name.as_str(), c)).collect();

    let mut keys: AHashSet<&str> = AHashSet::new();
    keys.extend(a_map.keys());
    keys.extend(b_map.keys());

    let mut out: Vec<DiffEntry> = keys
        .into_iter()
        .map(|k| {
            let av = a_map.get(k);
            let bv = b_map.get(k);
            let count_a = av.map_or(0, |c| c.instance_count);
            let count_b = bv.map_or(0, |c| c.instance_count);
            let bytes_a = av.map_or(0, |c| c.allocation_size_bytes);
            let bytes_b = bv.map_or(0, |c| c.allocation_size_bytes);
            DiffEntry {
                class_name: k.to_string(),
                count_a,
                count_b,
                delta_count: count_b as i64 - count_a as i64,
                bytes_a,
                bytes_b,
                delta_bytes: bytes_b as i64 - bytes_a as i64,
            }
        })
        .filter(|e| e.delta_count != 0 || e.delta_bytes != 0)
        .collect();

    match by {
        DiffSort::Count => out.sort_by_key(|e| Reverse(e.delta_count)),
        DiffSort::Bytes => out.sort_by_key(|e| Reverse(e.delta_bytes)),
    }
    out
}

pub fn render_text(entries: &[DiffEntry]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    if entries.is_empty() {
        let _ = writeln!(out, "\nNo per-class deltas — the two snapshots match.");
        return out;
    }
    let _ = writeln!(out, "\nClass deltas (sorted, top {} shown):", entries.len());
    let _ = writeln!(
        out,
        "  {:>12} {:>12} {:>12} {:>12}  class",
        "Δcount", "Δbytes", "count(a→b)", "bytes(a→b)"
    );
    for e in entries {
        let count_change = format!("{}→{}", e.count_a, e.count_b);
        let bytes_change = format!(
            "{}→{}",
            pretty_bytes_size(e.bytes_a),
            pretty_bytes_size(e.bytes_b)
        );
        let _ = writeln!(
            out,
            "  {:>+12} {:>+12} {:>12} {:>12}  {}",
            e.delta_count, e.delta_bytes, count_change, bytes_change, e.class_name
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs(name: &str, count: u64, bytes: u64) -> ClassAllocationStats {
        ClassAllocationStats::new(name.to_string(), count, 0, bytes)
    }

    #[test]
    fn compute_keys_join_and_sort_by_count() {
        let a = vec![cs("Foo", 10, 100), cs("Bar", 5, 50)];
        let b = vec![cs("Foo", 50, 500), cs("Bar", 5, 50), cs("Baz", 1, 10)];
        let entries = compute(&a, &b, DiffSort::Count);
        // Bar has zero delta — filtered.
        assert!(!entries.iter().any(|e| e.class_name == "Bar"));
        // Foo has +40 count, ranks first.
        assert_eq!(entries[0].class_name, "Foo");
        assert_eq!(entries[0].delta_count, 40);
        assert_eq!(entries[0].delta_bytes, 400);
        // Baz appears with +1 count, +10 bytes.
        assert!(
            entries
                .iter()
                .any(|e| e.class_name == "Baz" && e.delta_count == 1 && e.delta_bytes == 10)
        );
    }

    #[test]
    fn compute_with_class_only_in_baseline() {
        let a = vec![cs("Gone", 5, 200)];
        let b: Vec<ClassAllocationStats> = vec![];
        let entries = compute(&a, &b, DiffSort::Count);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].class_name, "Gone");
        assert_eq!(entries[0].delta_count, -5);
        assert_eq!(entries[0].delta_bytes, -200);
    }

    #[test]
    fn compute_sort_by_bytes_orders_correctly() {
        let a = vec![cs("Tiny", 1, 1), cs("Big", 1, 1)];
        let b = vec![cs("Tiny", 100, 100), cs("Big", 2, 1_000_000)];
        let entries = compute(&a, &b, DiffSort::Bytes);
        assert_eq!(entries[0].class_name, "Big");
    }

    #[test]
    fn diff_same_file_against_itself_is_empty() {
        let mode = Mode::Diff {
            from: "test-heap-dumps/hprof-64.bin".to_string(),
            to: "test-heap-dumps/hprof-64.bin".to_string(),
            by: DiffSort::Count,
            top: 30,
            json: false,
            json_out: None,
            mapping: crate::args::MappingOptions::default(),
        };
        let entries = run(&mode).unwrap();
        assert!(
            entries.is_empty(),
            "diffing a file against itself should be empty, got {entries:#?}"
        );
    }
}
