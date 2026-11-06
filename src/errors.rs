use thiserror::Error;

#[derive(Error, Debug)]
pub enum HprofSlurpError {
    #[error("input file `{name}` not found")]
    InputFileNotFound { name: String },
    #[error("invalid pointer size - the value should be either `4` or `8`")]
    InvalidIdSize,
    #[error("invalid content after header")]
    InvalidHeaderSize,
    #[error("invalid Hprof file - {message}")]
    InvalidHprofFile { message: String },
    #[error("CLI argument error ({0})")]
    ClapError(#[from] clap::Error),
    #[error("standard I/O error ({0})")]
    StdIoError(#[from] std::io::Error),
    #[error("the `{thread}` thread failed: {message}")]
    StdThreadError { thread: String, message: String },
    #[error("serialization error ({0})")]
    SerdeError(#[from] serde_json::Error),
}
