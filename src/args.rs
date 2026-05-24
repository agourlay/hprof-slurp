use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::{
    ConflictingModes, InputFileNotFound, InvalidTopPositiveInt, MissingInputFile,
};
use clap::{Parser, Subcommand};
use std::path::Path;

#[derive(Parser, Debug)]
#[command(
    name = "heaptrail",
    version,
    about = "JVM/Android heap dump (hprof) analyzer"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

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

    /// Write JSON output to this exact path. Requires --json. When omitted,
    /// --json keeps the current timestamped/default sidecar behavior.
    #[arg(long = "json-out", value_name = "PATH", requires = "json")]
    pub json_out: Option<String>,

    /// Optional Android dumpsys meminfo text to annotate diff-series reports.
    #[arg(long = "native-context", value_name = "PATH")]
    pub native_context: Option<String>,

    /// R8/ProGuard mapping file used to deobfuscate class and holder field names.
    #[arg(long = "mapping", value_name = "PATH", conflicts_with = "auto_mapping")]
    pub mapping: Option<String>,

    /// Discover the mapping file from a local Android Gradle project and an installed app version.
    #[arg(long = "auto-mapping", value_name = "MODE", num_args = 0..=1, default_missing_value = "strict")]
    pub auto_mapping: Option<AutoMappingMode>,

    /// Android project root for --auto-mapping.
    #[arg(long = "project-root", value_name = "DIR", requires = "auto_mapping")]
    pub project_root: Option<String>,

    /// Android package/application id for --auto-mapping.
    #[arg(long = "package", value_name = "PACKAGE", requires = "auto_mapping")]
    pub package: Option<String>,

    /// adb serial/device id for --auto-mapping.
    #[arg(long = "serial", value_name = "SERIAL")]
    pub serial: Option<String>,

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

    /// Group referrer rows by owner family, holder class, and field label.
    #[arg(long = "group-holders", default_value_t = false)]
    pub group_holders: bool,

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

    /// Ordered HProf snapshots for series diff. Requires at least 3 files.
    #[arg(long = "diff-series", value_name = "PATH", num_args = 1..)]
    pub diff_series: Vec<String>,

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
    /// edges (graph-theoretic dominator-tree definition); pair with
    /// `--exclude-soft-weak` for MAT-style leak hunting. Default off.
    #[arg(long = "retained-size", default_value_t = false)]
    pub retained_size: bool,

    /// Drop outgoing edges from java.lang.ref.{Soft,Weak,Phantom}Reference
    /// subclasses across path walks and the retained-size graph build.
    /// MAT's default leak-hunting filter; required on Android dumps
    /// where LeakCanary watchers and framework weak-refs would
    /// otherwise bury the real strong reference. Default off.
    #[arg(long = "exclude-soft-weak", default_value_t = false)]
    pub exclude_soft_weak: bool,

    /// Auto-rank dominators with retained share ≥ THRESHOLD; emit
    /// narrative + path-to-root + content preview per suspect.
    /// Implies --retained-size. Top-N suspects bounded by --top.
    /// Always shows at least top-3 (flagged "below threshold" if
    /// applicable). Default threshold 0.05 (5%).
    #[arg(long = "leak-suspects", value_name = "THRESHOLD", num_args = 0..=1, default_missing_value = "0.05")]
    pub leak_suspects: Option<f32>,

    /// Modifier on `--paths-from-id`. Fold paths-to-root for all
    /// instances of the start id's class into a single tree with
    /// branch counts. Pair with `--retained-size` for graph-verified
    /// convergence; otherwise textual prefix matching with a banner.
    #[arg(long = "merge-paths", default_value_t = false)]
    pub merge_paths: bool,

    /// List top-N android.graphics.Bitmap instances by pixel-byte
    /// size. Reports width × height × config and pixel bytes;
    /// Java-heap or native location; one-line holder summary.
    /// Android dumps only.
    #[arg(long = "bitmaps", default_value_t = false)]
    pub bitmaps: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Capture and validate an Android heap dump via adb.
    AndroidCapture(AndroidCaptureArgs),
}

