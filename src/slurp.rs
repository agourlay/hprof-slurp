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
use crate::result_recorder::{RenderedResult, ResultRecorder};
use crate::utils::pretty_bytes_size;

const FILE_HEADER_LENGTH: usize = 31; // the exact size of the file header (31 bytes)

pub fn slurp_file(file_path: String, top: usize, debug_mode: bool, list_strings: bool) -> Result<RenderedResult, HprofSlurpError> {
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
    let parser = HprofRecordParser::new(debug_mode, id_size);
    let parser_iter = HprofRecordParserIter::new(
        parser,
        reader,
        debug_mode,
        file_len,
        FILE_HEADER_LENGTH,
        stream_buffer_size,
    );

    // Communication channel to recorder's thread
    let (send_records, receive_records): (Sender<Vec<Record>>, Receiver<Vec<Record>>) = mpsc::channel();

    // Communication channel with from recorder's thread
    let (send_result, receive_result): (Sender<RenderedResult>, Receiver<RenderedResult>) = mpsc::channel();

    // Recorder
    let result_recorder = ResultRecorder::new(id_size, list_strings, top);
    let recorder_thread = result_recorder.start_recorder(receive_records, send_result);

    // Progress bar
    let pb = ProgressBar::new(file_len as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} (speed:{bytes_per_sec}) (eta:{eta})")
        .progress_chars("#>-"));

    // Pull data from the parser through the iterator
    parser_iter.for_each(|(processed, records)| {
        pb.set_position(processed as u64);
        // Send records over the channel for processing on a different thread
        send_records.send(records).expect("recorder channel should be alive");
    });

    // Finish and remove progress bar
    pb.finish_and_clear();

    // Send empty Vec to signal that there is no more data
    send_records.send(vec![]).expect("recorder channel should be alive");

    let rendered_result = receive_result.recv().expect("result channel should be alive");

    // Blocks until recorder is done
    recorder_thread
        .join()
        .map_err(|e| HprofSlurpError::StdThreadError { e })?;

    Ok(rendered_result)
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
    if !rest.is_empty() {
        return Err(InvalidHeaderSize);
    }
    Ok(header)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use super::*;

    const FILE_PATH_32: &str = "test-heap-dumps/hprof-32.bin";

    const FILE_PATH_64: &str = "test-heap-dumps/hprof-64.bin";
    const FILE_PATH_RESULT_64: &str = "test-heap-dumps/hprof-64-result.txt";

    fn validate_gold_rendered_result(result: RenderedResult, gold_path: &str) {
        let gold = fs::read_to_string(gold_path).expect("gold file not found!");
        let expected = format!("{}\n{}", result.summary, result.analysis);
        let mut expected_lines = expected.lines();
        for l1 in gold.lines() {
            let l2 = expected_lines.next().unwrap();
            if l1.trim_end() != l2.trim_end() {
                println!("#####");
                println!("{}", l1.trim_end());
                println!("#####");
                println!("{}", l2.trim_end());
                println!("#####");
                assert_eq!(l1, l2)
            }
        }
    }

    #[test]
    fn supported_32_bits() {
        let file_path = FILE_PATH_32.to_string();
        let result = slurp_file(file_path, 20, false, false);
        assert_eq!(result.is_err(), true);
    }

    #[test]
    fn supported_64_bits() {
        let file_path = FILE_PATH_64.to_string();
        let result = slurp_file(file_path, 20, false, false);
        assert_eq!(result.is_ok(), true);
        validate_gold_rendered_result(result.unwrap(), FILE_PATH_RESULT_64);
    }

    #[test]
    fn file_header_32_bits() {
        let file_path = FILE_PATH_32.to_string();
        let file = File::open(file_path).unwrap();
        let mut reader = BufReader::new(file);
        let file_header = slurp_header(&mut reader).unwrap();
        assert_eq!(file_header.size_pointers, 4);
        assert_eq!(file_header.format, "JAVA PROFILE 1.0.1".to_string());
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