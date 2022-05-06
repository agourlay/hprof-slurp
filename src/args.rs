use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::*;
use clap::{Arg, Command};
use std::path::Path;

fn command() -> clap::Command<'static> {
    Command::new("hprof-slurp")
        .version("0.4.1")
        .author("Arnaud Gourlay <arnaud.gourlay@gmail.com>")
        .about("JVM heap dump hprof file analyzer")
        .arg(
            Arg::new("inputFile")
                .help("binary hprof input file")
                .long("inputFile")
                .short('i')
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::new("top")
                .help("the top results to display")
                .long("top")
                .short('t')
                .takes_value(true)
                .default_value("20")
                .required(false),
        )
        .arg(
            Arg::new("debug")
                .help("debug info")
                .long("debug")
                .short('d'),
        )
        .arg(
            Arg::new("listStrings")
                .help("list all Strings found")
                .long("listStrings")
                .short('l'),
        )
}

pub fn get_args() -> Result<(String, usize, bool, bool), HprofSlurpError> {
    let matches = command().get_matches();

    let input_file = matches.value_of("inputFile").expect("impossible").trim();
    if !Path::new(input_file).is_file() {
        return Err(InputFileNotFound {
            name: input_file.to_string(),
        });
    }

    let top = matches.value_of_t("top")?;
    if top == 0 {
        return Err(InvalidTopPositiveInt);
    }

    let debug = matches.is_present("debug");
    let list_strings = matches.is_present("listStrings");
    Ok((input_file.to_string(), top, debug, list_strings))
}

#[cfg(test)]
mod args_tests {
    use crate::args::command;

    #[test]
    fn verify_command() {
        command().debug_assert();
    }
}
