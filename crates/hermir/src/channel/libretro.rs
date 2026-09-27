//! libretro cores from the buildbot, into a RetroArch cores directory: the same URL
//! RetroArch's own core updater fetches, so a core that "isn't installed" is one call away.
use std::path::Path;

use crate::channel::extract::extract;
use crate::channel::http::Http;
use crate::error::{Error, Result};
use crate::model::Os;
use crate::progress::{Event, Progress};

pub fn core_url(os: Os, core: &str) -> Option<String> {
    let (dir, ext) = match os {
        Os::Linux => ("linux/x86_64", "so"),
        Os::Windows => ("windows/x86_64", "dll"),
        Os::Macos => return None,
    };
    Some(format!(
        "https://buildbot.libretro.com/nightly/{dir}/latest/{core}_libretro.{ext}.zip"
    ))
}

/// Downloads and unpacks `<core>_libretro.<so|dll>` into `cores_dir`. Returns the core's path.
pub fn install_core(
    http: &dyn Http,
    os: Os,
    core: &str,
    cores_dir: &Path,
    tmp: &Path,
    progress: &dyn Progress,
) -> Result<std::path::PathBuf> {
    if !core.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(Error::Place {
            what: core.into(),
            why: "a core name is [A-Za-z0-9_]".into(),
        });
    }
    let url = core_url(os, core).ok_or_else(|| Error::UnsupportedOs {
        emulator: format!("core {core}"),
        os: os.to_string(),
    })?;
    std::fs::create_dir_all(tmp).map_err(|e| Error::io("create", tmp, e))?;
    let part = tmp.join(format!("{core}.zip.part"));
    let _ = std::fs::remove_file(&part);
    progress.on(Event::Resolved {
        release: "nightly/latest".into(),
        file_name: Some(format!("{core}_libretro.zip")),
        size: None,
    });
    http.download(&url, &part, progress)?;
    let zip = tmp.join(format!("{core}.zip"));
    std::fs::rename(&part, &zip).map_err(|e| Error::io("rename", &zip, e))?;
    progress.on(Event::Extracting);
    let out = tmp.join(format!("{core}.extract"));
    let _ = std::fs::remove_dir_all(&out);
    extract(&zip, &out)?;
    let _ = std::fs::remove_file(&zip);
    std::fs::create_dir_all(cores_dir).map_err(|e| Error::io("create", cores_dir, e))?;
    let mut placed = None;
    for entry in std::fs::read_dir(&out)
        .map_err(|e| Error::io("read", &out, e))?
        .flatten()
    {
        let name = entry.file_name();
        let dest = cores_dir.join(&name);
        std::fs::rename(entry.path(), &dest).map_err(|e| Error::io("rename", &dest, e))?;
        if name.to_string_lossy().contains("_libretro.") {
            placed = Some(dest);
        }
    }
    let _ = std::fs::remove_dir_all(&out);
    progress.on(Event::Placed);
    placed.ok_or_else(|| Error::Place {
        what: core.into(),
        why: "the archive held no core".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_follow_the_buildbot_layout() {
        assert_eq!(
            core_url(Os::Windows, "snes9x").unwrap(),
            "https://buildbot.libretro.com/nightly/windows/x86_64/latest/snes9x_libretro.dll.zip"
        );
        assert!(
            core_url(Os::Linux, "mgba")
                .unwrap()
                .ends_with("mgba_libretro.so.zip")
        );
        assert!(core_url(Os::Macos, "mgba").is_none());
    }
}
