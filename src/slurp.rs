use std::fs::File;
use std::io::{BufReader, ErrorKind, Read};

use indicatif::{ProgressBar, ProgressStyle};

use crossbeam_channel::{Receiver, Sender};

use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::{
    InvalidHeaderSize, InvalidHprofFile, InvalidIdSize, StdThreadError,
};
use crate::parser::file_header_parser::{FileHeader, parse_file_header};
use crate::parser::record::Record;
use crate::parser::record_stream_parser::HprofRecordStreamParser;
use crate::prefetch_reader::PrefetchReader;
use crate::rendered_result::RenderedResult;
use crate::result_recorder::ResultRecorder;
use crate::utils::pretty_bytes_size;

// the exact size of the file header (31 bytes)
const FILE_HEADER_LENGTH: usize = 31;

// 64 MB buffer performs nicely (higher is faster but increases the memory consumption)
pub const READ_BUFFER_SIZE: usize = 64 * 1024 * 1024;

pub fn slurp_file(
    file_path: &str,
    debug_mode: bool,
    list_strings: bool,
) -> Result<(FileHeader, RenderedResult), HprofSlurpError> {
    let file = File::open(file_path)?;
    let file_len = file.metadata()?.len() as usize;
    let mut reader = BufReader::new(file);

    // Parse file header
    let header = slurp_header(&mut reader)?;
    let id_size = header.size_pointers;
    // Diagnostics go to stderr so that a redirected stdout holds the report
    // and nothing else.
    eprintln!(
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
        id_size,
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
    let result_recorder = ResultRecorder::new(id_size, list_strings, header.timestamp);
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

    Ok((header, rendered_result))
}

// Every known flavour (`JAVA PROFILE 1.0.1`, `1.0.2`, `1.0.3`) shares this
// prefix; only the prefix is enforced so that a future minor version is not
// rejected up front.
const HPROF_FORMAT_PREFIX: &str = "JAVA PROFILE ";

pub fn slurp_header(reader: &mut impl Read) -> Result<FileHeader, HprofSlurpError> {
    let mut header_buffer = vec![0; FILE_HEADER_LENGTH];
    reader
        .read_exact(&mut header_buffer)
        .map_err(|e| match e.kind() {
            ErrorKind::UnexpectedEof => InvalidHprofFile {
                message: format!(
                    "the file is shorter than the {FILE_HEADER_LENGTH} bytes hprof header"
                ),
            },
            _ => e.into(),
        })?;
    let (rest, header) = parse_file_header(&header_buffer).map_err(|_| InvalidHprofFile {
        message: format!("the file does not start with a `{HPROF_FORMAT_PREFIX}...` header"),
    })?;
    // Invariants, the format first so that an arbitrary file is not reported
    // as having a bad pointer size
    if !header.format.starts_with(HPROF_FORMAT_PREFIX) {
        return Err(InvalidHprofFile {
            message: format!("unsupported format {:?}", header.format),
        });
    }
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
    use super::*;
    use std::fs;

    const FILE_PATH_32: &str = "test-heap-dumps/hprof-32.bin";
    const FILE_PATH_RESULT_32: &str = "test-heap-dumps/hprof-32-result.txt";

    const FILE_PATH_64: &str = "test-heap-dumps/hprof-64.bin";
    const FILE_PATH_RESULT_64: &str = "test-heap-dumps/hprof-64-result.txt";

    // Android `JAVA PROFILE 1.0.3` dump (32-bit ids) carrying ART/Dalvik
    // extension records (e.g. ROOT_FINALIZING 0x8a). See test-heap-dumps/README.md.
    const FILE_PATH_ANDROID: &str = "test-heap-dumps/hprof-android.bin";
    const FILE_PATH_RESULT_ANDROID: &str = "test-heap-dumps/hprof-android-result.txt";

    fn validate_gold_rendered_result(render_result: RenderedResult, gold_path: &str) {
        let gold = fs::read_to_string(gold_path).expect("gold file not found!");
        // top 20 hardcoded
        let expected = render_result.serialize(20, None);
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
        assert!(
            expected_lines.next().is_none(),
            "actual output has more lines than gold file"
        );
    }

    #[test]
    fn supported_32_bits() {
        let result = slurp_file(FILE_PATH_32, false, false);
        assert!(result.is_ok());
        validate_gold_rendered_result(result.unwrap().1, FILE_PATH_RESULT_32);
    }

    #[test]
    fn supported_64_bits() {
        let result = slurp_file(FILE_PATH_64, false, false);
        assert!(result.is_ok());
        validate_gold_rendered_result(result.unwrap().1, FILE_PATH_RESULT_64);
    }

    #[test]
    fn supported_android_1_0_3_extension_records() {
        // Regression: this dump emits Android extension GC records that older
        // versions panicked on ("unhandled gc record tag 138" = ROOT_FINALIZING).
        // It must now parse cleanly end-to-end and match the gold output.
        let result = slurp_file(FILE_PATH_ANDROID, false, false);
        assert!(result.is_ok());
        validate_gold_rendered_result(result.unwrap().1, FILE_PATH_RESULT_ANDROID);
    }

    #[test]
    fn file_header_android_1_0_3() {
        let file = File::open(FILE_PATH_ANDROID).unwrap();
        let mut reader = BufReader::new(file);
        let file_header = slurp_header(&mut reader).unwrap();
        assert_eq!(file_header.size_pointers, 4);
        assert_eq!(file_header.format, "JAVA PROFILE 1.0.3".to_string());
    }

    fn header_error(bytes: &[u8]) -> String {
        let mut reader = bytes;
        slurp_header(&mut reader)
            .expect_err("header should be rejected")
            .to_string()
    }

    #[test]
    fn file_header_rejects_an_empty_file() {
        assert_eq!(
            header_error(&[]),
            "invalid Hprof file - the file is shorter than the 31 bytes hprof header"
        );
    }

    #[test]
    fn file_header_rejects_a_file_shorter_than_the_header() {
        assert!(header_error(b"JAVA PROFILE 1.0.2\0").contains("shorter than the 31 bytes"));
    }

    #[test]
    fn file_header_rejects_a_missing_format_string() {
        // no NUL terminator within the header bytes
        assert_eq!(
            header_error(&[b'x'; 31]),
            "invalid Hprof file - the file does not start with a `JAVA PROFILE ...` header"
        );
    }

    #[test]
    fn file_header_rejects_an_unknown_format() {
        let mut bytes = b"NOT A PROFILE 1.0\0".to_vec();
        bytes.extend_from_slice(&8u32.to_be_bytes());
        bytes.extend_from_slice(&0u64.to_be_bytes());
        bytes.push(0); // pad to 31 bytes
        assert_eq!(bytes.len(), 31);
        assert_eq!(
            header_error(&bytes),
            "invalid Hprof file - unsupported format \"NOT A PROFILE 1.0\""
        );
    }

    #[test]
    fn file_header_32_bits() {
        let file_path = FILE_PATH_32.to_string();
        let file = File::open(file_path).unwrap();
        let mut reader = BufReader::new(file);
        let file_header = slurp_header(&mut reader).unwrap();
        assert_eq!(file_header.size_pointers, 4);
        assert!(matches!(
            file_header.format.as_str(),
            "JAVA PROFILE 1.0.1" | "JAVA PROFILE 1.0.2"
        ));
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
