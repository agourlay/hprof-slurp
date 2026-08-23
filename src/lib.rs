//! Library side of `hprof-slurp`, so that the report building can be driven
//! from tests instead of only from the binary.

pub mod args;
pub mod diff;
pub mod errors;
pub mod parser;
pub mod prefetch_reader;
pub mod rendered_result;
pub mod result_recorder;
pub mod slurp;
pub mod utils;

use crate::args::{Args, DiffArgs};
use crate::diff::{DumpSide, JsonDiffResult};
use crate::errors::HprofSlurpError;
use crate::parser::file_header_parser::FileHeader;
use crate::rendered_result::{DumpInfo, JsonResult, save_as_json_file};
use crate::slurp::slurp_file;

// Analyses a dump and returns the rendered report. The JSON file, when asked
// for, is written as a side effect before the report is returned.
pub fn analyze_file(args: Args) -> Result<String, HprofSlurpError> {
    let Args {
        file_path,
        top,
        debug,
        list_strings,
        json_output,
        output_file,
        filter,
    } = args;
    let (file_header, mut rendered_result) = slurp_file(&file_path, debug, list_strings)?;
    if json_output {
        // only dump metadata and memory usage rendered for now
        let dump_info = dump_info(&file_path, file_header)?;
        let json_result = JsonResult::new(
            dump_info,
            &mut rendered_result.memory_usage,
            top,
            filter.as_deref(),
        );
        json_result.save_as_file(output_file.as_deref())?;
    }
    Ok(rendered_result.serialize(top, filter.as_deref()))
}

// What a comparison produced: the rendered diff, plus whether the net growth
// went over the `--fail-over` threshold so the caller can pick an exit code.
pub struct DiffOutcome {
    pub report: String,
    pub over_threshold: bool,
}

// Compares two dumps and returns the rendered diff. The JSON file, when asked
// for, is written as a side effect before the report is returned.
pub fn diff_files(diff_args: DiffArgs) -> Result<DiffOutcome, HprofSlurpError> {
    let DiffArgs {
        from,
        to,
        top,
        filter,
        json_output,
        output_file,
        fail_over,
    } = diff_args;
    let (from_header, result_from) = slurp_file(&from, false, false)?;
    let (to_header, result_to) = slurp_file(&to, false, false)?;
    let mut entries = diff::compute(&result_from.memory_usage, &result_to.memory_usage);
    diff::filter_entries(&mut entries, filter.as_deref());

    // over the filtered entries, so a filtered run is gated on its selection
    let over_threshold = diff::exceeds_threshold(diff::net_delta_bytes(&entries), fail_over);

    if json_output {
        let json_result = JsonDiffResult::new(
            DumpSide {
                dump: dump_info(&from, from_header)?,
                stats: &result_from.memory_usage,
            },
            DumpSide {
                dump: dump_info(&to, to_header)?,
                stats: &result_to.memory_usage,
            },
            &entries,
            top,
            filter.as_deref(),
            fail_over,
        );
        save_as_json_file(&json_result, output_file.as_deref())?;
    }

    let report = diff::render(
        &from,
        &to,
        &result_from.memory_usage,
        &result_to.memory_usage,
        &entries,
        top,
        filter.as_deref(),
    );
    Ok(DiffOutcome {
        report,
        over_threshold,
    })
}

fn dump_info(file_path: &str, header: FileHeader) -> Result<DumpInfo, HprofSlurpError> {
    let file_size_bytes = std::fs::metadata(file_path)?.len();
    Ok(DumpInfo::new(
        file_path.to_string(),
        file_size_bytes,
        header.format,
        header.size_pointers,
        header.timestamp,
    ))
}
