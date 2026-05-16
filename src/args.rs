use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::{InputFileNotFound, InvalidTopPositiveInt};
use clap::{Arg, Command};
use clap::{crate_authors, crate_description, crate_name, crate_version};
use std::path::Path;

fn command() -> Command {
    Command::new(crate_name!())
        .version(crate_version!())
        .author(crate_authors!("\n"))
        .about(crate_description!())
        .arg(
            Arg::new("input-file")
                .help("binary hprof input file")
                .long("input-file")
                .short('i')
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
                .value_parser(clap::value_parser!(usize))
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
}

pub fn get_args() -> Result<Args, HprofSlurpError> {
    let matches = command().get_matches();

    let input_file = matches
        .get_one::<String>("input-file")
        .expect("impossible")
        .trim();
    if !Path::new(&input_file).is_file() {
        return Err(InputFileNotFound {
            name: input_file.to_string(),
        });
    }

    let top: usize = *matches.get_one("top").expect("impossible");
    if top == 0 {
        return Err(InvalidTopPositiveInt);
    }

    let debug = matches.get_flag("debug");
    let list_strings = matches.get_flag("list-strings");
    let json_output = matches.get_flag("json");
    let args = Args {
        file_path: input_file.to_string(),
        top,
        debug,
        list_strings,
        json_output,
    };
    Ok(args)
}

pub struct Args {
    pub file_path: String,
    pub top: usize,
    pub debug: bool,
    pub list_strings: bool,
    pub json_output: bool,
}

#[cfg(test)]
mod args_tests {
    use crate::args::command;

    #[test]
    fn verify_command() {
        command().debug_assert();
    }
}
