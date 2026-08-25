use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::InputFileNotFound;
use crate::utils::parse_bytes_size;
use clap::{Arg, Command};
use clap::{crate_authors, crate_description, crate_name, crate_version};
use std::path::Path;

fn top_arg() -> Arg {
    Arg::new("top")
        .help("the top results to display")
        .long("top")
        .short('t')
        .num_args(1)
        .default_value("20")
        .value_parser(clap::value_parser!(u64).range(1..))
        .required(false)
}

fn filter_arg() -> Arg {
    Arg::new("filter")
        .help("only report classes whose name contains this text")
        .long("filter")
        .short('f')
        .num_args(1)
        .value_name("PATTERN")
        .required(false)
}

fn json_arg() -> Arg {
    Arg::new("json")
        .help("additional JSON output in file")
        .long("json")
        .action(clap::ArgAction::SetTrue)
}

fn output_arg() -> Arg {
    Arg::new("output")
        .help("output file path for the JSON result (default: hprof-slurp-<timestamp>.json)")
        .long("output")
        .short('o')
        .num_args(1)
        .requires("json")
}

fn command() -> Command {
    Command::new(crate_name!())
        .version(crate_version!())
        .author(crate_authors!("\n"))
        .about(crate_description!())
        .subcommand_negates_reqs(true)
        .subcommand(
            Command::new("diff")
                .about("compare two dumps of the same process by per-class shallow heap deltas")
                .arg(
                    Arg::new("from")
                        .help("baseline hprof file")
                        .value_name("FROM")
                        .num_args(1)
                        .required(true),
                )
                .arg(
                    Arg::new("to")
                        .help("hprof file to compare against the baseline")
                        .value_name("TO")
                        .num_args(1)
                        .required(true),
                )
                .arg(top_arg())
                .arg(filter_arg())
                .arg(json_arg())
                .arg(output_arg())
                .arg(
                    Arg::new("fail-over")
                        .help(
                            "exit with code 2 when the net shallow heap growth exceeds this size (plain bytes, or a unit such as 10MiB)",
                        )
                        .long("fail-over")
                        .value_name("SIZE")
                        .num_args(1)
                        .value_parser(parse_bytes_size)
                        .required(false),
                ),
        )
        .arg(
            Arg::new("file")
                .help("binary hprof input file")
                .value_name("FILE")
                .num_args(1)
                .required(true),
        )
        .arg(top_arg())
        .arg(filter_arg())
        .arg(
            Arg::new("debug")
                .help("debug info")
                .long("debug")
                .short('d')
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("list-strings")
                .help("list all Strings found")
                .long("list-strings")
                .short('l')
                .action(clap::ArgAction::SetTrue),
        )
        .arg(json_arg())
        .arg(output_arg())
}

fn existing_file(raw_path: &str) -> Result<String, HprofSlurpError> {
    let path = raw_path.trim();
    if !Path::new(&path).is_file() {
        return Err(InputFileNotFound {
            name: path.to_string(),
        });
    }
    Ok(path.to_string())
}

fn get_top(matches: &clap::ArgMatches) -> usize {
    usize::try_from(*matches.get_one::<u64>("top").expect("impossible"))
        .expect("top should fit in usize")
}

pub fn get_args() -> Result<ParsedArgs, HprofSlurpError> {
    let matches = command().get_matches();

    if let Some(("diff", sub_matches)) = matches.subcommand() {
        let from = existing_file(sub_matches.get_one::<String>("from").expect("impossible"))?;
        let to = existing_file(sub_matches.get_one::<String>("to").expect("impossible"))?;
        let top = get_top(sub_matches);
        let filter = sub_matches.get_one::<String>("filter").cloned();
        let json_output = sub_matches.get_flag("json");
        let output_file = sub_matches.get_one::<String>("output").cloned();
        let fail_over = sub_matches.get_one::<u64>("fail-over").copied();
        return Ok(ParsedArgs::Diff(DiffArgs {
            from,
            to,
            top,
            filter,
            json_output,
            output_file,
            fail_over,
        }));
    }

    let file_path = existing_file(matches.get_one::<String>("file").expect("impossible"))?;
    let top = get_top(&matches);
    let debug = matches.get_flag("debug");
    let list_strings = matches.get_flag("list-strings");
    let json_output = matches.get_flag("json");
    let output_file = matches.get_one::<String>("output").cloned();
    let filter = matches.get_one::<String>("filter").cloned();
    let args = Args {
        file_path,
        top,
        debug,
        list_strings,
        json_output,
        output_file,
        filter,
    };
    Ok(ParsedArgs::Analyze(args))
}

