use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::*;
use clap::{value_t, App, Arg};
use std::path::Path;

pub fn get_args() -> Result<(String, usize, bool, bool), HprofSlurpError> {
    let matches = App::new("hprof-slurp")
        .version("0.1.0")
        .author("Arnaud Gourlay <arnaud.gourlay@gmail.com>")
        .about("JVM heap dump hprof file analyzer")
        .arg(
            Arg::with_name("inputFile")
                .help("binary hprof input file")
                .long("inputFile")
                .short("i")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name("top")
                .help("the top results to display")
                .long("top")
                .short("t")
                .takes_value(true)
                .default_value("20")
                .required(false),
        )
        .arg(
            Arg::with_name("debug")
                .help("debug info")
                .long("debug")
                .short("d"),
        )
        .arg(
            Arg::with_name("listStrings")
                .help("list all Strings found")
                .long("listStrings")
                .short("ls"),
        )
        .get_matches();

    let input_file = matches.value_of("inputFile").expect("impossible");
    if !Path::new(input_file).is_file() {
        return Err(InputFileNotFound {
            name: input_file.to_string(),
        });
    }

    let top = value_t!(matches, "top", usize)?;
    if top == 0 {
        return Err(InvalidTopPositiveInt);
    }

    let debug = matches.is_present("debug");
    let list_strings = matches.is_present("listStrings");
    Ok((input_file.to_string(), top, debug, list_strings))
}
