mod args;
mod errors;
mod file_header_parser;
mod gc_record;
mod primitive_parsers;
mod record;
mod record_parser;
mod record_parser_iter;
mod result_recorder;
mod utils;
mod slurp;

use std::time::Instant;
use crate::args::get_args;
use crate::errors::HprofSlurpError;
use crate::slurp::slurp_file;

fn main() -> Result<(), HprofSlurpError> {
    let now = Instant::now();
    let (file_path, top, debug_mode, list_strings) = get_args()?;
    slurp_file(file_path, top, debug_mode, list_strings)?;
    println!("File successfully processed in {} seconds", now.elapsed().as_secs());
    Ok(())
}
