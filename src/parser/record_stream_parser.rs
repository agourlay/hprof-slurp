use crate::parser::record::Record;
use crate::parser::record_parser::HprofRecordParser;

use nom::Err;
use nom::Needed::Size;
use nom::Needed::Unknown;

use crate::slurp::READ_BUFFER_SIZE;
use crossbeam_channel::{Receiver, Sender};
use std::thread;
use std::thread::JoinHandle;

pub struct HprofRecordStreamParser {
    parser: HprofRecordParser,
    debug_mode: bool,
    file_len: usize,
    processed_len: usize,
    loop_buffer: Vec<u8>,
    pooled_vec: Vec<Record>,
    needed: usize,
}

impl HprofRecordStreamParser {
    pub fn new(
        debug_mode: bool,
        file_len: usize,
        processed_len: usize,
        initial_loop_buffer: Vec<u8>,
    ) -> Self {
        let parser = HprofRecordParser::new(debug_mode);
        HprofRecordStreamParser {
            parser,
            debug_mode,
            file_len,
            processed_len,
            loop_buffer: initial_loop_buffer,
            pooled_vec: Vec::new(),
            needed: 0,
        }
    }

    pub fn start(
        mut self,
        receive_data: Receiver<Vec<u8>>,
        send_pooled_data: Sender<Vec<u8>>,
        send_progress: Sender<usize>,
        receive_pooled_vec: Receiver<Vec<Record>>,
        send_records: Sender<Vec<Record>>,
    ) -> std::io::Result<JoinHandle<()>> {
        thread::Builder::new()
            .name("hprof-parser".to_string())
            .spawn(move || {
                loop {
                    match receive_data.recv() {
                        Err(_) => break,
                        Ok(mut pooled_buffer) => {
                            // Move input buffer into working buffer
                            self.loop_buffer.append(&mut pooled_buffer);
                            // Send back empty pooled_buffer with storage
                            send_pooled_data.send(pooled_buffer).unwrap_or_default();
                            if self.needed > self.loop_buffer.len() {
                                // need more data for the ongoing object
                                continue;
                            }
                            let iteration_res = self
                                .parser
                                .parse_streaming(&self.loop_buffer, &mut self.pooled_vec);
                            match iteration_res {
                                Ok((rest, ())) => {
                                    let rest_len = rest.len();
                                    let iteration_processed = self.loop_buffer.len() - rest_len;
                                    self.processed_len += iteration_processed;
                                    self.loop_buffer.drain(0..iteration_processed);
                                    assert!(
                                        self.processed_len <= self.file_len,
                                        "Can't process more than the file length (processed:{} vs file:{})",
                                        self.processed_len,
                                        self.file_len
                                    );
                                    send_progress
                                        .send(self.processed_len)
                                        .expect("channel should not be closed");
                                    let mut next_pooled_vec = receive_pooled_vec
                                        .recv()
                                        .expect("channel should not be closed");
                                    // next_pooled_vec contains the records result after the swap
                                    std::mem::swap(&mut next_pooled_vec, &mut self.pooled_vec);
                                    send_records
                                        .send(next_pooled_vec)
                                        .expect("channel should not be closed");
                                    if self.needed > 0 {
                                        // Multi-buffer object successfully parsed
                                        // Do not hold on too much working memory
                                        self.loop_buffer.shrink_to(READ_BUFFER_SIZE * 2);
                                        // Reset extra data needed flag
                                        self.needed = 0;
                                    }
                                }
                                Err(Err::Incomplete(Size(n))) => {
                                    if self.debug_mode {
                                        println!("Incomplete: {} bytes required to finish parsing object & current buffer len {}", n.get(), self.loop_buffer.len());
                                    }
                                    // capture needed data (missing + existing)
                                    self.needed = n.get() + self.loop_buffer.len();
                                }
                                Err(Err::Incomplete(Unknown)) => {
                                    panic!("Unexpected Incomplete with unknown size")
                                }
                                Err(Err::Failure(e)) => {
                                    panic!("parsing failed with {e:?}")
                                }
                                Err(Err::Error(e)) => {
                                    panic!("parsing failed with {e:?}")
                                }
                            }
                        }
                    }
                }
            })
    }
}
