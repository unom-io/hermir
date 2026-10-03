//! Progress is a callback, not a channel or a future: a CLI prints it, a host forwards it.
/// One step of an install, as it happens.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// The channel said what it would install.
    Resolved {
        /// The release id, as the channel names it.
        release: String,
        /// The file that will be downloaded, for an archive channel.
        file_name: Option<String>,
        /// Its size in bytes, when the channel says.
        size: Option<u64>,
    },
    /// Bytes on disk so far; `total` when the server said.
    Download {
        /// Bytes on disk, a resumed part's included.
        done: u64,
        /// The whole file's size, when the server said.
        total: Option<u64>,
    },
    /// Checking the download against a published or pinned sha256.
    Verifying,
    /// Nothing to check the download against; `why` says so.
    NotVerified {
        /// What there was nothing to check against, in a phrase a UI shows.
        why: String,
    },
    /// Unpacking the download.
    Extracting,
    /// The release is in place.
    Placed,
}

/// Where install events go: a CLI prints them, a host forwards them. A closure taking an
/// [`Event`] is one.
pub trait Progress {
    /// One event, as it happens.
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
