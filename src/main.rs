use std::time::Instant;

use hprof_slurp::args::{ParsedArgs, get_args};
use hprof_slurp::errors::HprofSlurpError;
use hprof_slurp::{analyze_file, diff_files};

// Distinct from the failure code so that a CI job can tell a heap that grew
// too much apart from a run that did not complete.
const EXIT_OVER_THRESHOLD: i32 = 2;

fn main() {
    std::process::exit(match main_result() {
        Ok(exit_code) => exit_code,
        Err(err) => {
            eprintln!("error: {err}");
            1
        }
    });
}

fn main_result() -> Result<i32, HprofSlurpError> {
    let now = Instant::now();
    match get_args()? {
        ParsedArgs::Analyze(args) => {
            print!("{}", analyze_file(args)?);
            println!("File successfully processed in {:?}", now.elapsed());
            Ok(0)
        }
        ParsedArgs::Diff(diff_args) => {
            let outcome = diff_files(diff_args)?;
            print!("{}", outcome.report);
            println!("Files successfully compared in {:?}", now.elapsed());
            if outcome.over_threshold {
                eprintln!("net shallow heap growth is over the --fail-over threshold");
                return Ok(EXIT_OVER_THRESHOLD);
            }
            Ok(0)
        }
    }
}
