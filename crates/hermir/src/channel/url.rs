//! A fixed URL with a version pinned in the catalog: projects without GitHub releases.
use crate::model::Resolved;

pub fn resolve(emulator: &str, url: &str, version: &str, sha256: Option<&str>) -> Resolved {
    Resolved {
        emulator: emulator.into(),
        channel: "url".into(),
        url: Some(url.into()),
        file_name: url.rsplit('/').next().map(str::to_string),
        size: None,
        version: Some(version.into()),
        release: version.into(),
        digest: sha256.map(str::to_string),
    }
}
