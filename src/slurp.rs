use std::fs::File;
use std::io::{BufReader, Read};

use indicatif::{ProgressBar, ProgressStyle};

use crossbeam_channel::{Receiver, Sender};

use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::{
    InvalidHeaderSize, InvalidHprofFile, InvalidIdSize, StdThreadError, UnsupportedIdSize,
};
use crate::parser::file_header_parser::{parse_file_header, FileHeader};
use crate::parser::record::Record;
use crate::parser::record_stream_parser::HprofRecordStreamParser;
use crate::prefetch_reader::PrefetchReader;
use crate::result_recorder::{RenderedResult, ResultRecorder};
use crate::utils::pretty_bytes_size;

// the exact size of the file header (31 bytes)
const FILE_HEADER_LENGTH: usize = 31;

// 64 MB buffer performs nicely (higher is faster but increases the memory consumption)
pub const READ_BUFFER_SIZE: usize = 64 * 1024 * 1024;

pub fn slurp_file(
    file_path: String,
    top: usize,
    debug_mode: bool,
    list_strings: bool,
) -> Result<RenderedResult, HprofSlurpError> {
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

    // Communication channel from pre-fetcher to parser
    let (send_data, receive_data): (Sender<Vec<u8>>, Receiver<Vec<u8>>) =
        crossbeam_channel::unbounded();

    // Communication channel from parser to pre-fetcher (pooled input buffers)
    let (send_pooled_data, receive_pooled_data): (Sender<Vec<u8>>, Receiver<Vec<u8>>) =
        crossbeam_channel::unbounded();

    // Init pooled binary data with more than 1 element to enable the reader to make progress interdependently
    for _ in 0..2 {
        send_pooled_data
            .send(Vec::with_capacity(READ_BUFFER_SIZE))
            .expect("pre-fetcher channel should be alive");
    }

    // Communication channel from parser to recorder
    let (send_records, receive_records): (Sender<Vec<Record>>, Receiver<Vec<Record>>) =
        crossbeam_channel::unbounded();

    // Communication channel from recorder to parser (pooled record buffers)
    let (send_pooled_vec, receive_pooled_vec): (Sender<Vec<Record>>, Receiver<Vec<Record>>) =
        crossbeam_channel::unbounded();

    // Communication channel from recorder to main
    let (send_result, receive_result): (Sender<RenderedResult>, Receiver<RenderedResult>) =
        crossbeam_channel::unbounded();

    // Communication channel from parser to main
    let (send_progress, receive_progress): (Sender<usize>, Receiver<usize>) =
        crossbeam_channel::unbounded();

    // Init pre-fetcher
    let prefetcher = PrefetchReader::new(reader, file_len, FILE_HEADER_LENGTH, READ_BUFFER_SIZE);
    let prefetch_thread = prefetcher.start(send_data, receive_pooled_data)?;

    // Init pooled result vec
    send_pooled_vec
        .send(Vec::new())
        .expect("recorder channel should be alive");

    // Init stream parser
    let initial_loop_buffer = Vec::with_capacity(READ_BUFFER_SIZE); // will be added to the data pool after the first chunk
    let stream_parser = HprofRecordStreamParser::new(
        debug_mode,
        file_len,
        FILE_HEADER_LENGTH,
        initial_loop_buffer,
    );

    // Start stream parser
    let parser_thread = stream_parser.start(
        receive_data,
        send_pooled_data,
        send_progress,
        receive_pooled_vec,
        send_records,
    )?;

    // Init result recorder
    let result_recorder = ResultRecorder::new(id_size, list_strings, top);
    let recorder_thread = result_recorder.start(receive_records, send_result, send_pooled_vec)?;

    // Init progress bar
    let pb = ProgressBar::new(file_len as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} (speed:{bytes_per_sec}) (eta:{eta})")
        .expect("templating should never fail")
        .progress_chars("#>-"));

    // Feed progress bar
    while let Ok(processed) = receive_progress.recv() {
        pb.set_position(processed as u64);
    }

    // Finish and remove progress bar
    pb.finish_and_clear();

    // Wait for final result
    let rendered_result = receive_result
        .recv()
        .expect("result channel should be alive");

    // Blocks until pre-fetcher is done
    prefetch_thread.join().map_err(|e| StdThreadError { e })?;

    // Blocks until parser is done
    parser_thread.join().map_err(|e| StdThreadError { e })?;

    // Blocks until recorder is done
    recorder_thread.join().map_err(|e| StdThreadError { e })?;

    Ok(rendered_result)
}

pub fn slurp_header(reader: &mut BufReader<File>) -> Result<FileHeader, HprofSlurpError> {
    let mut header_buffer = vec![0; FILE_HEADER_LENGTH];
    reader.read_exact(&mut header_buffer)?;
    let (rest, header) = parse_file_header(&header_buffer).map_err(|e| InvalidHprofFile {
        message: format!("{e:?}"),
    })?;
    // Invariants
    let id_size = header.size_pointers;
    if id_size != 4 && id_size != 8 {
        return Err(InvalidIdSize);
    }
    if id_size == 4 {
        return Err(UnsupportedIdSize {
            message: "32 bits heap dumps are not supported yet".to_string(),
        });
    }
    if !rest.is_empty() {
        return Err(InvalidHeaderSize);
    }
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const FILE_PATH_32: &str = "test-heap-dumps/hprof-32.bin";

    const FILE_PATH_64: &str = "test-heap-dumps/hprof-64.bin";
    const FILE_PATH_RESULT_64: &str = "test-heap-dumps/hprof-64-result.txt";

    fn validate_gold_rendered_result(result: &RenderedResult, gold_path: &str) {
        let gold = fs::read_to_string(gold_path).expect("gold file not found!");
        let expected = format!(
            "{}\n{}\n{}",
            result.summary, result.thread_info, result.memory_usage
        );
        let mut expected_lines = expected.lines();
        for (i1, l1) in gold.lines().enumerate() {
            let l2 = expected_lines.next().unwrap();
            if l1.trim_end() != l2.trim_end() {
                println!("## GOLD line {} ##", i1 + 1);
                println!("{}", l1.trim_end());
                println!("## ACTUAL ##");
                println!("{}", l2.trim_end());
                println!("#####");
                assert_eq!(l1, l2);
            }
        }
    }

    #[test]
    fn unsupported_32_bits() {
        let file_path = FILE_PATH_32.to_string();
        let result = slurp_file(file_path, 20, false, false);
        assert!(result.is_err());
    }

    #[test]
    fn supported_64_bits() {
        let file_path = FILE_PATH_64.to_string();
        let result = slurp_file(file_path, 20, false, false);
        assert!(result.is_ok());
        validate_gold_rendered_result(&result.unwrap(), FILE_PATH_RESULT_64);
    }

    #[test]
    fn file_header_32_bits() {
        let file_path = FILE_PATH_32.to_string();
        let file = File::open(file_path).unwrap();
        let mut reader = BufReader::new(file);
        let result = slurp_header(&mut reader);
        assert!(result.is_err());
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
