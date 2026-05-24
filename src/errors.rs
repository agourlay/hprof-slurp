use std::any::Any;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HprofSlurpError {
    #[error("input file `{name:?}` not found")]
    InputFileNotFound { name: String },
    #[error("invalid argument `top` - the value should be strictly positive")]
    InvalidTopPositiveInt,
    #[error("missing required `--inputFile <path>` for the selected mode")]
    MissingInputFile,
    #[error(
        "conflicting modes: pick exactly one of `--find-referrers`, `--paths-from-id`, or `--diff-from`/`--diff-to`"
    )]
    ConflictingModes,
    #[error("target class not found in dump: `{name}`")]
    TargetClassNotFound { name: String },
    #[error("not yet implemented: {what}")]
    NotYetImplemented { what: &'static str },
    #[error(
        "android.graphics.Bitmap class is not loaded in this dump; bitmap accounting has nothing to report. This can happen on Android dumps from screens that have not used Bitmap-backed images."
    )]
    BitmapClassNotLoaded,
    #[error("no AllocationSites records in this dump (capture with `am profile start <pid>`)")]
    NoAllocationSites,
    #[error("invalid pointer size - the value should be either `4` or `8`")]
    InvalidIdSize,
    #[error("invalid content after header")]
    InvalidHeaderSize,
    #[error("invalid Hprof file - {message:?}")]
    InvalidHprofFile { message: String },
    #[error("CLI argument error ({0})")]
    ClapError(#[from] clap::Error),
    #[error("standard I/O error ({0})")]
    StdIoError(#[from] std::io::Error),
    #[error("standard thread error ({e:?})")]
    StdThreadError { e: Box<dyn Any + Send + 'static> },
    #[error("serialization error ({0})")]
    SerdeError(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_class_not_loaded_message_is_actionable() {
        let message = HprofSlurpError::BitmapClassNotLoaded.to_string();
        assert_eq!(
            message,
            "android.graphics.Bitmap class is not loaded in this dump; bitmap accounting has nothing to report. This can happen on Android dumps from screens that have not used Bitmap-backed images."
        );
    }
}
