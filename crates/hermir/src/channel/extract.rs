//! Archive extraction into a fresh directory. Zip, 7z and tar.gz in-process; an AppImage
//! unpacks itself, also when it is the only thing an archive held. When an archive wraps
//! everything in one top-level directory, that directory is stripped, so `exe` in the
//! catalog is always relative to the emulator's own root.
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

pub fn extract(archive: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).map_err(|e| Error::io("create", dest, e))?;
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(".zip") {
        unzip(archive, dest)?;
    } else if name.ends_with(".7z") {
        sevenz_rust2::decompress_file(archive, dest).map_err(|e| Error::Place {
            what: archive.display().to_string(),
            why: format!("7z: {e}"),
        })?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        untar(archive, dest)?;
    } else if name.ends_with(".appimage") {
        appimage(archive, dest)?;
    } else {
        return Err(Error::Place {
            what: archive.display().to_string(),
            why: "not a .zip, .7z, .tar.gz or .AppImage".into(),
        });
    }
    strip_single_top_dir(dest)?;
    if let Some(inner) = lone_appimage(dest) {
        appimage(&inner, dest)?;
        fs::remove_file(&inner).map_err(|e| Error::io("remove", &inner, e))?;
        strip_single_top_dir(dest)?;
    }
    Ok(())
}

/// The AppImage that is all an archive held (shadPS4 zips one).
fn lone_appimage(dest: &Path) -> Option<PathBuf> {
    let mut entries = fs::read_dir(dest).ok()?.flatten();
    let only = entries.next()?.path();
    if entries.next().is_some() || !only.is_file() {
        return None;
    }
    let lower = only.file_name()?.to_string_lossy().to_ascii_lowercase();
    lower.ends_with(".appimage").then_some(only)
}

fn untar(archive: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(archive).map_err(|e| Error::io("open", archive, e))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let entries = tar.entries().map_err(|e| Error::Place {
        what: archive.display().to_string(),
        why: format!("tar: {e}"),
    })?;
    for entry in entries {
        let mut entry = entry.map_err(|e| Error::Place {
            what: archive.display().to_string(),
            why: format!("tar entry: {e}"),
        })?;
        let rel = entry
            .path()
            .map(|p| p.into_owned())
            .map_err(|e| Error::Place {
                what: archive.display().to_string(),
                why: format!("tar entry name: {e}"),
            })?;
        if !is_safe_relative(&rel) {
            return Err(Error::Place {
                what: archive.display().to_string(),
                why: format!("entry {} escapes the archive", rel.display()),
            });
        }
        entry.unpack_in(dest).map_err(|e| Error::Place {
            what: archive.display().to_string(),
            why: format!("tar unpack {}: {e}", rel.display()),
        })?;
    }
    Ok(())
}

