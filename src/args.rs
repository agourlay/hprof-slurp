use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::{
    ConflictingModes, InputFileNotFound, InvalidTopPositiveInt, MissingInputFile,
};
use clap::Parser;
use std::path::Path;

#[derive(Parser, Debug)]
#[command(
    name = "heaptrail",
    version,
    about = "JVM/Android heap dump (hprof) analyzer"
)]
pub struct Cli {
    /// Binary hprof input file. Required for summary, --find-referrers, and
    /// --paths-from-id modes. Not used in diff mode (see --diff-from/--diff-to).
    #[arg(short = 'i', long = "inputFile")]
    pub input_file: Option<String>,

    /// The top N results to display.
    #[arg(short = 't', long = "top", default_value_t = 20)]
    pub top: usize,

    /// Debug info.
    #[arg(short = 'd', long = "debug", default_value_t = false)]
    pub debug: bool,

    /// List all Strings found (summary mode).
    #[arg(short = 'l', long = "listStrings", default_value_t = false)]
    pub list_strings: bool,

    /// Additional JSON output.
    #[arg(long = "json", default_value_t = false)]
    pub json: bool,

    // -- referrer mode --
    /// Find direct + N-hop referrers of a target. Accepts an FQ class name
    /// (e.g. `java.util.ArrayList`) or `id:<u64>` / a bare `<u64>` for a
    /// specific object id.
    #[arg(long = "find-referrers", value_name = "TARGET")]
    pub find_referrers: Option<String>,

    /// Number of hops for referrer tracing. 1 = direct only, 2 = also through
    /// Object[] arrays, 3 = three-link chain.
    #[arg(long = "hops", default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=5))]
    pub hops: u8,

    /// Include class statics as candidate referrers.
    #[arg(long = "include-statics", default_value_t = true)]
    pub include_statics: bool,

    // -- paths mode --
    /// Trace holder chain from this object id toward a GC root.
    #[arg(long = "paths-from-id", value_name = "ID")]
    pub paths_from_id: Option<u64>,

    /// Maximum chain depth before giving up (paths mode).
    #[arg(long = "max-depth", default_value_t = 12)]
    pub max_depth: u8,

    // -- diff mode --
    /// Baseline (older) hprof for diff.
    #[arg(long = "diff-from", value_name = "PATH")]
    pub diff_from: Option<String>,

    /// Comparison (newer) hprof for diff.
    #[arg(long = "diff-to", value_name = "PATH")]
    pub diff_to: Option<String>,

