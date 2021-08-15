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

use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};

use crate::args::get_args;
use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::*;
use crate::file_header_parser::parse_file_header;
use crate::record::Record;
use crate::record_parser::HprofRecordParser;
use crate::record_parser_iter::HprofRecordParserIter;
use crate::result_recorder::ResultRecorder;
use crate::utils::pretty_bytes_size;

fn main() -> Result<(), HprofSlurpError> {
    let (file_path, top, debug_mode, list_strings) = get_args()?;

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
    let stream_buffer_size = 64 * 1024 * 1024;

    // Init parser state
    let result_recorder = ResultRecorder::new(id_size, list_strings, top);
    let parser = HprofRecordParser::new(debug_mode);
    let parser_iter = HprofRecordParserIter::new(
        parser,
        reader,
        debug_mode,
        file_len,
        file_header_length,
        stream_buffer_size,
    );

    // Communication channel with recorder's thread
    let (tx, rx): (Sender<Vec<Record>>, Receiver<Vec<Record>>) = mpsc::channel();
    let recorder_thread = result_recorder.start_recorder(rx);

    // Pull data from the parser through the iterator
    parser_iter.for_each(|(processed, records)| {
        pb.set_position(processed as u64);
        // Send records over the channel for processing on a different thread
        tx.send(records).expect("recorder channel should be alive");
    });

    // Finish and remove progress bar
    pb.finish_and_clear();

    // Send empty Vec to signal that there is no more data
    tx.send(vec![]).expect("recorder channel should be alive");

    // Blocks until recorder is done
    recorder_thread
        .join()
        .map_err(|e| HprofSlurpError::StdThreadError { e })
}
