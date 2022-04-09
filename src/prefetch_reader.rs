use std::fs::File;
use std::io::{BufReader, Read};
use std::sync::mpsc::SyncSender;
use std::thread;
use std::thread::JoinHandle;

pub struct PrefetchReader {
    reader: BufReader<File>,
    file_len: usize,
    processed_len: usize,
    read_size: usize,
}

impl PrefetchReader {
    pub fn new(
        reader: BufReader<File>,
        file_len: usize,
        processed_len: usize,
        read_size: usize,
    ) -> Self {
        PrefetchReader {
            reader,
            file_len,
            processed_len,
            read_size,
        }
    }

    pub fn start(mut self, send_data: SyncSender<Vec<u8>>) -> std::io::Result<JoinHandle<()>> {
        thread::Builder::new()
            .name("hprof-prefetch".to_string())
            .spawn(move || {
                while self.processed_len != self.file_len {
                    let remaining = self.file_len - self.processed_len;
                    let next_size = if remaining > self.read_size {
                        self.read_size
                    } else {
                        remaining
                    };
                    let mut extra_buffer = vec![0; next_size];
                    self.reader
                        .read_exact(&mut extra_buffer)
                        .unwrap_or_else(|e| {
                            panic!(
                                "Fail to read buffer for incomplete input:\n
                                error->{}\n
                                next->{}\n
                                processed->{}\n
                                file_len->{}\n
                                remaining->{}",
                                e,
                                next_size,
                                self.processed_len,
                                self.file_len,
                                self.file_len - self.processed_len
                            )
                        });
                    send_data
                        .send(extra_buffer)
                        .expect("Channel should not be closed");
                    self.processed_len += next_size
                }
            })
    }
}
