use ahash::{AHashMap, AHashSet};
use serde::Serialize;
use std::cmp::Reverse;

use crate::args::DiffSort;
use crate::diff::DiffEntry;
use crate::rendered_result::ClassAllocationStats;

#[derive(Serialize, Debug, Clone)]
pub struct SeriesSnapshot {
    pub index: usize,
    pub path: String,
    pub total_shallow_bytes: u64,
    pub class_count: usize,
}

#[derive(Serialize, Debug, Clone)]
pub struct SeriesClassRow {
    pub class_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obfuscated_class_name: Option<String>,
    pub counts: Vec<u64>,
    pub bytes: Vec<u64>,
    pub total_delta_count: i64,
    pub total_delta_bytes: i64,
}

#[derive(Serialize, Debug, Clone)]
pub struct SeriesStep {
    pub from_index: usize,
    pub to_index: usize,
    pub deltas: Vec<DiffEntry>,
}

#[derive(Serialize, Debug, Clone)]
pub struct DiffSeriesReport {
    pub snapshots: Vec<SeriesSnapshot>,
    pub steps: Vec<SeriesStep>,
    pub classes: Vec<SeriesClassRow>,
    pub monotonic_growth: Vec<SeriesClassRow>,
}

pub fn compute_from_rollups(
    paths: &[String],
    rollups: &[Vec<ClassAllocationStats>],
    by: DiffSort,
) -> DiffSeriesReport {
    let snapshots = rollups
        .iter()
        .enumerate()
        .map(|(index, classes)| SeriesSnapshot {
            index,
            path: paths[index].clone(),
            total_shallow_bytes: classes.iter().map(|c| c.allocation_size_bytes).sum(),
            class_count: classes.len(),
        })
        .collect::<Vec<_>>();

    let steps = rollups
        .windows(2)
        .enumerate()
        .map(|(from_index, pair)| SeriesStep {
            from_index,
            to_index: from_index + 1,
            deltas: crate::diff::compute(&pair[0], &pair[1], by),
        })
        .collect::<Vec<_>>();

    let mut names = AHashSet::new();
    for classes in rollups {
        for class in classes {
            names.insert(class.class_name.clone());
        }
    }

    let per_snapshot = rollups
        .iter()
        .map(|classes| {
            classes
                .iter()
                .map(|c| (c.class_name.as_str(), c))
                .collect::<AHashMap<_, _>>()
        })
        .collect::<Vec<_>>();

    let mut classes = names
        .into_iter()
        .map(|class_name| {
            let mut counts = Vec::with_capacity(rollups.len());
            let mut bytes = Vec::with_capacity(rollups.len());
            for snapshot in &per_snapshot {
                let row = snapshot.get(class_name.as_str());
                counts.push(row.map_or(0, |c| c.instance_count));
                bytes.push(row.map_or(0, |c| c.allocation_size_bytes));
            }
            let total_delta_count = counts.last().copied().unwrap_or(0) as i64
                - counts.first().copied().unwrap_or(0) as i64;
            let total_delta_bytes = bytes.last().copied().unwrap_or(0) as i64
                - bytes.first().copied().unwrap_or(0) as i64;
            SeriesClassRow {
                class_name,
                obfuscated_class_name: None,
                counts,
                bytes,
                total_delta_count,
                total_delta_bytes,
            }
        })
        .collect::<Vec<_>>();

    sort_class_rows(&mut classes, by);
    let mut monotonic_growth = classes
        .iter()
        .filter(|row| is_monotonic_growth(row, by))
        .cloned()
        .collect::<Vec<_>>();
    sort_class_rows(&mut monotonic_growth, by);

    DiffSeriesReport {
        snapshots,
        steps,
        classes,
        monotonic_growth,
    }
}

fn is_monotonic_growth(row: &SeriesClassRow, by: DiffSort) -> bool {
    let values = match by {
        DiffSort::Count => &row.counts,
        DiffSort::Bytes => &row.bytes,
    };
    values.windows(2).all(|pair| pair[1] >= pair[0])
        && values.last().copied().unwrap_or(0) > values.first().copied().unwrap_or(0)
}

fn sort_class_rows(rows: &mut [SeriesClassRow], by: DiffSort) {
    match by {
        DiffSort::Count => rows.sort_by_key(|r| Reverse(r.total_delta_count)),
        DiffSort::Bytes => rows.sort_by_key(|r| Reverse(r.total_delta_bytes)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs(name: &str, count: u64, bytes: u64) -> ClassAllocationStats {
        ClassAllocationStats::new(name.to_string(), count, 0, bytes)
    }

    #[test]
    fn detects_monotonic_growth_by_bytes() {
        let paths = vec![
            "launch.hprof".into(),
            "play.hprof".into(),
            "soak.hprof".into(),
        ];
        let rollups = vec![
            vec![cs("Decoder", 1, 100), cs("Temp", 10, 500)],
            vec![cs("Decoder", 2, 200), cs("Temp", 2, 100)],
            vec![cs("Decoder", 3, 350), cs("Temp", 8, 300)],
        ];

        let report = compute_from_rollups(&paths, &rollups, DiffSort::Bytes);

        assert_eq!(report.snapshots.len(), 3);
        assert_eq!(report.steps.len(), 2);
        assert_eq!(report.monotonic_growth.len(), 1);
        assert_eq!(report.monotonic_growth[0].class_name, "Decoder");
        assert_eq!(report.monotonic_growth[0].bytes, vec![100, 200, 350]);
        assert_eq!(report.monotonic_growth[0].total_delta_bytes, 250);
    }

    #[test]
    fn excludes_flat_or_decreasing_classes_from_monotonic_growth() {
        let paths = vec!["a.hprof".into(), "b.hprof".into(), "c.hprof".into()];
        let rollups = vec![
            vec![cs("Flat", 1, 100), cs("Drop", 3, 300)],
            vec![cs("Flat", 1, 100), cs("Drop", 2, 200)],
            vec![cs("Flat", 1, 100), cs("Drop", 4, 400)],
        ];

        let report = compute_from_rollups(&paths, &rollups, DiffSort::Bytes);

        assert!(report.monotonic_growth.is_empty());
    }
}
