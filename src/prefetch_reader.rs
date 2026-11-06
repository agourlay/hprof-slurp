use crossbeam_channel::{Receiver, Sender};
use std::fs::File;
use std::io::{BufReader, Read};
use std::thread;
use std::thread::JoinHandle;

pub struct PrefetchReader {
    reader: BufReader<File>,
    file_len: usize,
    processed_len: usize,
    read_size: usize,
}

impl PrefetchReader {
    pub const fn new(
        reader: BufReader<File>,
        file_len: usize,
        processed_len: usize,
        read_size: usize,
    ) -> Self {
        Self {
            reader,
            file_len,
            processed_len,
            read_size,
        }
    }

    pub fn start(
        mut self,
        send_data: Sender<Vec<u8>>,
        receive_pooled_data: Receiver<Vec<u8>>,
    ) -> std::io::Result<JoinHandle<std::io::Result<()>>> {
        thread::Builder::new()
            .name("hprof-prefetch".to_string())
            .spawn(move || {
                while self.processed_len != self.file_len {
                    let remaining = self.file_len - self.processed_len;
                    let next_size = remaining.min(self.read_size);
                    // A closed channel means the parser is gone: stop quietly,
                    // its own failure is the one reported.
                    let Ok(mut pooled_buffer) = receive_pooled_data.recv() else {
                        return Ok(());
                    };
                    pooled_buffer.resize(next_size, 0);
                    self.reader.read_exact(&mut pooled_buffer).map_err(|e| {
                        std::io::Error::new(
                            e.kind(),
                            format!(
                                "failed to read {next_size} bytes at offset {} of {} bytes: {e}",
                                self.processed_len, self.file_len
                            ),
                        )
                    })?;
                    if send_data.send(pooled_buffer).is_err() {
                        return Ok(());
                    }
                    self.processed_len += next_size;
                }
                Ok(())
            })
    }
}
