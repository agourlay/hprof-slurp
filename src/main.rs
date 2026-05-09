mod args;
mod errors;
mod parser;
mod prefetch_reader;
mod referrer;
mod rendered_result;
mod result_recorder;
mod slurp;
mod utils;

use std::time::Instant;

use clap::Parser;
use rendered_result::JsonResult;

use crate::args::{Cli, Mode, resolve};
use crate::errors::HprofSlurpError;
use crate::slurp::slurp_file;

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
    let cli = Cli::parse();
    match resolve(cli)? {
        Mode::Summary {
            input_file,
            top,
            debug,
            list_strings,
            json,
        } => run_summary(&input_file, top, debug, list_strings, json, now),
        mode @ Mode::FindReferrers { .. } => run_find_referrers(mode, now),
        Mode::Paths { .. } => Err(HprofSlurpError::NotYetImplemented {
            what: "paths-from-id (Task 9 in plan)",
        }),
        Mode::Diff { .. } => Err(HprofSlurpError::NotYetImplemented {
            what: "diff-from / diff-to (Task 10 in plan)",
        }),
    }
}

fn run_find_referrers(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let json = match &mode {
        Mode::FindReferrers { json, .. } => *json,
        _ => unreachable!(),
    };
    let result = referrer::run(&mode)?;
    if json {
        let path = format!(
            "hprof-slurp-referrers-{}.json",
            chrono::Utc::now().timestamp_millis()
        );
        let f = std::fs::File::create(&path)?;
        serde_json::to_writer(std::io::BufWriter::new(f), &result)?;
        println!("Output JSON result file {path}");
    }
    print!("{}", referrer::render_text(&result));
    println!("\nFile successfully processed in {:?}", started.elapsed());
    Ok(())
}

fn run_summary(
    input_file: &str,
    top: usize,
    debug: bool,
    list_strings: bool,
    json: bool,
    started: Instant,
) -> Result<(), HprofSlurpError> {
    let mut rendered_result = slurp_file(input_file, debug, list_strings)?;
    if json {
        let json_result = JsonResult::new(&mut rendered_result.memory_usage, top);
        json_result.save_as_file()?;
    }
    print!("{}", rendered_result.serialize(top));
    println!("File successfully processed in {:?}", started.elapsed());
    Ok(())
}
