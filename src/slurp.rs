use std::fs::File;
use std::io::{BufReader, Read};

use indicatif::{ProgressBar, ProgressStyle};

use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};

use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::*;
use crate::file_header_parser::{FileHeader, parse_file_header};
use crate::record::Record;
use crate::record_parser::HprofRecordParser;
use crate::record_parser_iter::HprofRecordParserIter;
use crate::result_recorder::ResultRecorder;
use crate::utils::pretty_bytes_size;

const FILE_HEADER_LENGTH: usize = 31; // the exact size of the file header (31 bytes)

pub fn slurp_file(file_path: String, top: usize, debug_mode: bool, list_strings: bool) -> Result<(), HprofSlurpError> {
    let file = File::open(file_path)?;
    let file_len = file.metadata()?.len() as usize;
    let mut reader = BufReader::new(file);

    // Parse file header
    let header = slurp_header(&mut reader)?;
    let id_size = header.size_pointers;
    println!(
        "Processing {} binary hprof file in '{}' format.",
        pretty_bytes_size(file_len as u64),
        header.format
    );

    // 64 MB buffer performs nicely (higher is faster but increases the memory consumption)
    let stream_buffer_size = 64 * 1024 * 1024;

    // Init parser state
    let parser = HprofRecordParser::new(debug_mode);
    let parser_iter = HprofRecordParserIter::new(
        parser,
        reader,
        debug_mode,
        file_len,
        FILE_HEADER_LENGTH,
        stream_buffer_size,
    );

    // Communication channel with recorder's thread
    let (tx, rx): (Sender<Vec<Record>>, Receiver<Vec<Record>>) = mpsc::channel();

    // Recorder
    let result_recorder = ResultRecorder::new(id_size, list_strings, top);
    let recorder_thread = result_recorder.start_recorder(rx);

    // Progress bar
    let pb = ProgressBar::new(file_len as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} (speed:{bytes_per_sec}) (eta:{eta})")
        .progress_chars("#>-"));

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

pub fn slurp_header(reader: &mut BufReader<File>) -> Result<FileHeader, HprofSlurpError> {
    let mut header_buffer = vec![0; FILE_HEADER_LENGTH];
    reader.read_exact(&mut header_buffer)?;
    let (rest, header) = parse_file_header(&header_buffer).unwrap();
    // Invariants
    let id_size = header.size_pointers;
    if id_size != 4 && id_size != 8 {
        return Err(InvalidIdSize);
    }
    if id_size == 4 {
        return Err(UnsupportedHeaderSize { message: "32 bits heap dumps are not supported yet".to_string() });
    }
    if !rest.is_empty() {
        return Err(InvalidHeaderSize);
    }
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE_PATH_32: &str = "test-heap-dumps/hprof-32.bin";
    const FILE_PATH_64: &str = "test-heap-dumps/hprof-64.bin";

    #[test]
    fn unsupported_32_bits() {
        let file_path = FILE_PATH_32.to_string();
        let result = slurp_file(file_path, 10, false, false);
        assert_eq!(result.is_err(), true);
    }

    #[test]
    fn supported_64_bits() {
        let file_path = FILE_PATH_64.to_string();
        let result = slurp_file(file_path, 10, false, false);
        assert_eq!(result.is_ok(), true);
    }

    #[test]
    fn file_header_32_bits() {
        let file_path = FILE_PATH_32.to_string();
        let file = File::open(file_path).unwrap();
        let mut reader = BufReader::new(file);
        let result = slurp_header(&mut reader);
        assert_eq!(result.is_err(), true);
    }

    #[test]
    fn file_header_64_bits() {
        let file_path = FILE_PATH_64.to_string();
        let file = File::open(file_path).unwrap();
        let mut reader = BufReader::new(file);
        let file_header = slurp_header(&mut reader).unwrap();
        assert_eq!(file_header.size_pointers, 8);
        assert_eq!(file_header.format, "JAVA PROFILE 1.0.1".to_string());
    }

}