    /// Diff sort key (count = delta instances, bytes = delta shallow size).
    #[arg(long = "diff-by", default_value = "count")]
    pub diff_by: DiffSort,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSort {
    Count,
    Bytes,
}

#[derive(Debug)]
pub enum Mode {
    Summary {
        input_file: String,
        top: usize,
        debug: bool,
        list_strings: bool,
        json: bool,
    },
    FindReferrers {
        input_file: String,
        target: String,
        hops: u8,
        top: usize,
        include_statics: bool,
        debug: bool,
        json: bool,
    },
    Paths {
        input_file: String,
        object_id: u64,
        max_depth: u8,
        debug: bool,
        json: bool,
    },
    Diff {
        from: String,
        to: String,
        by: DiffSort,
        top: usize,
        json: bool,
    },
}

/// Resolve the parsed CLI into a single concrete `Mode`. Enforces:
/// - exactly one of {summary, find-referrers, paths-from-id, diff} is selected
/// - the input file (or both diff files) exist
/// - `top > 0`
pub fn resolve(cli: Cli) -> Result<Mode, HprofSlurpError> {
    if cli.top == 0 {
        return Err(InvalidTopPositiveInt);
    }

    let referrers_set = cli.find_referrers.is_some();
    let paths_set = cli.paths_from_id.is_some();
    let diff_set = cli.diff_from.is_some() || cli.diff_to.is_some();

    let mode_count = [referrers_set, paths_set, diff_set]
        .iter()
        .filter(|b| **b)
        .count();
    if mode_count > 1 {
        return Err(ConflictingModes);
    }

    if diff_set {
        let from = cli.diff_from.ok_or(ConflictingModes)?;
        let to = cli.diff_to.ok_or(ConflictingModes)?;
        check_file(&from)?;
        check_file(&to)?;
        return Ok(Mode::Diff {
            from,
            to,
            by: cli.diff_by,
            top: cli.top,
            json: cli.json,
        });
    }

    let input_file = cli.input_file.ok_or(MissingInputFile)?;
    check_file(&input_file)?;

    if let Some(target) = cli.find_referrers {
        return Ok(Mode::FindReferrers {
            input_file,
            target,
            hops: cli.hops,
            top: cli.top,
            include_statics: cli.include_statics,
            debug: cli.debug,
            json: cli.json,
        });
    }
    if let Some(object_id) = cli.paths_from_id {
        return Ok(Mode::Paths {
            input_file,
            object_id,
            max_depth: cli.max_depth,
            debug: cli.debug,
            json: cli.json,
        });
    }
    Ok(Mode::Summary {
        input_file,
        top: cli.top,
        debug: cli.debug,
        list_strings: cli.list_strings,
        json: cli.json,
    })
}

fn check_file(p: &str) -> Result<(), HprofSlurpError> {
    if !Path::new(p).is_file() {
        return Err(InputFileNotFound {
            name: p.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod args_tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn verify_command() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_legacy_summary_invocation() {
        let cli = Cli::try_parse_from(["heaptrail", "-i", "x.hprof", "-t", "5"]).unwrap();
        assert_eq!(cli.input_file.as_deref(), Some("x.hprof"));
        assert_eq!(cli.top, 5);
        assert!(cli.find_referrers.is_none());
        assert!(cli.paths_from_id.is_none());
        assert!(cli.diff_from.is_none());
    }

    #[test]
    fn parses_find_referrers_with_hops() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--find-referrers",
            "java.util.ArrayList",
            "--hops",
            "3",
        ])
        .unwrap();
        assert_eq!(cli.find_referrers.as_deref(), Some("java.util.ArrayList"));
        assert_eq!(cli.hops, 3);
    }

    #[test]
    fn parses_paths_from_id() {
        let cli = Cli::try_parse_from(["heaptrail", "-i", "x.hprof", "--paths-from-id", "12345"])
            .unwrap();
        assert_eq!(cli.paths_from_id, Some(12345));
    }

    #[test]
    fn parses_diff_with_by_bytes() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "--diff-from",
            "a.hprof",
            "--diff-to",
            "b.hprof",
            "--diff-by",
            "bytes",
        ])
        .unwrap();
        assert_eq!(cli.diff_from.as_deref(), Some("a.hprof"));
        assert_eq!(cli.diff_to.as_deref(), Some("b.hprof"));
        assert_eq!(cli.diff_by, DiffSort::Bytes);
    }

    #[test]
    fn resolve_rejects_conflicting_modes() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--find-referrers",
            "Foo",
            "--paths-from-id",
            "1",
        ])
        .unwrap();
        let err = resolve(cli).unwrap_err();
        match err {
            HprofSlurpError::ConflictingModes => {}
            other => panic!("expected ConflictingModes, got {other:?}"),
        }
    }

    #[test]
    fn resolve_rejects_missing_input_file_in_summary() {
        let cli = Cli::try_parse_from(["heaptrail"]).unwrap();
        let err = resolve(cli).unwrap_err();
        match err {
            HprofSlurpError::MissingInputFile => {}
            other => panic!("expected MissingInputFile, got {other:?}"),
        }
    }

    #[test]
    fn resolve_picks_summary_for_existing_file() {
        let cli = Cli::try_parse_from(["heaptrail", "-i", "test-heap-dumps/hprof-64.bin"]).unwrap();
        match resolve(cli).unwrap() {
            Mode::Summary { input_file, .. } => {
                assert_eq!(input_file, "test-heap-dumps/hprof-64.bin");
            }
            other => panic!("expected Summary, got {other:?}"),
        }
    }

    #[test]
    fn resolve_picks_find_referrers() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "test-heap-dumps/hprof-64.bin",
            "--find-referrers",
            "java.util.LinkedList",
            "--hops",
            "1",
        ])
        .unwrap();
        match resolve(cli).unwrap() {
            Mode::FindReferrers { target, hops, .. } => {
                assert_eq!(target, "java.util.LinkedList");
                assert_eq!(hops, 1);
            }
            other => panic!("expected FindReferrers, got {other:?}"),
        }
    }
}
