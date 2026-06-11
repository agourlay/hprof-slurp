mod args;
mod diff;
mod errors;
mod parser;
mod prefetch_reader;
mod rendered_result;
mod result_recorder;
mod slurp;
mod utils;

use std::time::Instant;

use rendered_result::{DumpInfo, JsonResult};

use crate::args::{Args, DiffArgs, ParsedArgs, get_args};
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
    match get_args()? {
        ParsedArgs::Analyze(args) => {
            analyze_file(args)?;
            println!("File successfully processed in {:?}", now.elapsed());
        }
        ParsedArgs::Diff(diff_args) => {
            diff_files(diff_args)?;
            println!("Files successfully compared in {:?}", now.elapsed());
        }
    }
    Ok(())
}

fn analyze_file(args: Args) -> Result<(), HprofSlurpError> {
    let Args {
        file_path,
        top,
        debug,
        list_strings,
        json_output,
        output_file,
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
        let json_result = JsonResult::new(dump_info, &mut rendered_result.memory_usage, top);
        json_result.save_as_file(output_file.as_deref())?;
    }
    print!("{}", rendered_result.serialize(top));
    Ok(())
}

fn diff_files(diff_args: DiffArgs) -> Result<(), HprofSlurpError> {
    let DiffArgs { from, to, top } = diff_args;
    let (_, result_from) = slurp_file(&from, false, false)?;
    let (_, result_to) = slurp_file(&to, false, false)?;
    let entries = diff::compute(&result_from.memory_usage, &result_to.memory_usage);
    print!(
        "{}",
        diff::render(
            &from,
            &to,
            &result_from.memory_usage,
            &result_to.memory_usage,
            &entries,
            top
        )
    );
    Ok(())
}