#[derive(clap::Args, Debug)]
pub struct AndroidCaptureArgs {
    /// adb serial/device id. When omitted, adb's default target is used.
    #[arg(long = "serial", value_name = "SERIAL")]
    pub serial: Option<String>,

    /// Android application package name to capture.
    #[arg(long = "package", value_name = "PACKAGE")]
    pub package: String,

    /// Local output directory for the pulled hprof and transcript.
    #[arg(long = "out", value_name = "DIR")]
    pub out: String,

    /// Attempt allocation tracking setup before dump capture.
    #[arg(long = "allocation-sites", default_value_t = false)]
    pub allocation_sites: bool,

    /// Bring the package to foreground with `monkey -p <package> 1`.
    #[arg(long = "foreground", default_value_t = false)]
    pub foreground: bool,

    /// Number of validated HProf captures to collect for a diff-series run.
    #[arg(long = "series", value_name = "COUNT", default_value_t = 1)]
    pub series: usize,

    /// Seconds to wait between captures when --series is greater than 1.
    #[arg(
        long = "series-delay-seconds",
        value_name = "SECONDS",
        default_value_t = 0
    )]
    pub series_delay_seconds: u64,

    /// Discover the matching R8 mapping from a local Android Gradle project.
    #[arg(long = "auto-mapping", value_name = "MODE", num_args = 0..=1, default_missing_value = "strict")]
    pub auto_mapping: Option<AutoMappingMode>,

    /// Android project root for capture transcript mapping metadata.
    #[arg(long = "project-root", value_name = "DIR", requires = "auto_mapping")]
    pub project_root: Option<String>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSort {
    Count,
    Bytes,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoMappingMode {
    Strict,
    Optional,
}

#[derive(Debug, Clone, Default)]
pub struct MappingOptions {
    pub mapping: Option<String>,
    pub auto_mapping: Option<AutoMappingMode>,
    pub project_root: Option<String>,
    pub package: Option<String>,
    pub serial: Option<String>,
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
        json_out: Option<String>,
        preview_bytes: u32,
        list_arrays_min_bytes: u32,
        retained_size: bool,
        exclude_soft_weak: bool,
        mapping: MappingOptions,
    },
    FindReferrers {
        input_file: String,
        target: ReferrersTarget,
        hops: u8,
        top: usize,
        include_statics: bool,
        debug: bool,
        json: bool,
        json_out: Option<String>,
        preview_bytes: u32,
        retained_size: bool,
        exclude_soft_weak: bool,
        group_holders: bool,
        mapping: MappingOptions,
    },
    Paths {
        input_file: String,
        object_id: u64,
        max_depth: u8,
        debug: bool,
        json: bool,
        json_out: Option<String>,
        preview_bytes: u32,
        retained_size: bool,
        exclude_soft_weak: bool,
        merge_paths: bool,
        mapping: MappingOptions,
    },
    Diff {
        from: String,
        to: String,
        by: DiffSort,
        top: usize,
        json: bool,
        json_out: Option<String>,
        mapping: MappingOptions,
    },
    DiffSeries {
        inputs: Vec<String>,
        by: DiffSort,
        top: usize,
        json: bool,
        json_out: Option<String>,
        mapping: MappingOptions,
        native_context: Option<String>,
    },
    AllocationSites {
        input_file: String,
        top: usize,
        debug: bool,
        json: bool,
        json_out: Option<String>,
        mapping: MappingOptions,
    },
    LeakSuspects {
        input_file: String,
        top: usize,
        threshold: f32,
        exclude_soft_weak: bool,
        preview_bytes: u32,
        debug: bool,
        json: bool,
        json_out: Option<String>,
        mapping: MappingOptions,
    },
    Bitmaps {
        input_file: String,
        top: usize,
        debug: bool,
        json: bool,
        json_out: Option<String>,
        mapping: MappingOptions,
    },
    AndroidCapture {
        serial: Option<String>,
        package: String,
        out_dir: String,
        allocation_sites: bool,
        foreground: bool,
        series: usize,
        series_delay_seconds: u64,
        auto_mapping: Option<AutoMappingMode>,
        project_root: Option<String>,
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

    if let Some(command) = cli.command {
        return match command {
            Command::AndroidCapture(args) => {
                if args.series == 0 || args.series == 2 {
                    return Err(HprofSlurpError::AndroidCapture {
                        message: "--series must be 1 or at least 3".to_string(),
                    });
                }
                Ok(Mode::AndroidCapture {
                    serial: args.serial,
                    package: args.package,
                    out_dir: args.out,
                    allocation_sites: args.allocation_sites,
                    foreground: args.foreground,
                    series: args.series,
                    series_delay_seconds: args.series_delay_seconds,
                    auto_mapping: args.auto_mapping,
                    project_root: args.project_root,
                })
            }
        };
    }

    let mapping = mapping_options(&cli);

    let referrers_set = cli.find_referrers.is_some() || cli.target_glob.is_some();
    let paths_set = cli.paths_from_id.is_some();
    let diff_set = cli.diff_from.is_some() || cli.diff_to.is_some();
    let diff_series_set = !cli.diff_series.is_empty();
    let alloc_sites_set = cli.allocation_sites;
    let leak_suspects_set = cli.leak_suspects.is_some();
    let bitmaps_set = cli.bitmaps;

    let mode_count = [
        referrers_set,
        paths_set,
        diff_set,
        diff_series_set,
        alloc_sites_set,
        leak_suspects_set,
        bitmaps_set,
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if mode_count > 1 {
        return Err(ConflictingModes);
    }
    if cli.group_holders && !referrers_set {
        return Err(ConflictingModes);
    }

    if diff_series_set {
        if cli.diff_series.len() < 3 {
            return Err(HprofSlurpError::InvalidHprofFile {
                message: "--diff-series requires at least 3 HProf files".to_string(),
            });
        }
        for input in &cli.diff_series {
            check_file(input)?;
        }
        return Ok(Mode::DiffSeries {
            inputs: cli.diff_series,
            by: cli.diff_by,
            top: cli.top,
            json: cli.json,
            json_out: cli.json_out.clone(),
            mapping: mapping.clone(),
            native_context: cli.native_context.clone(),
        });
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
            json_out: cli.json_out.clone(),
            mapping: mapping.clone(),
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
            json_out: cli.json_out.clone(),
            mapping: mapping.clone(),
        });
    }

    if let Some(threshold) = cli.leak_suspects {
        return Ok(Mode::LeakSuspects {
            input_file,
            top: cli.top,
            threshold,
            exclude_soft_weak: cli.exclude_soft_weak,
            preview_bytes: cli.preview_bytes,
            debug: cli.debug,
            json: cli.json,
            json_out: cli.json_out.clone(),
            mapping: mapping.clone(),
        });
    }

    if cli.bitmaps {
        return Ok(Mode::Bitmaps {
            input_file,
            top: cli.top,
            debug: cli.debug,
            json: cli.json,
            json_out: cli.json_out.clone(),
            mapping: mapping.clone(),
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
            json_out: cli.json_out.clone(),
            preview_bytes: cli.preview_bytes,
            retained_size: cli.retained_size,
            exclude_soft_weak: cli.exclude_soft_weak,
            group_holders: cli.group_holders,
            mapping: mapping.clone(),
        });
    }
    if let Some(object_id) = cli.paths_from_id {
        return Ok(Mode::Paths {
            merge_paths: cli.merge_paths,
            input_file,
            object_id,
            max_depth: cli.max_depth,
            debug: cli.debug,
            json: cli.json,
            json_out: cli.json_out.clone(),
            preview_bytes: cli.preview_bytes,
            retained_size: cli.retained_size,
            exclude_soft_weak: cli.exclude_soft_weak,
            mapping: mapping.clone(),
        });
    }
    Ok(Mode::Summary {
        input_file,
        top: cli.top,
        debug: cli.debug,
        list_strings: cli.list_strings,
        json: cli.json,
        json_out: cli.json_out,
        preview_bytes: cli.preview_bytes,
        list_arrays_min_bytes: cli.list_arrays_min_bytes,
        retained_size: cli.retained_size,
        exclude_soft_weak: cli.exclude_soft_weak,
        mapping,
    })
}

