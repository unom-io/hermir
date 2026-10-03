//! Progress is a callback, not a channel or a future: a CLI prints it, a host forwards it.
/// One step of an install, as it happens.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Event {
    Resolved {
        release: String,
        file_name: Option<String>,
        size: Option<u64>,
    },
    /// Bytes on disk so far; `total` when the server said.
    Download {
        done: u64,
        total: Option<u64>,
    },
    /// Checking the download against a published or pinned sha256.
    Verifying,
    /// Nothing to check the download against; `why` says so.
    NotVerified {
        why: String,
    },
    Extracting,
    Placed,
}

pub trait Progress {
    fn on(&self, event: Event);
}

/// Reports nothing.
pub struct Quiet;

impl Progress for Quiet {
    fn on(&self, _: Event) {}
}

impl<F: Fn(Event)> Progress for F {
    fn on(&self, event: Event) {
        self(event)
    }
}
