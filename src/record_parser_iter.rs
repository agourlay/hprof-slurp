use crate::record::Record;
use crate::record_parser::HprofRecordParser;
use crate::utils::pretty_bytes_size;

use nom::Err;
use nom::Needed::Size;
use nom::Needed::Unknown;

use std::fs::File;
use std::io::{BufReader, Read};

pub struct HprofRecordParserIter {
    parser: HprofRecordParser,
    debug_mode: bool,
    file_len: usize,
    processed: usize,
    loop_buffer: Vec<u8>,
    reader: BufReader<File>,
    optimistic_buffer_size: usize,
}

impl HprofRecordParserIter {
    pub fn new(
        parser: HprofRecordParser,
        debug_mode: bool,
        file_len: usize,
        processed: usize,
        reader: BufReader<File>,
        optimistic_buffer_size: usize,
    ) -> Self {
        HprofRecordParserIter {
            parser,
            debug_mode,
            file_len,
            processed,
            loop_buffer: Vec::new(), // will be init properly during the first iteration
            reader,
            optimistic_buffer_size,
        }
    }

    // pull next batch of records recursively until a result set fits in the buffer
    fn pull_next(&mut self) -> Option<(usize, Vec<Record>)> {
        if self.processed != self.file_len {
            let iteration_res = self.parser.parse_streaming(&self.loop_buffer);
            match iteration_res {
                Ok((rest, records)) => {
                    self.processed += self.loop_buffer.len() - rest.len();
                    self.loop_buffer = rest.to_vec(); // TODO remove rest.to_vec() allocations
                    assert!(
                        self.processed <= self.file_len,
                        "Can't process more than the file length"
                    );
                    Some((self.processed, records))
                }
                Err(Err::Incomplete(Size(nzu))) => {
                    let needed = nzu.get();
                    // Preload bigger buffer if possible to avoid parsing failure overhead
                    let next_size = if needed > self.optimistic_buffer_size {
                        needed
                    } else {
                        // need to account for in-flight data in the loop_buffer
                        let remaining = self.file_len - self.processed - self.loop_buffer.len();
                        if (remaining) > self.optimistic_buffer_size {
                            self.optimistic_buffer_size
                        } else {
                            remaining
                        }
                    };
                    if self.debug_mode {
                        // might not be visible if the progress bar overwrite it
                        println!(
                            "{}",
                            format!(
                                "Need more data {:?}, pull {}, remaining {}, buffer {}",
                                needed,
                                pretty_bytes_size(next_size as u64),
                                self.file_len - self.processed,
                                self.loop_buffer.len()
                            )
                        );
                    }
                    let mut extra_buffer = vec![0; next_size];
                    self.reader
                        .read_exact(&mut extra_buffer)
                        .unwrap_or_else(|e| {
                            panic!(
                                "Fail to read buffer for incomplete input:\n
                                error->{}\n
                                needed->{}\n
                                next->{}\n
                                processed->{}\n
                                file_len->{}\n
                                remaining->{}\n
                                buffer_len->{}",
                                e,
                                needed,
                                next_size,
                                self.processed,
                                self.file_len,
                                self.file_len - self.processed,
                                self.loop_buffer.len()
                            )
                        });
                    self.loop_buffer.extend_from_slice(&extra_buffer);
                    // recurse with extended buffer
                    self.pull_next()
                }
                Err(Err::Incomplete(Unknown)) => {
                    panic!("Unexpected Incomplete with unknown size")
                }
                Err(Err::Failure(e)) => {
                    panic!("parsing failed with {:?}", e)
                }
                Err(Err::Error(e)) => {
                    panic!("parsing failed with {:?}", e)
                }
            }
        } else {
            // nothing more to pull
            None
        }
    }
}

impl Iterator for HprofRecordParserIter {
    type Item = (usize, Vec<Record>);
    fn next(&mut self) -> Option<Self::Item> {
        self.pull_next()
    }
}
