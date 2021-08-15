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

use std::fs::File;
use std::io::{BufReader, Read};

use indicatif::{ProgressBar, ProgressStyle};

use crate::args::get_args;
use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::*;
use crate::file_header_parser::parse_file_header;
use crate::record_parser::HprofRecordParser;
use crate::record_parser_iter::HprofRecordParserIter;
use crate::result_recorder::ResultRecorder;
use crate::utils::pretty_bytes_size;

fn main() -> Result<(), HprofSlurpError> {
    let (file_path, top, debug, list_strings) = get_args()?;

    let file = File::open(file_path)?;
    let meta = file.metadata()?;
    let file_len = meta.len() as usize;

    // Parse file header
    let mut reader = BufReader::new(file);
    let file_header_length = 31; // read the exact size of the file header (31 bytes)
    let mut header_buffer = vec![0; file_header_length];
    reader.read_exact(&mut header_buffer)?;
    let (rest, header) = parse_file_header(&header_buffer).unwrap();
    // Invariants
    let id_size = header.size_pointers;
    if id_size != 4 && id_size != 8 {
        return Err(InvalidIdSize);
    }
    if id_size == 4 {
        panic!("32 bits heap dumps are not supported yet")
    }
    if !rest.is_empty() {
        return Err(InvalidHeaderSize);
    }

    println!(
        "Processing {} binary hprof file in '{}' format.",
        pretty_bytes_size(file_len as u64),
        header.format
    );

    // Progress bar
    let pb = ProgressBar::new(file_len as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} (speed:{bytes_per_sec}) (eta:{eta})")
        .progress_chars("#>-"));

    // 64 MB buffer performs nicely (higher is faster but increases the memory consumption)
    let stream_buffer_size: usize = 64 * 1024 * 1024;

    // Init parser state
    let mut result_recorder = ResultRecorder::new_empty(id_size);
    let parser = HprofRecordParser::new(debug, id_size == 8);
    let parser_iter = HprofRecordParserIter::new(
        parser,
        reader,
        debug,
        file_len,
        file_header_length,
        stream_buffer_size,
    );

    // Pull data from the parser through the iterator
    parser_iter.for_each(|(processed, records)| {
        pb.set_position(processed as u64);
        // TODO handle on another thread via a channel to free parsing thread from expensive hashing
        result_recorder.record_records(records);
    });
    // Finish and remove progress bar
    pb.finish_and_clear();

    result_recorder.print_summary();
    result_recorder.print_analysis(top);

    if list_strings {
        result_recorder.print_strings()
    }

    Ok(())
}
