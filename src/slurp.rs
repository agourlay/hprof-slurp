use std::fs::File;
use std::io::{BufReader, Read};

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

/// Synchronous record-by-record parsing. Parses the hprof header, then
/// streams records through the in-process `HprofRecordParser`, calling
/// `handler` for every record produced. Used by the multi-pass referrer /
/// paths drivers, which benefit more from request flexibility than from
/// the threaded prefetcher pipeline `slurp_file` uses.
///
/// Returns the dump's `id_size` (4 or 8) so callers can use it for field
/// decoding without re-parsing the header themselves.
pub fn parse_records<F>(
    file_path: &str,
    debug: bool,
    retain_bodies: bool,
    handler: F,
) -> Result<u32, HprofSlurpError>
where
    F: FnMut(crate::parser::record::Record),
{
    parse_records_with_modes(file_path, debug, retain_bodies, false, 0, handler)
}

/// Like `parse_records`, but with explicit control over both
/// `retain_bodies` (for instance fields + object-array elements) and
/// `retain_primitive_bodies` (for primitive-array bodies). The latter
/// is gated on `preview_bytes_limit` to bound memory usage. v0.9.0
/// feature B uses this to collect primitive previews on demand.
pub fn parse_records_with_modes<F>(
    file_path: &str,
    debug: bool,
    retain_bodies: bool,
    retain_primitive_bodies: bool,
    preview_bytes_limit: u32,
    mut handler: F,
) -> Result<u32, HprofSlurpError>
where
    F: FnMut(crate::parser::record::Record),
{
    use crate::parser::record_parser::HprofRecordParser;
    use std::io::Read;
    let file = File::open(file_path)?;
    let mut reader = BufReader::new(file);
    let header = slurp_header(&mut reader)?;
    let id_size = header.size_pointers;

    let mut parser = HprofRecordParser::with_modes(
        debug,
        id_size,
        retain_bodies,
        retain_primitive_bodies,
        preview_bytes_limit,
    );
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 20); // 1 MiB working buffer
    let mut pooled: Vec<Record> = Vec::with_capacity(1024);
    let mut chunk = vec![0u8; 1 << 20];

    loop {
        let n = reader.read(&mut chunk)?;
        if n > 0 {
            buf.extend_from_slice(&chunk[..n]);
        }
        if buf.is_empty() {
            break;
        }
        match parser.parse_streaming(&buf, &mut pooled) {
            Ok((rest, ())) => {
                let consumed = buf.len() - rest.len();
                buf.drain(0..consumed);
                for rec in pooled.drain(..) {
                    handler(rec);
                }
                if n == 0 && buf.is_empty() {
                    break;
                }
                if n == 0 && consumed == 0 {
                    // EOF and parser made no progress — leftover trailing bytes;
                    // surface as a parse error rather than infinite-loop.
                    return Err(InvalidHprofFile {
                        message: format!("trailing bytes at EOF: {} unparsed bytes", buf.len()),
                    });
                }
            }
            Err(nom::Err::Incomplete(_)) => {
                if n == 0 {
                    return Err(InvalidHprofFile {
                        message: format!("unexpected EOF mid-record: {} unparsed bytes", buf.len()),
                    });
                }
                // need more data; loop and read more
                continue;
            }
            Err(nom::Err::Error(e)) | Err(nom::Err::Failure(e)) => {
                return Err(InvalidHprofFile {
                    message: format!("{e:?}"),
                });
            }
        }
    }
    Ok(id_size)
}

/// Existing entry point — runs the streaming pipeline with no preview
/// retention. Equivalent to `slurp_file_with_preview(.., 0, 1024)`.
pub fn slurp_file(
    file_path: &str,
    debug_mode: bool,
    list_strings: bool,
) -> Result<RenderedResult, HprofSlurpError> {
    slurp_file_with_preview(file_path, debug_mode, list_strings, 0, 1024)
}

/// Like `slurp_file`, with explicit control over primitive-array
/// preview capture (v0.9.0 feature B). When `preview_bytes > 0`:
///
/// * the parser is constructed with `retain_primitive_bodies=true`
///   and `preview_bytes_limit=preview_bytes`
/// * the recorder retains the truncated body of the largest array
///   of each element type for surfacing under "Largest array
///   instances" entries
///
/// `list_arrays_min_bytes` is the threshold for the `-l` extension
/// covered by PR 6.
pub fn slurp_file_with_preview(
    file_path: &str,
    debug_mode: bool,
    list_strings: bool,
    preview_bytes: u32,
    list_arrays_min_bytes: u32,
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
    let stream_parser = HprofRecordStreamParser::with_modes(
        debug_mode,
        id_size,
        file_len,
        FILE_HEADER_LENGTH,
        initial_loop_buffer,
        false, // retain_bodies
        preview_bytes > 0,
        preview_bytes,
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
    let result_recorder =
        ResultRecorder::with_preview(id_size, list_strings, preview_bytes, list_arrays_min_bytes);
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

    fn validate_gold_rendered_result(render_result: RenderedResult, gold_path: &str) {
        let gold = fs::read_to_string(gold_path).expect("gold file not found!");
        // top 20 hardcoded
        let expected = render_result.serialize(20);
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

        let rendered_result = result.unwrap();
        assert!(rendered_result.summary.contains("UTF-8 Strings:"));
        assert!(!rendered_result.memory_usage.is_empty());
    }

    #[test]
    fn supported_64_bits() {
        let result = slurp_file(FILE_PATH_64, false, false);
        assert!(result.is_ok());
        validate_gold_rendered_result(result.unwrap(), FILE_PATH_RESULT_64);
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
