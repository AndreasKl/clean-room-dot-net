use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The input is not a well-formed PE/CLI image.
    BadImage(String),
    /// The input is well-formed but uses a feature not supported yet.
    Unsupported(String),
    /// A logical model handed to the writer is inconsistent.
    Malformed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadImage(m) => write!(f, "bad image: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Malformed(m) => write!(f, "malformed model: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
