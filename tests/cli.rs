//! Covers the wiring between the command line and the rendered report by
//! running the real binary. The unit tests exercise argument parsing and
//! rendering separately, so nothing else catches an option that is parsed but
//! never threaded through, or an exit code that does not reach the shell.

use std::path::PathBuf;
use std::process::Command;

const DUMP_32: &str = "test-heap-dumps/hprof-32.bin";
const DUMP_64: &str = "test-heap-dumps/hprof-64.bin";

// Exit code of a run whose diff went over `--fail-over`.
const EXIT_OVER_THRESHOLD: i32 = 2;

struct Run {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

// Runs the binary Cargo built for this test, from the package root so the
// test dumps resolve by their relative paths.
fn hprof_slurp(args: &[&str]) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_hprof-slurp"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("the binary should start");
    Run {
        exit_code: output.status.code().expect("should exit, not be killed"),
        stdout: String::from_utf8(output.stdout).expect("stdout should be UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("stderr should be UTF-8"),
    }
}

// Like [`hprof_slurp`] for a run that is expected to succeed: returns stdout.
fn report(args: &[&str]) -> String {
    let run = hprof_slurp(args);
    assert_eq!(run.exit_code, 0, "{args:?} failed: {}", run.stderr);
    run.stdout
}

// A per-test path in the system temp dir, so parallel tests never share one.
fn temp_json(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "hprof-slurp-cli-{name}-{}.json",
        std::process::id()
    ))
}

#[test]
fn analyze_threads_the_filter_into_the_report() {
    let unfiltered = report(&[DUMP_64]);
    assert!(unfiltered.contains("java.lang.String"));
    assert!(unfiltered.contains("char[]"));
    assert!(!unfiltered.contains("Filter '"));

    let filtered = report(&[DUMP_64, "--filter", "java.util"]);
    assert!(filtered.contains("Filter 'java.util' matches"));
    assert!(filtered.contains("java.util.HashMap$Node"));
    assert!(!filtered.contains("| char[] "));
    // the dump wide banner is untouched by the filter
    assert!(filtered.contains("Found a total of 2.51MiB"));
}

#[test]
fn analyze_threads_top_into_the_report() {
    let report = report(&[DUMP_64, "--top", "3"]);
    assert!(report.contains("Top 3 raw shallow heap classes:"));
    // 3 rows between the header and the closing rule of the first table
    let rows = report
        .lines()
        .filter(|line| line.starts_with("| ") && line.contains("KiB |"))
        .count();
    assert!(rows > 0 && rows <= 8, "unexpected row count {rows}");
}

#[test]
fn analyze_keeps_diagnostics_off_stdout() {
    let run = hprof_slurp(&[DUMP_64]);
    assert_eq!(run.exit_code, 0);
    assert!(run.stderr.contains("Processing 3.07MiB binary hprof file"));
    assert!(!run.stdout.contains("Processing"));
}

#[test]
fn analyze_writes_the_json_document_it_was_asked_for() {
    let out_path = temp_json("analyze");
    report(&[
        DUMP_64,
        "--top",
        "2",
        "--json",
        "-o",
        out_path.to_str().unwrap(),
    ]);

    let written = std::fs::read_to_string(&out_path).expect("json file should exist");
    std::fs::remove_file(&out_path).ok();
    let json: serde_json::Value = serde_json::from_str(&written).expect("should be valid json");
    assert_eq!(json["dump"]["file"], DUMP_64);
    assert_eq!(
        json["heap"]["top_allocated_classes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn a_missing_input_file_fails_the_run() {
    let run = hprof_slurp(&["test-heap-dumps/does-not-exist.bin"]);
    assert_eq!(run.exit_code, 1);
    assert!(run.stderr.contains("not found"));
    assert!(run.stdout.is_empty());
}

#[test]
fn diff_threads_the_filter_into_the_report() {
    let unfiltered = report(&["diff", DUMP_32, DUMP_64]);
    assert!(unfiltered.contains("char[]"));
    assert!(!unfiltered.contains("filter:"));

    let filtered = report(&["diff", DUMP_32, DUMP_64, "--filter", "java.util"]);
    assert!(filtered.contains("filter: 'java.util' ("));
    assert!(filtered.contains("java.util.HashMap$Node"));
    assert!(!filtered.contains("char[]"));
    // the from/to totals keep covering the whole dumps
    assert!(filtered.contains("from: test-heap-dumps/hprof-32.bin (137.98KiB)"));
}

#[test]
fn diff_warns_when_the_dumps_use_different_id_sizes() {
    let mixed = report(&["diff", DUMP_32, DUMP_64, "--top", "3"]);
    assert!(mixed.ends_with(
        "\nWarning: the dumps use different identifier sizes (4 bytes in FROM, 8 bytes in TO).\nObject headers and references are sized differently on each side, so part of every delta comes from the pointer width and not from the heap contents.\n"
    ));

    let same = report(&["diff", DUMP_64, DUMP_64]);
    assert!(!same.contains("Warning:"));
}

#[test]
fn diff_turns_the_threshold_into_the_exit_code() {
    // the 64 bit dump is ~2.37MiB bigger than the 32 bit one
    let run = hprof_slurp(&["diff", DUMP_32, DUMP_64, "--fail-over", "1000000"]);
    assert_eq!(run.exit_code, EXIT_OVER_THRESHOLD);
    assert!(run.stderr.contains("over the --fail-over threshold"));
    // the report is still printed in full
    assert!(run.stdout.contains("Heap diff of raw shallow sizes:"));

    let run = hprof_slurp(&["diff", DUMP_32, DUMP_64, "--fail-over", "100MiB"]);
    assert_eq!(run.exit_code, 0);

    // no threshold asked for is never a failure
    let run = hprof_slurp(&["diff", DUMP_32, DUMP_64]);
    assert_eq!(run.exit_code, 0);

    // a shrinking heap does not trip it either
    let run = hprof_slurp(&["diff", DUMP_64, DUMP_32, "--fail-over", "0"]);
    assert_eq!(run.exit_code, 0);
}

#[test]
fn diff_writes_the_json_document_it_was_asked_for() {
    let out_path = temp_json("diff");
    let run = hprof_slurp(&[
        "diff",
        DUMP_32,
        DUMP_64,
        "--top",
        "3",
        "--filter",
        "java.util",
        "--json",
        "-o",
        out_path.to_str().unwrap(),
        "--fail-over",
        "0",
    ]);
    assert_eq!(run.exit_code, EXIT_OVER_THRESHOLD);

    let written = std::fs::read_to_string(&out_path).expect("json file should exist");
    std::fs::remove_file(&out_path).ok();
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
}

#[test]
fn diff_of_a_dump_against_itself_reports_no_difference() {
    let report = report(&["diff", DUMP_64, DUMP_64]);
    assert!(report.contains("No per-class differences between the two dumps."));
}
