use std::any::Any;

#[derive(Debug)]
pub enum HprofSlurpError {
    InputFileNotFound { name: String },
    InvalidTopPositiveInt,
    InvalidIdSize,
    InvalidHeaderSize,
    UnsupportedHeaderSize { message: String },
    ClapError { e: clap::Error },
    StdIoError { e: std::io::Error },
    StdThreadError { e: Box<dyn Any + Send + 'static>},
}

impl std::convert::From<std::io::Error> for HprofSlurpError {
    fn from(e: std::io::Error) -> Self {
        HprofSlurpError::StdIoError { e }
    }
}

impl std::convert::From<clap::Error> for HprofSlurpError {
    fn from(e: clap::Error) -> Self {
        HprofSlurpError::ClapError { e }
    }
}