/// An AppImage unpacks itself (`--appimage-extract`), so the copy runs without FUSE. It was
/// digest-verified before this runs it. The classic runtime writes `squashfs-root/`; newer ones
/// write `AppDir/` with `squashfs-root` as a link to it, which goes so one folder is left.
#[cfg(unix)]
fn appimage(archive: &Path, dest: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let place = |why: String| Error::Place {
        what: archive.display().to_string(),
        why,
    };
    let exe = archive
        .canonicalize()
        .map_err(|e| Error::io("resolve", archive, e))?;
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755))
        .map_err(|e| Error::io("chmod", &exe, e))?;
    let out = std::process::Command::new(&exe)
        .arg("--appimage-extract")
        .current_dir(dest)
        .output()
        .map_err(|e| place(format!("run --appimage-extract: {e}")))?;
    let link = dest.join("squashfs-root");
    if link.is_symlink() && dest.join("AppDir").is_dir() {
        fs::remove_file(&link).map_err(|e| Error::io("remove", &link, e))?;
    }
    if !out.status.success() || !(link.is_dir() || dest.join("AppDir").is_dir()) {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(place(format!(
            "--appimage-extract: {}",
            err.lines()
                .last()
                .unwrap_or("left no squashfs-root or AppDir")
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn appimage(archive: &Path, _dest: &Path) -> Result<()> {
    Err(Error::Place {
        what: archive.display().to_string(),
        why: "an AppImage runs on Linux only".into(),
    })
}

fn unzip(archive: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(archive).map_err(|e| Error::io("open", archive, e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Place {
        what: archive.display().to_string(),
        why: format!("zip: {e}"),
    })?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| Error::Place {
            what: archive.display().to_string(),
            why: format!("zip entry {i}: {e}"),
        })?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(Error::Place {
                what: archive.display().to_string(),
                why: format!("entry {} escapes the archive", entry.name()),
            });
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| Error::io("create", &out, e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
        }
        let mut f = fs::File::create(&out).map_err(|e| Error::io("create", &out, e))?;
        io::copy(&mut entry, &mut f).map_err(|e| Error::io("write", &out, e))?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// `dest/Only-Dir/…` becomes `dest/…`.
fn strip_single_top_dir(dest: &Path) -> Result<()> {
    let entries: Vec<PathBuf> = fs::read_dir(dest)
        .map_err(|e| Error::io("read", dest, e))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    let [only] = entries.as_slice() else {
        return Ok(());
    };
    if !only.is_dir() {
        return Ok(());
    }
    let staging = dest.with_file_name(format!(
        "{}.strip",
        dest.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    ));
    let _ = fs::remove_dir_all(&staging);
    fs::rename(only, &staging).map_err(|e| Error::io("rename", only, e))?;
    fs::remove_dir(dest).map_err(|e| Error::io("remove", dest, e))?;
    fs::rename(&staging, dest).map_err(|e| Error::io("rename", &staging, e))
}

/// True when `rel` stays inside its root: no `..`, no absolute prefix.
pub fn is_safe_relative(rel: &Path) -> bool {
    rel.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, bytes) in entries {
                w.start_file(*name, opts).unwrap();
                w.write_all(bytes).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn strips_a_single_top_dir_and_keeps_flat_archives() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.zip");
        fs::write(&a, zip_with(&[("Top/x.exe", b"1"), ("Top/sub/y", b"2")])).unwrap();
        let out = dir.path().join("out");
        extract(&a, &out).unwrap();
        assert!(out.join("x.exe").exists() && out.join("sub/y").exists());

        let b = dir.path().join("b.zip");
        fs::write(&b, zip_with(&[("x.exe", b"1"), ("sub/y", b"2")])).unwrap();
        let out2 = dir.path().join("out2");
        extract(&b, &out2).unwrap();
        assert!(out2.join("x.exe").exists());
    }

    /// A stand-in AppImage: a script that unpacks the way the real runtime does.
    #[cfg(unix)]
    #[test]
    fn an_appimage_unpacks_itself_and_its_root_is_lifted() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("rpcs3-v0.0.42_linux64.AppImage");
        fs::write(
            &a,
            "#!/bin/sh\n[ \"$1\" = --appimage-extract ] || exit 2\nmkdir -p squashfs-root/usr/bin\necho run > squashfs-root/AppRun\n",
        )
        .unwrap();
        let out = dir.path().join("out");
        extract(&a, &out).unwrap();
        assert!(out.join("AppRun").is_file() && out.join("usr/bin").is_dir());

        // The newer runtime (RPCS3's, 2026): `AppDir/` plus a `squashfs-root` link to it.
        let b = dir.path().join("new_linux64.AppImage");
        fs::write(
            &b,
            "#!/bin/sh\nmkdir -p AppDir\necho run > AppDir/AppRun\nln -s ./AppDir squashfs-root\n",
        )
        .unwrap();
        let out2 = dir.path().join("out2");
        extract(&b, &out2).unwrap();
        assert!(out2.join("AppRun").is_file());
    }

    #[test]
    fn refuses_unknown_and_escaping() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("a.tar");
        fs::write(&t, b"").unwrap();
        assert!(matches!(
            extract(&t, &dir.path().join("o")),
            Err(Error::Place { .. })
        ));
        assert!(!is_safe_relative(Path::new("../x")));
        assert!(is_safe_relative(Path::new("Dolphin-x64/Dolphin.exe")));
    }
}
