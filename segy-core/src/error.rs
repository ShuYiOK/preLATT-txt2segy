use thiserror::Error;

#[derive(Error, Debug)]
pub enum SegyError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Invalid file format: {0}")]
    InvalidFormat(String),

    #[error("Data mismatch: {0}")]
    DataMismatch(String),

    #[error("Operation cancelled")]
    Cancelled,
}