pub enum ParsedArgs {
    Analyze(Args),
    Diff(DiffArgs),
}

pub struct Args {
    pub file_path: String,
    pub top: usize,
    pub debug: bool,
    pub list_strings: bool,
    pub json_output: bool,
    pub output_file: Option<String>,
    // only report classes whose name contains this text
    pub filter: Option<String>,
}

pub struct DiffArgs {
    pub from: String,
    pub to: String,
    pub top: usize,
    // only report classes whose name contains this text
    pub filter: Option<String>,
    pub json_output: bool,
    pub output_file: Option<String>,
    // net growth, in bytes, above which the run reports failure
    pub fail_over: Option<u64>,
}

#[cfg(test)]
mod args_tests {
    use crate::args::command;

    #[test]
    fn verify_command() {
        command().debug_assert();
    }

    #[test]
    fn accepts_positional_input_file() {
        let result = command().try_get_matches_from(["hprof-slurp", "f.hprof"]);
        assert!(result.is_ok());

        let result = command().try_get_matches_from(["hprof-slurp"]);
        assert!(result.is_err(), "input file should be required");
    }

    #[test]
    fn rejects_non_positive_top() {
        let result = command().try_get_matches_from(["hprof-slurp", "f.hprof", "-t", "0"]);
        assert!(result.is_err());
    }

    #[test]
    fn diff_subcommand_requires_two_files() {
        let result = command().try_get_matches_from(["hprof-slurp", "diff", "a.hprof", "b.hprof"]);
        assert!(result.is_ok());

        let result = command().try_get_matches_from(["hprof-slurp", "diff", "a.hprof"]);
        assert!(result.is_err(), "diff should require two files");

        let result = command().try_get_matches_from([
            "hprof-slurp",
            "diff",
            "a.hprof",
            "b.hprof",
            "-t",
            "5",
        ]);
        assert!(result.is_ok(), "diff should accept --top");
    }

    #[test]
    fn accepts_filter_on_both_commands() {
        let result =
            command().try_get_matches_from(["hprof-slurp", "f.hprof", "--filter", "com.example"]);
        assert_eq!(
            result.unwrap().get_one::<String>("filter"),
            Some(&"com.example".to_string())
        );

        let result = command().try_get_matches_from([
            "hprof-slurp",
            "diff",
            "a.hprof",
            "b.hprof",
            "-f",
            "com.example",
        ]);
        let matches = result.unwrap();
        let (_, sub_matches) = matches.subcommand().expect("diff subcommand");
        assert_eq!(
            sub_matches.get_one::<String>("filter"),
            Some(&"com.example".to_string())
        );
    }

    #[test]
    fn diff_accepts_json_output_and_threshold() {
        let matches = command()
            .try_get_matches_from([
                "hprof-slurp",
                "diff",
                "a.hprof",
                "b.hprof",
                "--json",
                "-o",
                "out.json",
                "--fail-over",
                "1048576",
            ])
            .expect("diff should accept json and threshold");
        let (_, sub_matches) = matches.subcommand().expect("diff subcommand");
        assert!(sub_matches.get_flag("json"));
        assert_eq!(
            sub_matches.get_one::<String>("output"),
            Some(&"out.json".to_string())
        );
        assert_eq!(sub_matches.get_one::<u64>("fail-over"), Some(&1_048_576));
    }

    #[test]
    fn diff_fail_over_accepts_a_unit_suffix() {
        let matches = command()
            .try_get_matches_from([
                "hprof-slurp",
                "diff",
                "a.hprof",
                "b.hprof",
                "--fail-over",
                "10MiB",
            ])
            .expect("diff should accept a threshold with a unit");
        let (_, sub_matches) = matches.subcommand().expect("diff subcommand");
        assert_eq!(sub_matches.get_one::<u64>("fail-over"), Some(&10_485_760));
    }

    #[test]
    fn diff_fail_over_rejects_an_unknown_unit() {
        let result = command().try_get_matches_from([
            "hprof-slurp",
            "diff",
            "a.hprof",
            "b.hprof",
            "--fail-over",
            "10potatoes",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn diff_output_requires_json() {
        let result = command().try_get_matches_from([
            "hprof-slurp",
            "diff",
            "a.hprof",
            "b.hprof",
            "-o",
            "out.json",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn output_requires_json() {
        let result = command().try_get_matches_from(["hprof-slurp", "f.hprof", "-o", "out.json"]);
        assert!(result.is_err());

        let result =
            command().try_get_matches_from(["hprof-slurp", "f.hprof", "--json", "-o", "out.json"]);
        assert!(result.is_ok());
    }
}