fn mapping_options(cli: &Cli) -> MappingOptions {
    MappingOptions {
        mapping: cli.mapping.clone(),
        auto_mapping: cli.auto_mapping,
        project_root: cli.project_root.clone(),
        package: cli.package.clone(),
        serial: cli.serial.clone(),
    }
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
        assert!(cli.command.is_none());
        assert!(cli.find_referrers.is_none());
        assert!(cli.paths_from_id.is_none());
        assert!(cli.diff_from.is_none());
    }

    #[test]
    fn parses_android_capture_subcommand() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "android-capture",
            "--serial",
            "192.168.50.98:5555",
            "--package",
            "com.nexio.tv",
            "--out",
            "artifacts/run",
        ])
        .unwrap();

        match cli.command {
            Some(Command::AndroidCapture(args)) => {
                assert_eq!(args.serial.as_deref(), Some("192.168.50.98:5555"));
                assert_eq!(args.package, "com.nexio.tv");
                assert_eq!(args.out, "artifacts/run");
                assert!(!args.allocation_sites);
                assert!(!args.foreground);
            }
            other => panic!("expected android-capture, got {other:?}"),
        }
    }

    #[test]
    fn parses_android_capture_with_allocation_sites_and_foreground() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "android-capture",
            "--package",
            "com.example.app",
            "--out",
            "artifacts/run",
            "--allocation-sites",
            "--foreground",
        ])
        .unwrap();

        match cli.command {
            Some(Command::AndroidCapture(args)) => {
                assert!(args.allocation_sites);
                assert!(args.foreground);
            }
            other => panic!("expected android-capture, got {other:?}"),
        }
    }

    #[test]
    fn parses_android_capture_series_options() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "android-capture",
            "--package",
            "com.example.app",
            "--out",
            "artifacts/run",
            "--series",
            "3",
            "--series-delay-seconds",
            "5",
        ])
        .unwrap();

        match cli.command {
            Some(Command::AndroidCapture(args)) => {
                assert_eq!(args.series, 3);
                assert_eq!(args.series_delay_seconds, 5);
            }
            other => panic!("expected android-capture, got {other:?}"),
        }
    }

    #[test]
    fn rejects_android_capture_series_two() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "android-capture",
            "--package",
            "com.example.app",
            "--out",
            "artifacts/run",
            "--series",
            "2",
        ])
        .unwrap();

        let err = resolve(cli).unwrap_err();
        match err {
            HprofSlurpError::AndroidCapture { message } => {
                assert!(message.contains("--series must be 1 or at least 3"));
            }
            other => panic!("expected AndroidCapture, got {other:?}"),
        }
    }

    #[test]
    fn parses_preview_bytes() {
        let cli =
            Cli::try_parse_from(["heaptrail", "-i", "x.hprof", "--preview-bytes", "200"]).unwrap();
        assert_eq!(cli.preview_bytes, 200);
        assert_eq!(cli.list_arrays_min_bytes, 1024); // default
    }

    #[test]
    fn parses_json_out_with_json() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--json",
            "--json-out",
            "reports/summary.json",
        ])
        .unwrap();

        assert!(cli.json);
        assert_eq!(cli.json_out.as_deref(), Some("reports/summary.json"));
    }

    #[test]
    fn parses_manual_mapping_for_summary() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--mapping",
            "app/build/outputs/mapping/universalRelease/mapping.txt",
        ])
        .unwrap();

        assert_eq!(
            cli.mapping.as_deref(),
            Some("app/build/outputs/mapping/universalRelease/mapping.txt")
        );
        assert!(cli.auto_mapping.is_none());
    }

    #[test]
    fn parses_auto_mapping_options_for_analysis() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--auto-mapping",
            "--project-root",
            "/repo",
            "--package",
            "com.nexio.tv",
            "--serial",
            "device-1",
        ])
        .unwrap();

        assert_eq!(cli.auto_mapping, Some(AutoMappingMode::Strict));
        assert_eq!(cli.project_root.as_deref(), Some("/repo"));
        assert_eq!(cli.package.as_deref(), Some("com.nexio.tv"));
        assert_eq!(cli.serial.as_deref(), Some("device-1"));
    }

    #[test]
    fn parses_optional_auto_mapping() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--auto-mapping",
            "optional",
            "--project-root",
            "/repo",
            "--package",
            "com.nexio.tv",
        ])
        .unwrap();

        assert_eq!(cli.auto_mapping, Some(AutoMappingMode::Optional));
    }

    #[test]
    fn rejects_manual_and_auto_mapping_together() {
        let err = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--mapping",
            "mapping.txt",
            "--auto-mapping",
            "--project-root",
            "/repo",
            "--package",
            "com.nexio.tv",
        ])
        .unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn rejects_json_out_without_json() {
        let err = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--json-out",
            "reports/summary.json",
        ])
        .unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
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
    fn parses_group_holders_for_referrers() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--target-glob",
            "androidx.media3.**",
            "--group-holders",
        ])
        .unwrap();

        assert!(cli.group_holders);
    }

    #[test]
    fn rejects_group_holders_without_referrer_mode() {
        let cli = Cli::try_parse_from(["heaptrail", "-i", "x.hprof", "--group-holders"]).unwrap();
        let err = resolve(cli).unwrap_err();

        assert!(matches!(err, HprofSlurpError::ConflictingModes));
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
    fn parses_diff_series_mode() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "--diff-series",
            "launch.hprof",
            "play.hprof",
            "soak.hprof",
            "--diff-by",
            "bytes",
            "--json",
            "--json-out",
            "reports/series.json",
        ])
        .unwrap();

        assert_eq!(
            cli.diff_series,
            vec!["launch.hprof", "play.hprof", "soak.hprof"]
        );
        assert_eq!(cli.diff_by, DiffSort::Bytes);
        assert_eq!(cli.json_out.as_deref(), Some("reports/series.json"));
    }

    #[test]
    fn parses_native_context_for_diff_series() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "--diff-series",
            "a.hprof",
            "b.hprof",
            "c.hprof",
            "--native-context",
            "meminfo.txt",
        ])
        .unwrap();

        assert_eq!(cli.native_context.as_deref(), Some("meminfo.txt"));
    }

    #[test]
    fn diff_series_requires_three_inputs() {
        let cli =
            Cli::try_parse_from(["heaptrail", "--diff-series", "a.hprof", "b.hprof"]).unwrap();
        let err = resolve(cli).unwrap_err();

        assert!(
            err.to_string()
                .contains("--diff-series requires at least 3 HProf files"),
            "unexpected error: {err}"
        );
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
    fn resolve_carries_json_out_into_summary_mode() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "test-heap-dumps/hprof-64.bin",
            "--json",
            "--json-out",
            "reports/summary.json",
        ])
        .unwrap();

        match resolve(cli).unwrap() {
            Mode::Summary { json, json_out, .. } => {
                assert!(json);
                assert_eq!(json_out.as_deref(), Some("reports/summary.json"));
            }
            other => panic!("expected Summary, got {other:?}"),
        }
    }

    #[test]
    fn resolve_carries_json_out_into_diff_mode() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "--diff-from",
            "test-heap-dumps/hprof-64.bin",
            "--diff-to",
            "test-heap-dumps/hprof-64.bin",
            "--json",
            "--json-out",
            "reports/diff.json",
        ])
        .unwrap();

        match resolve(cli).unwrap() {
            Mode::Diff { json, json_out, .. } => {
                assert!(json);
                assert_eq!(json_out.as_deref(), Some("reports/diff.json"));
            }
            other => panic!("expected Diff, got {other:?}"),
        }
    }

    #[test]
    fn resolve_picks_android_capture_without_input_file() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "android-capture",
            "--package",
            "com.example.app",
            "--out",
            "artifacts/run",
        ])
        .unwrap();

        match resolve(cli).unwrap() {
            Mode::AndroidCapture {
                serial,
                package,
                out_dir,
                allocation_sites,
                foreground,
                ..
            } => {
                assert_eq!(serial, None);
                assert_eq!(package, "com.example.app");
                assert_eq!(out_dir, "artifacts/run");
                assert!(!allocation_sites);
                assert!(!foreground);
            }
            other => panic!("expected AndroidCapture, got {other:?}"),
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
