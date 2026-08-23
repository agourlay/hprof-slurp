//! Covers the wiring between parsed arguments and the rendered report. The unit
//! tests exercise argument parsing and rendering separately, so nothing else
//! catches an option that is parsed but never threaded through.

use hprof_slurp::args::{Args, DiffArgs};
use hprof_slurp::{analyze_file, diff_files};

const DUMP_32: &str = "test-heap-dumps/hprof-32.bin";
const DUMP_64: &str = "test-heap-dumps/hprof-64.bin";

fn analyze_args(file_path: &str, top: usize, filter: Option<&str>) -> Args {
    Args {
        file_path: file_path.to_string(),
        top,
        debug: false,
        list_strings: false,
        json_output: false,
        output_file: None,
        filter: filter.map(str::to_string),
    }
}

#[test]
fn analyze_threads_the_filter_into_the_report() {
    let unfiltered = analyze_file(analyze_args(DUMP_64, 20, None)).expect("should analyze");
    assert!(unfiltered.contains("java.lang.String"));
    assert!(unfiltered.contains("char[]"));
    assert!(!unfiltered.contains("Filter '"));

    let filtered =
        analyze_file(analyze_args(DUMP_64, 20, Some("java.util"))).expect("should analyze");
    assert!(filtered.contains("Filter 'java.util' matches"));
    assert!(filtered.contains("java.util.HashMap$Node"));
    assert!(!filtered.contains("| char[] "));
    // the dump wide banner is untouched by the filter
    assert!(filtered.contains("Found a total of 2.51MiB"));
}

#[test]
fn analyze_threads_top_into_the_report() {
    let report = analyze_file(analyze_args(DUMP_64, 3, None)).expect("should analyze");
    assert!(report.contains("Top 3 raw shallow heap classes:"));
    // 3 rows between the header and the closing rule of the first table
    let rows = report
        .lines()
        .filter(|line| line.starts_with("| ") && line.contains("KiB |"))
        .count();
    assert!(rows > 0 && rows <= 8, "unexpected row count {rows}");
}

#[test]
fn diff_threads_the_filter_into_the_report() {
    let diff_args = |filter: Option<&str>| DiffArgs {
        from: DUMP_32.to_string(),
        to: DUMP_64.to_string(),
        top: 20,
        filter: filter.map(str::to_string),
    };

    let unfiltered = diff_files(diff_args(None)).expect("should diff");
    assert!(unfiltered.contains("char[]"));
    assert!(!unfiltered.contains("filter:"));

    let filtered = diff_files(diff_args(Some("java.util"))).expect("should diff");
    assert!(filtered.contains("filter: 'java.util'"));
    assert!(filtered.contains("java.util.HashMap$Node"));
    assert!(!filtered.contains("char[]"));
    // the from/to totals keep covering the whole dumps
    assert!(filtered.contains("from: test-heap-dumps/hprof-32.bin (137.98KiB)"));
}

#[test]
fn diff_of_a_dump_against_itself_reports_no_difference() {
    let report = diff_files(DiffArgs {
        from: DUMP_64.to_string(),
        to: DUMP_64.to_string(),
        top: 20,
        filter: None,
    })
    .expect("should diff");

    assert!(report.contains("No per-class differences between the two dumps."));
}
