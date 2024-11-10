use std::any::Any;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HprofSlurpError {
    #[error("input file `{name:?}` not found")]
    InputFileNotFound { name: String },
    #[error("invalid argument `top` - the value should be strictly positive")]
    InvalidTopPositiveInt,
    #[error("invalid pointer size - the value should be either `4` or `8`")]
    InvalidIdSize,
    #[error("invalid content after header")]
    InvalidHeaderSize,
    #[error("invalid Hprof file - {message:?}")]
    InvalidHprofFile { message: String },
    #[error("unsupported pointer size - {message:?}")]
    UnsupportedIdSize { message: String },
    #[error("CLI argument error ({e})")]
    ClapError { e: clap::Error },
    #[error("standard I/O error ({e})")]
    StdIoError { e: std::io::Error },
    #[error("standard thread error ({e:?})")]
    StdThreadError { e: Box<dyn Any + Send + 'static> },
}

impl From<std::io::Error> for HprofSlurpError {
    fn from(e: std::io::Error) -> Self {
        Self::StdIoError { e }
    }
}

impl From<clap::Error> for HprofSlurpError {
    fn from(e: clap::Error) -> Self {
        Self::ClapError { e }
    }
}
