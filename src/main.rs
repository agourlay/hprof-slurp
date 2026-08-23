use std::time::Instant;

use hprof_slurp::args::{ParsedArgs, get_args};
use hprof_slurp::errors::HprofSlurpError;
use hprof_slurp::{analyze_file, diff_files};

fn main() {
    std::process::exit(match main_result() {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("error: {err}");
            1
        }
    });
}

fn main_result() -> Result<(), HprofSlurpError> {
    let now = Instant::now();
    match get_args()? {
        ParsedArgs::Analyze(args) => {
            print!("{}", analyze_file(args)?);
            println!("File successfully processed in {:?}", now.elapsed());
        }
        ParsedArgs::Diff(diff_args) => {
            print!("{}", diff_files(diff_args)?);
            println!("Files successfully compared in {:?}", now.elapsed());
        }
    }
    Ok(())
}
