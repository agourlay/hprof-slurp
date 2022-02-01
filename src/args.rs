use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::*;
use clap::{App, Arg};
use std::path::Path;

fn app() -> clap::App<'static> {
    App::new("hprof-slurp")
        .version("0.2.2")
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
    let matches = app().get_matches();

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
    use crate::args::app;

    #[test]
    fn verify_app() {
        app().debug_assert();
    }
}
