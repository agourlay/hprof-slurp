mod args;
mod errors;
mod parser;
mod prefetch_reader;
mod rendered_result;
mod result_recorder;
mod slurp;
mod utils;

use std::time::Instant;

use rendered_result::{DumpInfo, JsonResult};

use crate::args::Args;
use crate::args::get_args;
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
    let Args {
        file_path,
        top,
        debug,
        list_strings,
        json_output,
        output_file,
    } = get_args()?;
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
    println!("File successfully processed in {:?}", now.elapsed());
    Ok(())
}
