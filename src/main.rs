mod allocation_sites;
mod android_capture;
mod args;
mod bitmaps;
mod diff;
mod dominators;
mod errors;
mod leak_suspects;
mod mapping;
mod mapping_discovery;
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
mod series_diff;
mod slurp;
mod utils;

use std::time::Instant;

use clap::Parser;
use mapping::ResolvedMapping;
use rendered_result::JsonResult;
use serde::Serialize;

use crate::args::{Cli, MappingOptions, Mode, resolve};
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
            mapping,
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
            mapping,
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
            auto_mapping,
            project_root,
        } => {
            let mapping = if let Some(auto_mapping) = auto_mapping {
                crate::mapping::resolve_mapping(&crate::args::MappingOptions {
                    mapping: None,
                    auto_mapping: Some(auto_mapping),
                    project_root,
                    package: Some(package.clone()),
                    serial: serial.clone(),
                })?
            } else {
                None
            };
            let report = android_capture::run(android_capture::CaptureOptions {
                serial,
                package,
                out_dir: out_dir.into(),
                allocation_sites,
                foreground,
                mapping,
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

fn resolve_mapping_for_mode(mode: &Mode) -> Result<Option<ResolvedMapping>, HprofSlurpError> {
    match mode {
        Mode::Summary { mapping, .. }
        | Mode::FindReferrers { mapping, .. }
        | Mode::Paths { mapping, .. }
        | Mode::Diff { mapping, .. }
        | Mode::AllocationSites { mapping, .. }
        | Mode::LeakSuspects { mapping, .. }
        | Mode::Bitmaps { mapping, .. } => crate::mapping::resolve_mapping(mapping),
        Mode::AndroidCapture { .. } => Ok(None),
    }
}

fn print_mapping_notice(mapping: Option<&ResolvedMapping>) {
    if let Some(mapping) = mapping {
        println!("{}", mapping.notice());
    }
}

fn validate_hprof_input(path: &str) -> Result<(), HprofSlurpError> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() == 0 {
        return Err(HprofSlurpError::EmptyHprofInput {
            path: path.to_string(),
        });
    }
    Ok(())
}

fn validate_mode_inputs(mode: &Mode) -> Result<(), HprofSlurpError> {
    match mode {
        Mode::Summary { input_file, .. }
        | Mode::FindReferrers { input_file, .. }
        | Mode::Paths { input_file, .. }
        | Mode::AllocationSites { input_file, .. }
        | Mode::LeakSuspects { input_file, .. }
        | Mode::Bitmaps { input_file, .. } => validate_hprof_input(input_file),
        Mode::Diff { from, to, .. } => {
            validate_hprof_input(from)?;
            validate_hprof_input(to)
        }
        Mode::AndroidCapture { .. } => Ok(()),
    }
}

fn mode_hprof_label(mode: &Mode) -> &str {
    match mode {
        Mode::Summary { input_file, .. }
        | Mode::FindReferrers { input_file, .. }
        | Mode::Paths { input_file, .. }
        | Mode::AllocationSites { input_file, .. }
        | Mode::LeakSuspects { input_file, .. }
        | Mode::Bitmaps { input_file, .. } => input_file.as_str(),
        Mode::Diff { .. } => "diff input",
        Mode::AndroidCapture { .. } => "android capture",
    }
}

fn with_hprof_context<T>(
    path: &str,
    result: Result<T, HprofSlurpError>,
) -> Result<T, HprofSlurpError> {
    match result {
        Err(HprofSlurpError::StdIoError(source))
            if source.kind() == std::io::ErrorKind::UnexpectedEof =>
        {
            Err(HprofSlurpError::TruncatedHprofInput {
                path: path.to_string(),
                source,
            })
        }
        other => other,
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
    validate_mode_inputs(&mode)?;
    let resolved_mapping = resolve_mapping_for_mode(&mode)?;
    print_mapping_notice(resolved_mapping.as_ref());
    let mut result = with_hprof_context(mode_hprof_label(&mode), bitmaps::run(&mode))?;
    if let Some(mapping) = resolved_mapping.as_ref() {
        result.symbolicate(&mapping.symbolicator);
    }
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
    validate_mode_inputs(&mode)?;
    let resolved_mapping = resolve_mapping_for_mode(&mode)?;
    print_mapping_notice(resolved_mapping.as_ref());
    let mut result = with_hprof_context(mode_hprof_label(&mode), leak_suspects::run(&mode))?;
    if let Some(mapping) = resolved_mapping.as_ref() {
        result.symbolicate(&mapping.symbolicator);
    }
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
    validate_mode_inputs(&mode)?;
    let resolved_mapping = resolve_mapping_for_mode(&mode)?;
    print_mapping_notice(resolved_mapping.as_ref());
    let mut result = with_hprof_context(mode_hprof_label(&mode), allocation_sites::run(&mode))?;
    if let Some(mapping) = resolved_mapping.as_ref() {
        result.symbolicate(&mapping.symbolicator);
    }
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
    validate_mode_inputs(&mode)?;
    let resolved_mapping = resolve_mapping_for_mode(&mode)?;
    print_mapping_notice(resolved_mapping.as_ref());
    let mut result = with_hprof_context(mode_hprof_label(&mode), referrer::run(&mode))?;
    if let Some(mapping) = resolved_mapping.as_ref() {
        result.symbolicate(&mapping.symbolicator);
    }
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
    validate_mode_inputs(&mode)?;
    let resolved_mapping = resolve_mapping_for_mode(&mode)?;
    print_mapping_notice(resolved_mapping.as_ref());
    let mut entries = with_hprof_context("diff input", diff::run(&mode))?;
    if let Some(mapping) = resolved_mapping.as_ref() {
        diff::symbolicate_entries(&mut entries, &mapping.symbolicator);
    }
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
    validate_mode_inputs(&mode)?;
    if merge {
        let resolved_mapping = resolve_mapping_for_mode(&mode)?;
        print_mapping_notice(resolved_mapping.as_ref());
        let mut result = with_hprof_context(mode_hprof_label(&mode), merge_paths::run(&mode))?;
        if let Some(mapping) = resolved_mapping.as_ref() {
            result.symbolicate(&mapping.symbolicator);
        }
        if json {
            write_json_file(&result, json_out, "heaptrail-merge-paths")?;
        }
        print!("{}", merge_paths::render_text(&result));
        println!("\nFile successfully processed in {:?}", started.elapsed());
        return Ok(());
    }
    let resolved_mapping = resolve_mapping_for_mode(&mode)?;
    print_mapping_notice(resolved_mapping.as_ref());
    let mut result = with_hprof_context(mode_hprof_label(&mode), paths::run(&mode))?;
    if let Some(mapping) = resolved_mapping.as_ref() {
        result.symbolicate(&mapping.symbolicator);
    }
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
    mapping: MappingOptions,
    started: Instant,
) -> Result<(), HprofSlurpError> {
    validate_hprof_input(input_file)?;
    let resolved_mapping = crate::mapping::resolve_mapping(&mapping)?;
    print_mapping_notice(resolved_mapping.as_ref());
    let mut rendered_result = with_hprof_context(
        input_file,
        crate::slurp::slurp_file_with_modes(
            input_file,
            debug,
            list_strings,
            preview_bytes,
            list_arrays_min_bytes,
            retained_size,
            exclude_soft_weak,
        ),
    )?;
    if let Some(mapping) = resolved_mapping.as_ref() {
        rendered_result.symbolicate(&mapping.symbolicator);
    }
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

    #[test]
    fn validate_hprof_input_rejects_zero_byte_file() {
        let path = std::env::temp_dir().join(format!(
            "heaptrail-empty-{}.hprof",
            chrono::Utc::now().timestamp_millis()
        ));
        std::fs::write(&path, []).unwrap();

        let err = validate_hprof_input(path.to_str().unwrap()).unwrap_err();

        assert!(err.to_string().contains("input hprof is 0 bytes"));
    }

    #[test]
    fn with_hprof_context_rewrites_unexpected_eof() {
        let err = with_hprof_context::<()>(
            "broken.hprof",
            Err(HprofSlurpError::StdIoError(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "failed to fill whole buffer",
            ))),
        )
        .unwrap_err();

        assert!(err.to_string().contains("input hprof appears truncated"));
        assert!(err.to_string().contains("broken.hprof"));
    }
}
