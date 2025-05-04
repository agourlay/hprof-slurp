mod args;
mod errors;
mod parser;
mod prefetch_reader;
mod rendered_result;
mod result_recorder;
mod slurp;
mod utils;

use std::time::Instant;

use rendered_result::JsonResult;

use crate::args::get_args;
use crate::args::Args;
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
    } = get_args()?;
    let mut rendered_result = slurp_file(file_path, debug, list_strings)?;
    if json_output {
        // only memory usage rendered for now
        let json_result = JsonResult::new(&mut rendered_result.memory_usage, top);
        json_result.save_as_file()?;
    }
    print!("{}", rendered_result.serialize(top));
    println!("File successfully processed in {:?}", now.elapsed());
    Ok(())
}
