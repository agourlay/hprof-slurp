mod allocation_sites;
mod android_capture;
mod args;
mod bitmaps;
mod diff;
mod dominators;
mod errors;
mod leak_suspects;
mod merge_paths;
mod parser;
mod paths;
mod prefetch_reader;
mod preview;
mod reference_classes;
mod reference_graph;
mod referrer;
mod rendered_result;
mod result_recorder;
mod retained;
mod slurp;
mod utils;

use std::time::Instant;

use clap::Parser;
use rendered_result::JsonResult;
use serde::Serialize;

use crate::args::{Cli, Mode, resolve};
use crate::errors::HprofSlurpError;

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
            json_out,
            preview_bytes,
            list_arrays_min_bytes,
            retained_size,
            exclude_soft_weak,
        } => run_summary(
            &input_file,
            top,
            debug,
            list_strings,
            json,
            json_out,
            preview_bytes,
            list_arrays_min_bytes,
            retained_size,
            exclude_soft_weak,
            now,
        ),
        mode @ Mode::FindReferrers { .. } => run_find_referrers(mode, now),
        mode @ Mode::Paths { .. } => run_paths(mode, now),
        mode @ Mode::Diff { .. } => run_diff(mode, now),
        mode @ Mode::AllocationSites { .. } => run_allocation_sites(mode, now),
        mode @ Mode::LeakSuspects { .. } => run_leak_suspects(mode, now),
        mode @ Mode::Bitmaps { .. } => run_bitmaps(mode, now),
        Mode::AndroidCapture {
            serial,
            package,
            out_dir,
            allocation_sites,
            foreground,
        } => {
            let report = android_capture::run(android_capture::CaptureOptions {
                serial,
                package,
                out_dir: out_dir.into(),
                allocation_sites,
                foreground,
            })?;
            println!("Captured heap dump: {}", report.local_hprof.display());
            println!("Dump size: {} bytes", report.dump_size_bytes);
            println!(
                "AllocationSites present: {}",
                report.allocation_sites_present
            );
            println!("Transcript: {}", report.transcript.display());
            Ok(())
        }
    }
}

fn json_output_path(explicit_path: Option<&str>, default_prefix: &str) -> String {
    explicit_path.map_or_else(
        || {
            format!(
                "{default_prefix}-{}.json",
                chrono::Utc::now().timestamp_millis()
            )
        },
        str::to_string,
    )
}

fn write_json_file<T: Serialize>(
    value: &T,
    explicit_path: Option<&str>,
    default_prefix: &str,
) -> Result<(), HprofSlurpError> {
    let path = json_output_path(explicit_path, default_prefix);
    let file = std::fs::File::create(&path)?;
    serde_json::to_writer(std::io::BufWriter::new(file), value)?;
    println!("Output JSON result file {path}");
    Ok(())
}

fn run_bitmaps(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let (json, json_out) = match &mode {
        Mode::Bitmaps { json, json_out, .. } => (*json, json_out.as_deref()),
        _ => unreachable!(),
    };
    let result = bitmaps::run(&mode)?;
    if json {
        write_json_file(&result, json_out, "heaptrail-bitmaps")?;
    }
    print!("{}", bitmaps::render_text(&result));
    println!("\nFile successfully processed in {:?}", started.elapsed());
    Ok(())
}

fn run_leak_suspects(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let (json, json_out) = match &mode {
        Mode::LeakSuspects { json, json_out, .. } => (*json, json_out.as_deref()),
        _ => unreachable!(),
    };
    let result = leak_suspects::run(&mode)?;
    if json {
        write_json_file(&result, json_out, "heaptrail-leak-suspects")?;
    }
    print!("{}", leak_suspects::render_text(&result));
    println!("\nFile successfully processed in {:?}", started.elapsed());
    Ok(())
}

fn run_allocation_sites(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let (json, json_out) = match &mode {
        Mode::AllocationSites { json, json_out, .. } => (*json, json_out.as_deref()),
        _ => unreachable!(),
    };
    let result = allocation_sites::run(&mode)?;
    if json {
        write_json_file(&result, json_out, "heaptrail-allocation-sites")?;
    }
    print!("{}", allocation_sites::render_text(&result));
    println!("\nFile successfully processed in {:?}", started.elapsed());
    Ok(())
}

fn run_find_referrers(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let (json, json_out) = match &mode {
        Mode::FindReferrers { json, json_out, .. } => (*json, json_out.as_deref()),
        _ => unreachable!(),
    };
    let result = referrer::run(&mode)?;
    if json {
        write_json_file(&result, json_out, "heaptrail-referrers")?;
    }
    print!("{}", referrer::render_text(&result));
    println!("\nFile successfully processed in {:?}", started.elapsed());
    Ok(())
}

fn run_diff(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let (json, json_out) = match &mode {
        Mode::Diff { json, json_out, .. } => (*json, json_out.as_deref()),
        _ => unreachable!(),
    };
    let entries = diff::run(&mode)?;
    if json {
        write_json_file(&entries, json_out, "heaptrail-diff")?;
    }
    print!("{}", diff::render_text(&entries));
    println!("\nFile successfully processed in {:?}", started.elapsed());
    Ok(())
}

fn run_paths(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let (json, json_out, merge) = match &mode {
        Mode::Paths {
            json,
            json_out,
            merge_paths,
            ..
        } => (*json, json_out.as_deref(), *merge_paths),
        _ => unreachable!(),
    };
    if merge {
        let result = merge_paths::run(&mode)?;
        if json {
            write_json_file(&result, json_out, "heaptrail-merge-paths")?;
        }
        print!("{}", merge_paths::render_text(&result));
        println!("\nFile successfully processed in {:?}", started.elapsed());
        return Ok(());
    }
    let result = paths::run(&mode)?;
    if json {
        write_json_file(&result, json_out, "heaptrail-paths")?;
    }
    print!("{}", paths::render_text(&result));
    println!("\nFile successfully processed in {:?}", started.elapsed());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_summary(
    input_file: &str,
    top: usize,
    debug: bool,
    list_strings: bool,
    json: bool,
    json_out: Option<String>,
    preview_bytes: u32,
    list_arrays_min_bytes: u32,
    retained_size: bool,
    exclude_soft_weak: bool,
    started: Instant,
) -> Result<(), HprofSlurpError> {
    let mut rendered_result = crate::slurp::slurp_file_with_modes(
        input_file,
        debug,
        list_strings,
        preview_bytes,
        list_arrays_min_bytes,
        retained_size,
        exclude_soft_weak,
    )?;
    if json {
        let json_result = JsonResult::new(&mut rendered_result.memory_usage, top);
        write_json_file(&json_result, json_out.as_deref(), "heaptrail")?;
    }
    print!("{}", rendered_result.serialize(top));
    println!("File successfully processed in {:?}", started.elapsed());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_output_path_uses_explicit_path_when_present() {
        let actual = json_output_path(Some("reports/leaks.json"), "heaptrail-leak-suspects");
        assert_eq!(actual, "reports/leaks.json");
    }

    #[test]
    fn json_output_path_uses_timestamped_prefix_when_absent() {
        let actual = json_output_path(None, "heaptrail-diff");
        assert!(actual.starts_with("heaptrail-diff-"), "got {actual}");
        assert!(actual.ends_with(".json"), "got {actual}");
    }
}
