//! One error type for the whole library. Every variant maps to a CLI exit code, so a script
//! can branch on the number and a human can read the message.
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0} is not in the catalog")]
    NotInCatalog(String),
    #[error("{emulator}: {why}")]
    Policy { emulator: String, why: String },
    #[error("{emulator} is not offered on {os}")]
    UnsupportedOs { emulator: String, os: String },
    #[error("{op}: {why}")]
    Network { op: String, why: String },
    #[error("{what}: expected sha256 {expected}, got {actual}")]
    Verify {
        what: String,
        expected: String,
        actual: String,
    },
    #[error("{what}: {why}")]
    Place { what: String, why: String },
    #[error("catalog entry {entry}: {why}")]
    Catalog { entry: String, why: String },
    #[error("{0} is busy: another hermir holds the prefix lock")]
    Locked(PathBuf),
    #[error("{op} {path}: {source}")]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    /// The CLI exit code: 0 ok · 1 I/O · 2 catalog or policy · 3 network · 4 verification ·
    /// 5 extract or place · 6 unsupported on this OS · 8 locked.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Io { .. } => 1,
            Error::NotInCatalog(_) | Error::Policy { .. } | Error::Catalog { .. } => 2,
            Error::Network { .. } => 3,
            Error::Verify { .. } => 4,
            Error::Place { .. } => 5,
            Error::UnsupportedOs { .. } => 6,
            Error::Locked(_) => 8,
        }
    }

    pub(crate) fn io(op: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            op,
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
