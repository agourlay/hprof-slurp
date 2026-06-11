use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::InputFileNotFound;
use clap::{Arg, Command};
use clap::{crate_authors, crate_description, crate_name, crate_version};
use std::path::Path;

fn command() -> Command {
    Command::new(crate_name!())
        .version(crate_version!())
        .author(crate_authors!("\n"))
        .about(crate_description!())
        .arg(
            Arg::new("file")
                .help("binary hprof input file")
                .value_name("FILE")
                .num_args(1)
                .required(true),
        )
        .arg(
            Arg::new("top")
                .help("the top results to display")
                .long("top")
                .short('t')
                .num_args(1)
                .default_value("20")
                .value_parser(clap::value_parser!(u64).range(1..))
                .required(false),
        )
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
        .arg(
            Arg::new("json")
                .help("additional JSON output in file")
                .long("json")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("output")
                .help(
                    "output file path for the JSON result (default: hprof-slurp-<timestamp>.json)",
                )
                .long("output")
                .short('o')
                .num_args(1)
                .requires("json"),
        )
}

pub fn get_args() -> Result<Args, HprofSlurpError> {
    let matches = command().get_matches();

    let input_file = matches
        .get_one::<String>("file")
        .expect("impossible")
        .trim();
    if !Path::new(&input_file).is_file() {
        return Err(InputFileNotFound {
            name: input_file.to_string(),
        });
    }

    let top = usize::try_from(*matches.get_one::<u64>("top").expect("impossible"))
        .expect("top should fit in usize");

    let debug = matches.get_flag("debug");
    let list_strings = matches.get_flag("list-strings");
    let json_output = matches.get_flag("json");
    let output_file = matches.get_one::<String>("output").cloned();
    let args = Args {
        file_path: input_file.to_string(),
        top,
        debug,
        list_strings,
        json_output,
        output_file,
    };
    Ok(args)
}

pub struct Args {
    pub file_path: String,
    pub top: usize,
    pub debug: bool,
    pub list_strings: bool,
    pub json_output: bool,
    pub output_file: Option<String>,
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
    fn output_requires_json() {
        let result = command().try_get_matches_from(["hprof-slurp", "f.hprof", "-o", "out.json"]);
        assert!(result.is_err());

        let result =
            command().try_get_matches_from(["hprof-slurp", "f.hprof", "--json", "-o", "out.json"]);
        assert!(result.is_ok());
    }
}
