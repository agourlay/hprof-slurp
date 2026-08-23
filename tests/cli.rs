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
        json_output: false,
        output_file: None,
        fail_over: None,
    };

    let unfiltered = diff_files(diff_args(None)).expect("should diff").report;
    assert!(unfiltered.contains("char[]"));
    assert!(!unfiltered.contains("filter:"));

    let filtered = diff_files(diff_args(Some("java.util")))
        .expect("should diff")
        .report;
    assert!(filtered.contains("filter: 'java.util' ("));
    assert!(filtered.contains("java.util.HashMap$Node"));
    assert!(!filtered.contains("char[]"));
    // the from/to totals keep covering the whole dumps
    assert!(filtered.contains("from: test-heap-dumps/hprof-32.bin (137.98KiB)"));
}

#[test]
fn diff_threads_the_threshold_into_the_outcome() {
    let with_threshold = |from: &str, to: &str, fail_over: Option<u64>| DiffArgs {
        from: from.to_string(),
        to: to.to_string(),
        top: 20,
        filter: None,
        json_output: false,
        output_file: None,
        fail_over,
    };

    // the 64 bit dump is ~2.37MiB bigger than the 32 bit one
    let outcome =
        diff_files(with_threshold(DUMP_32, DUMP_64, Some(1_000_000))).expect("should diff");
    assert!(outcome.over_threshold);

    let outcome =
        diff_files(with_threshold(DUMP_32, DUMP_64, Some(100_000_000))).expect("should diff");
    assert!(!outcome.over_threshold);

    // no threshold asked for is never a failure
    let outcome = diff_files(with_threshold(DUMP_32, DUMP_64, None)).expect("should diff");
    assert!(!outcome.over_threshold);

    // a shrinking heap does not trip it either
    let outcome = diff_files(with_threshold(DUMP_64, DUMP_32, Some(0))).expect("should diff");
    assert!(!outcome.over_threshold);
}

#[test]
fn diff_writes_the_json_document_it_was_asked_for() {
    let out_dir = std::env::temp_dir().join("hprof-slurp-diff-json-test");
    std::fs::create_dir_all(&out_dir).expect("should create the output directory");
    let out_path = out_dir.join("diff.json");

    let outcome = diff_files(DiffArgs {
        from: DUMP_32.to_string(),
        to: DUMP_64.to_string(),
        top: 3,
        filter: Some("java.util".to_string()),
        json_output: true,
        output_file: Some(out_path.to_string_lossy().into_owned()),
        fail_over: Some(0),
    })
    .expect("should diff");
    assert!(outcome.over_threshold);

    let written = std::fs::read_to_string(&out_path).expect("json file should exist");
    let json: serde_json::Value = serde_json::from_str(&written).expect("should be valid json");

    assert_eq!(json["tool"]["name"], "hprof-slurp");
    assert_eq!(json["diff"]["from"]["file"], DUMP_32);
    assert_eq!(json["diff"]["to"]["file"], DUMP_64);
    // dump metadata comes along for free, which the text report never showed
    assert_eq!(json["diff"]["from"]["format"], "JAVA PROFILE 1.0.1");
    assert_eq!(json["diff"]["from"]["id_size_bytes"], 4);
    assert_eq!(json["diff"]["to"]["id_size_bytes"], 8);
    assert_eq!(json["diff"]["filter"]["pattern"], "java.util");
    // the threshold gates the filtered selection, not the whole dump
    let filtered_net = json["diff"]["net_shallow_bytes_delta"].as_i64().unwrap();
    let whole_dump_net = json["diff"]["to"]["total_shallow_bytes"].as_i64().unwrap()
        - json["diff"]["from"]["total_shallow_bytes"]
            .as_i64()
            .unwrap();
    assert!(
        filtered_net < whole_dump_net,
        "java.util grew less than the whole dump"
    );
    assert_eq!(json["diff"]["fail_over_bytes"], 0);
    assert_eq!(json["diff"]["over_threshold"], true);
    // the listing honours --top while the count covers every matching delta
    assert_eq!(
        json["diff"]["top_class_deltas"].as_array().unwrap().len(),
        3
    );
    assert!(json["diff"]["class_delta_count"].as_u64().unwrap() > 3);
    for entry in json["diff"]["top_class_deltas"].as_array().unwrap() {
        let name = entry["class_name"].as_str().unwrap();
        assert!(name.contains("java.util"), "unfiltered entry {name}");
    }

    std::fs::remove_file(&out_path).ok();
}

#[test]
fn diff_of_a_dump_against_itself_reports_no_difference() {
    let report = diff_files(DiffArgs {
        from: DUMP_64.to_string(),
        to: DUMP_64.to_string(),
        top: 20,
        filter: None,
        json_output: false,
        output_file: None,
        fail_over: None,
    })
    .expect("should diff")
    .report;

    assert!(report.contains("No per-class differences between the two dumps."));
}
