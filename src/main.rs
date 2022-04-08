mod args;
mod errors;
mod parser;
mod prefetch_reader;
mod result_recorder;
mod slurp;
mod utils;

use crate::args::get_args;
use crate::errors::HprofSlurpError;
use crate::slurp::slurp_file;
use std::time::Instant;

fn main() {
    std::process::exit(match main_result() {
        Ok(_) => 0,
        Err(err) => {
            eprintln!("error: {}", err);
            1
        }
    });
}

fn main_result() -> Result<(), HprofSlurpError> {
    let now = Instant::now();
    let (file_path, top, debug_mode, list_strings) = get_args()?;
    let rendered_result = slurp_file(file_path, top, debug_mode, list_strings)?;

    // Print results
    println!("{}", rendered_result.summary);
    println!("{}", rendered_result.analysis);
    if let Some(list_strings) = rendered_result.captured_strings {
        println!("{}", list_strings);
    }

    println!(
        "File successfully processed in {} seconds",
        now.elapsed().as_secs()
    );
    Ok(())
}
