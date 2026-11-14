//! libretro cores from the buildbot, into a RetroArch cores directory: the same URL
//! RetroArch's own core updater fetches, so a core that "isn't installed" is one call away.
use std::path::Path;

use crate::channel::extract::extract;
use crate::channel::http::Http;
use crate::error::{Error, Result};
use crate::model::Os;
use crate::progress::{Event, Progress};

/// Why a core is never verified.
pub const UNVERIFIED: &str = "the libretro buildbot publishes no checksums";

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

/// Downloads and unpacks `<core>_libretro.<so|dll>` into `cores_dir`, through `tmp`. Returns the
/// core's path.
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
    http.download(&url, &part, None, progress)?;
    progress.on(Event::NotVerified {
        why: UNVERIFIED.into(),
    });
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
        place(&entry.path(), &dest).map_err(|e| Error::io("place", &dest, e))?;
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

/// `from` copied to `dest` through a new file beside it. A new file takes the cores folder's
/// ACL; one renamed in from `tmp` keeps tmp's, which RetroArch's user may not be able to read.
fn place(from: &Path, dest: &Path) -> std::io::Result<()> {
    let part = dest.with_extension("hermir-part");
    let _ = std::fs::remove_file(&part);
    let placed = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .and_then(|mut to| std::io::copy(&mut std::fs::File::open(from)?, &mut to))
        .and_then(|_| std::fs::rename(&part, dest));
    if placed.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    placed
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

    /// A core replaces the old one and leaves nothing behind. On Windows it carries the cores
    /// folder's inherited ACL, not the one of `tmp` it was unpacked in.
    #[test]
    fn a_core_lands_as_a_new_file_of_the_cores_folder() {
        use crate::channel::http::fake::FakeHttp;
        use std::io::Write;

        let mut zip = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut zip);
            w.start_file(
                "snes9x_libretro.dll",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            w.write_all(b"new").unwrap();
            w.finish().unwrap();
        }
        let mut http = FakeHttp::default();
        let url = core_url(Os::Windows, "snes9x").unwrap();
        http.files.insert(url, zip.into_inner());
        let root = tempfile::tempdir().unwrap();
        let (tmp, cores) = (root.path().join("tmp"), root.path().join("cores"));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::create_dir_all(&cores).unwrap();
        std::fs::write(cores.join("snes9x_libretro.dll"), b"old").unwrap();
        #[cfg(windows)]
        {
            let icacls = |p: &Path, args: &[&str]| {
                let s = std::process::Command::new("icacls")
                    .arg(p)
                    .args(args)
                    .status();
                assert!(s.unwrap().success(), "icacls {} {args:?}", p.display());
            };
            let me = format!("{}:(OI)(CI)F", std::env::var("USERNAME").unwrap());
            icacls(
                &tmp,
                &["/inheritance:r", "/grant:r", &me, "*S-1-5-18:(OI)(CI)F"],
            );
            icacls(&cores, &["/grant", "*S-1-5-32-545:(OI)(CI)RX"]);
        }

        let quiet = crate::progress::Quiet;
        let core = install_core(&http, Os::Windows, "snes9x", &cores, &tmp, &quiet).unwrap();

        assert_eq!(core, cores.join("snes9x_libretro.dll"));
        assert_eq!(std::fs::read(&core).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&cores).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
        #[cfg(windows)]
        {
            let path = core.display().to_string().replace('\'', "''");
            let sddl = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command"])
                .arg(format!("(Get-Acl -LiteralPath '{path}').Sddl"))
                .output()
                .unwrap();
            let sddl = String::from_utf8_lossy(&sddl.stdout);
            assert!(
                sddl.contains(";;;BU)"),
                "no Users grant from the cores folder: {sddl}"
            );
        }
    }
}
