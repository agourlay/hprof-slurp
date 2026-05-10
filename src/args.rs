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

    /// Show first N bytes/chars of primitive arrays in summary, paths,
    /// find-referrers id:N, and (with -l) the standalone-array list.
    /// Default 0 (off); recommended 200. UTF-8 / UTF-16 BE auto-detect
    /// with control-char escaping; falls back to xxd-style hex on
    /// binary content. See USERGUIDE §B.
    #[arg(long = "preview-bytes", value_name = "N", default_value_t = 0)]
    pub preview_bytes: u32,

    /// Minimum total byte size for a standalone primitive array to
    /// appear in `-l` output. Effective only when both `-l` and
    /// `--preview-bytes` are set. Default 1024.
    #[arg(long = "list-arrays-min-bytes", default_value_t = 1024)]
    pub list_arrays_min_bytes: u32,

    // -- referrer mode --
    /// Find direct + N-hop referrers of a target. Accepts an FQ class name
    /// (e.g. `java.util.ArrayList`) or `id:<u64>` / a bare `<u64>` for a
    /// specific object id.
    #[arg(long = "find-referrers", value_name = "TARGET")]
    pub find_referrers: Option<String>,

    /// Find direct + N-hop referrers of every class matching this glob.
    /// Mutually exclusive with `--find-referrers`. Glob syntax: `*` matches
    /// within a package level, `**` crosses package levels, `?` matches one
    /// character, `[abc]` is a class. See USERGUIDE §F.
    #[arg(
        long = "target-glob",
        value_name = "PATTERN",
        conflicts_with = "find_referrers"
    )]
    pub target_glob: Option<String>,

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

    // -- allocation-sites mode --
    /// Show per-class allocation sites with stack traces. Requires the
    /// dump to have been captured under allocation tracking
    /// (Android: `am profile start <pid>` before `am dumpheap`).
    #[arg(long = "allocation-sites", default_value_t = false)]
    pub allocation_sites: bool,

    /// Compute and surface retained sizes via Lengauer–Tarjan
    /// dominator tree. Annotates summary's class table (re-sorted by
    /// retained), `--paths-from-id` hops, and `--find-referrers`
    /// holders. Adds ~250 MiB working memory and ~1–3 s wall time
    /// on a 200 MiB Android dump. Includes weak/soft/phantom-reference
    /// edges (graph-theoretic dominator-tree definition); excluding
    /// those is a v1.1+ flag. Default off.
    #[arg(long = "retained-size", default_value_t = false)]
    pub retained_size: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSort {
    Count,
    Bytes,
}

/// Target source for `Mode::FindReferrers`. Either an exact FQ-name (or
/// `id:<u64>` / bare numeric id), or a shell-style glob over dotted FQ-names.
#[derive(Debug, Clone)]
pub enum ReferrersTarget {
    Exact(String),
    Glob(String),
}

#[derive(Debug)]
pub enum Mode {
    Summary {
        input_file: String,
        top: usize,
        debug: bool,
        list_strings: bool,
        json: bool,
        preview_bytes: u32,
        list_arrays_min_bytes: u32,
        retained_size: bool,
    },
    FindReferrers {
        input_file: String,
        target: ReferrersTarget,
        hops: u8,
        top: usize,
        include_statics: bool,
        debug: bool,
        json: bool,
        preview_bytes: u32,
        retained_size: bool,
    },
    Paths {
        input_file: String,
        object_id: u64,
        max_depth: u8,
        debug: bool,
        json: bool,
        preview_bytes: u32,
        retained_size: bool,
    },
    Diff {
        from: String,
        to: String,
        by: DiffSort,
        top: usize,
        json: bool,
    },
    AllocationSites {
        input_file: String,
        top: usize,
        debug: bool,
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

    let referrers_set = cli.find_referrers.is_some() || cli.target_glob.is_some();
    let paths_set = cli.paths_from_id.is_some();
    let diff_set = cli.diff_from.is_some() || cli.diff_to.is_some();
    let alloc_sites_set = cli.allocation_sites;

    let mode_count = [referrers_set, paths_set, diff_set, alloc_sites_set]
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

    if cli.allocation_sites {
        return Ok(Mode::AllocationSites {
            input_file,
            top: cli.top,
            debug: cli.debug,
            json: cli.json,
        });
    }

    let referrers_target = match (cli.find_referrers, cli.target_glob) {
        (Some(t), None) => Some(ReferrersTarget::Exact(t)),
        (None, Some(g)) => Some(ReferrersTarget::Glob(g)),
        (Some(_), Some(_)) => return Err(ConflictingModes),
        (None, None) => None,
    };
    if let Some(target) = referrers_target {
        return Ok(Mode::FindReferrers {
            input_file,
            target,
            hops: cli.hops,
            top: cli.top,
            include_statics: cli.include_statics,
            debug: cli.debug,
            json: cli.json,
            preview_bytes: cli.preview_bytes,
            retained_size: cli.retained_size,
        });
    }
    if let Some(object_id) = cli.paths_from_id {
        return Ok(Mode::Paths {
            input_file,
            object_id,
            max_depth: cli.max_depth,
            debug: cli.debug,
            json: cli.json,
            preview_bytes: cli.preview_bytes,
            retained_size: cli.retained_size,
        });
    }
    Ok(Mode::Summary {
        input_file,
        top: cli.top,
        debug: cli.debug,
        list_strings: cli.list_strings,
        json: cli.json,
        preview_bytes: cli.preview_bytes,
        list_arrays_min_bytes: cli.list_arrays_min_bytes,
        retained_size: cli.retained_size,
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
    fn parses_preview_bytes() {
        let cli =
            Cli::try_parse_from(["heaptrail", "-i", "x.hprof", "--preview-bytes", "200"]).unwrap();
        assert_eq!(cli.preview_bytes, 200);
        assert_eq!(cli.list_arrays_min_bytes, 1024); // default
    }

    #[test]
    fn parses_list_arrays_min_bytes() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--preview-bytes",
            "100",
            "--list-arrays-min-bytes",
            "4096",
        ])
        .unwrap();
        assert_eq!(cli.preview_bytes, 100);
        assert_eq!(cli.list_arrays_min_bytes, 4096);
    }

    #[test]
    fn parses_target_glob() {
        let cli = Cli::try_parse_from(["heaptrail", "-i", "x.hprof", "--target-glob", "com.foo.*"])
            .unwrap();
        assert_eq!(cli.target_glob.as_deref(), Some("com.foo.*"));
        assert!(cli.find_referrers.is_none());
    }

    #[test]
    fn target_glob_conflicts_with_find_referrers() {
        let res = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--find-referrers",
            "java.util.ArrayList",
            "--target-glob",
            "java.util.*",
        ]);
        assert!(res.is_err(), "clap should reject both flags together");
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
                match target {
                    ReferrersTarget::Exact(s) => assert_eq!(s, "java.util.LinkedList"),
                    ReferrersTarget::Glob(_) => panic!("expected Exact target"),
                }
                assert_eq!(hops, 1);
            }
            other => panic!("expected FindReferrers, got {other:?}"),
        }
    }
}
