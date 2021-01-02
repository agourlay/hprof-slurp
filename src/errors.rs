#[derive(Debug)]
pub enum HprofSlurpError {
    InputFileNotFound,
    InvalidTopPositiveInt,
    InvalidIdSize,
    InvalidHeaderSize,
    ClapError { e: clap::Error },
    StdIoError { e: std::io::Error },
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
