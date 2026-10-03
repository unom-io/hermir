//! One error type for the whole library. Every variant maps to a CLI exit code, so a script
//! can branch on the number and a human can read the message.
use std::path::PathBuf;

/// Everything that can go wrong, by kind: each kind is one CLI exit code
/// ([`Error::exit_code`]).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No catalog entry has this id.
    #[error("{0} is not in the catalog")]
    NotInCatalog(String),
    /// The catalog refuses on purpose: a Switch emulator is never installed.
    #[error("{emulator}: {why}")]
    Policy {
        /// The entry's id.
        emulator: String,
        /// The catalog's reason.
        why: String,
    },
    /// The entry has no channel for this OS.
    #[error("{emulator} is not offered on {os}")]
    UnsupportedOs {
        /// The entry's id.
        emulator: String,
        /// The OS asked for.
        os: String,
    },
    /// A request failed, timed out, or answered something unusable.
    #[error("{op}: {why}")]
    Network {
        /// What was being fetched: a URL.
        op: String,
        /// Why it failed.
        why: String,
    },
    /// A download does not match the sha256 published or pinned for it.
    #[error("{what}: expected sha256 {expected}, got {actual}")]
    Verify {
        /// The file.
        what: String,
        /// The digest it should have.
        expected: String,
        /// The digest it has.
        actual: String,
    },
    /// Verification was required, and there is nothing to check this download against.
    #[error("{what} is not verified: {why}")]
    Unverified {
        /// The file or emulator.
        what: String,
        /// What is missing.
        why: String,
    },
    /// An archive could not be unpacked, or its files not put in place.
    #[error("{what}: {why}")]
    Place {
        /// The archive, file or emulator.
        what: String,
        /// Why not.
        why: String,
    },
    /// A catalog entry is malformed, or breaks a rule the schema cannot express.
    #[error("catalog entry {entry}: {why}")]
    Catalog {
        /// The entry's id, or the file.
        entry: String,
        /// What is wrong with it.
        why: String,
    },
    /// Another hermir holds the prefix lock; the path is the prefix.
    #[error("{0} is busy: another hermir holds the prefix lock")]
    Locked(PathBuf),
    /// What was asked cannot be done as asked: a seat given twice, a newline in a value.
    #[error("{0}")]
    Invalid(String),
    /// There is no copy of the emulator on this machine to act on.
    #[error("{0} is not on this machine")]
    NotInstalled(String),
    /// SDL could not list the pads (the `enumerate` feature), in its words.
    #[error("listing the pads: {0}")]
    Pads(String),
    /// A file or folder could not be read or written.
    #[error("{op} {path}: {source}")]
    Io {
        /// What was being done: `read`, `write`, `rename`…
        op: &'static str,
        /// To what.
        path: PathBuf,
        /// What the OS said.
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    /// The CLI exit code: 0 ok · 1 I/O, or SDL could not list the pads · 2 catalog, policy or an impossible request · 3 network ·
    /// 4 verification · 5 extract or place · 6 unsupported on this OS · 7 a config file could
    /// not be written (a failed step of `apply`, `revert` or `prepare`) · 8 locked · 9 not on
    /// this machine.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Io { .. } | Error::Pads(_) => 1,
            Error::NotInCatalog(_)
            | Error::Policy { .. }
            | Error::Catalog { .. }
            | Error::Invalid(_) => 2,
            Error::Network { .. } => 3,
            Error::Verify { .. } | Error::Unverified { .. } => 4,
            Error::Place { .. } => 5,
            Error::UnsupportedOs { .. } => 6,
            Error::Locked(_) => 8,
            Error::NotInstalled(_) => 9,
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

/// The library's result: every error is an [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
