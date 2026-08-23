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
use crate::errors::HprofSlurpError;
use crate::rendered_result::{DumpInfo, JsonResult};
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
        let file_size_bytes = std::fs::metadata(&file_path)?.len();
        let dump_info = DumpInfo::new(
            file_path,
            file_size_bytes,
            file_header.format,
            file_header.size_pointers,
            file_header.timestamp,
        );
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

// Compares two dumps and returns the rendered diff.
pub fn diff_files(diff_args: DiffArgs) -> Result<String, HprofSlurpError> {
    let DiffArgs {
        from,
        to,
        top,
        filter,
    } = diff_args;
    let (_, result_from) = slurp_file(&from, false, false)?;
    let (_, result_to) = slurp_file(&to, false, false)?;
    let mut entries = diff::compute(&result_from.memory_usage, &result_to.memory_usage);
    diff::filter_entries(&mut entries, filter.as_deref());
    Ok(diff::render(
        &from,
        &to,
        &result_from.memory_usage,
        &result_to.memory_usage,
        &entries,
        top,
        filter.as_deref(),
    ))
}
