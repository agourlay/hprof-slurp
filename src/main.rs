mod allocation_sites;
mod args;
mod diff;
mod errors;
mod parser;
mod paths;
mod prefetch_reader;
mod preview;
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
        mode @ Mode::Paths { .. } => run_paths(mode, now),
        mode @ Mode::Diff { .. } => run_diff(mode, now),
        mode @ Mode::AllocationSites { .. } => run_allocation_sites(mode, now),
    }
}

fn run_allocation_sites(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let json = match &mode {
        Mode::AllocationSites { json, .. } => *json,
        _ => unreachable!(),
    };
    let result = allocation_sites::run(&mode)?;
    if json {
        let path = format!(
            "heaptrail-allocation-sites-{}.json",
            chrono::Utc::now().timestamp_millis()
        );
        let f = std::fs::File::create(&path)?;
        serde_json::to_writer(std::io::BufWriter::new(f), &result)?;
        println!("Output JSON result file {path}");
    }
    print!("{}", allocation_sites::render_text(&result));
    println!("\nFile successfully processed in {:?}", started.elapsed());
    Ok(())
}

fn run_find_referrers(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let json = match &mode {
        Mode::FindReferrers { json, .. } => *json,
        _ => unreachable!(),
    };
    let result = referrer::run(&mode)?;
    if json {
        let path = format!(
            "heaptrail-referrers-{}.json",
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

fn run_diff(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let json = match &mode {
        Mode::Diff { json, .. } => *json,
        _ => unreachable!(),
    };
    let entries = diff::run(&mode)?;
    if json {
        let path = format!(
            "heaptrail-diff-{}.json",
            chrono::Utc::now().timestamp_millis()
        );
        let f = std::fs::File::create(&path)?;
        serde_json::to_writer(std::io::BufWriter::new(f), &entries)?;
        println!("Output JSON result file {path}");
    }
    print!("{}", diff::render_text(&entries));
    println!("\nFile successfully processed in {:?}", started.elapsed());
    Ok(())
}

fn run_paths(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let json = match &mode {
        Mode::Paths { json, .. } => *json,
        _ => unreachable!(),
    };
    let result = paths::run(&mode)?;
    if json {
        let path = format!(
            "heaptrail-paths-{}.json",
            chrono::Utc::now().timestamp_millis()
        );
        let f = std::fs::File::create(&path)?;
        serde_json::to_writer(std::io::BufWriter::new(f), &result)?;
        println!("Output JSON result file {path}");
    }
    print!("{}", paths::render_text(&result));
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
