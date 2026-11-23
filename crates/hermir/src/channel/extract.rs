//! Archive extraction into a fresh directory. Zip, 7z, tar.gz and tar.xz in-process; an
//! AppImage unpacks itself, also when it is the only thing an archive held; a disk image is
//! attached and copied out (macOS). When an archive wraps everything in one top-level
//! directory, that directory is stripped, so `exe` in the catalog is always relative to the
//! emulator's own root; a lone `X.app` stays, being the app.
//!
//! Nothing lands outside `dest`: an entry that names a path outside it, a link whose target
//! leaves it, an entry written through a link, and a device or pipe all refuse the archive.
use std::fs;
use std::io::{self, Read};
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
        un7z(archive, dest)?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        untar(archive, dest, flate2::read::GzDecoder::new)?;
    } else if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        untar(archive, dest, |f| lzma_rust2::XzReader::new(f, true))?;
    } else if name.ends_with(".dmg") {
        dmg(archive, dest)?;
    } else if name.ends_with(".appimage") {
        appimage(archive, dest)?;
    } else {
        return Err(Error::Place {
            what: archive.display().to_string(),
            why: "not a .zip, .7z, .tar.gz, .tar.xz, .dmg or .AppImage".into(),
        });
    }
    drop_apple_doubles(dest)?;
    strip_single_top_dir(dest)?;
    if let Some(inner) = lone_appimage(dest) {
        appimage(&inner, dest)?;
        fs::remove_file(&inner).map_err(|e| Error::io("remove", &inner, e))?;
        strip_single_top_dir(dest)?;
    }
    Ok(())
}

/// A 7z, with the Unix mode an entry's attributes carry when bit 15 is set (high 16 bits, as
/// p7zip writes it): a link is recreated when it stays inside, a file keeps whether it runs. A
/// Mac app's frameworks are links, and its exe must run.
fn un7z(archive: &Path, dest: &Path) -> Result<()> {
    let stop = |why: String| sevenz_rust2::Error::Other(why.into());
    sevenz_rust2::decompress_file_with_extract_fn(archive, dest, |entry, reader, out| {
        let rel = out.strip_prefix(dest).unwrap_or(out);
        if through_link(dest, rel) {
            return Err(stop(format!("entry {} lies through a link", rel.display())));
        }
        let unix = entry.has_windows_attributes && entry.windows_attributes & 0x8000 != 0;
        let mode = if unix {
            entry.windows_attributes >> 16
        } else {
            0
        };
        if mode & 0o170000 == 0o120000 {
            let mut target = String::new();
            reader.read_to_string(&mut target)?;
            let base = rel.parent().unwrap_or(Path::new(""));
            if target.is_empty() || !link_stays_inside(base, Path::new(&target)) {
                let why = format!("link {} -> {target} leaves the archive", rel.display());
                return Err(stop(why));
            }
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, out)?;
            #[cfg(not(unix))]
            fs::write(out, &target)?;
            return Ok(true);
        }
        let done = sevenz_rust2::default_entry_extract_fn(entry, reader, out)?;
        #[cfg(unix)]
        if mode & 0o111 != 0 && !entry.is_directory() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(out, fs::Permissions::from_mode(0o755))?;
        }
        Ok(done)
    })
    .map_err(|e| refuse(archive, format!("7z: {e}")))
}

/// What macOS archivers add beside the real files: a zip's `__MACOSX/` of resource forks and
/// a tar's `._name` AppleDouble files. Neither is part of the release, and a stray file inside
/// an app breaks its signature.
fn drop_apple_doubles(dir: &Path) -> Result<()> {
    for e in fs::read_dir(dir).map_err(|e| Error::io("read", dir, e))? {
        let e = e.map_err(|e| Error::io("read", dir, e))?;
        let (path, name) = (e.path(), e.file_name());
        let is_dir = e
            .file_type()
            .map_err(|e| Error::io("stat", &path, e))?
            .is_dir();
        if is_dir && name == "__MACOSX" {
            fs::remove_dir_all(&path).map_err(|e| Error::io("remove", &path, e))?;
        } else if is_dir {
            drop_apple_doubles(&path)?;
        } else if name.to_string_lossy().starts_with("._") {
            fs::remove_file(&path).map_err(|e| Error::io("remove", &path, e))?;
        }
    }
    Ok(())
}

/// A disk image is attached read-only and out of sight, and what it shows is copied out: its
/// hidden files and the `Applications` link of a drag-to-install image stay behind. An image
/// that asks for a licence agreement first is refused, as stdin is closed. The mount point is
/// in the temp dir: `hdiutil` refuses one on an external volume.
#[cfg(target_os = "macos")]
fn dmg(archive: &Path, dest: &Path) -> Result<()> {
    use std::process::{Command, Stdio};
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    let mount = std::env::temp_dir().join(format!("hermir-{}-{name}.mount", std::process::id()));
    fs::create_dir_all(&mount).map_err(|e| Error::io("create", &mount, e))?;
    let out = Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noautoopen",
            "-noverify",
        ])
        .arg("-mountpoint")
        .arg(&mount)
        .arg(archive)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| refuse(archive, format!("run hdiutil: {e}")))?;
    if !out.status.success() {
        let _ = fs::remove_dir(&mount);
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(refuse(
            archive,
            format!("hdiutil attach: {}", err.lines().last().unwrap_or("failed")),
        ));
    }
    let copied = copy_volume(archive, &mount, dest);
    let _ = Command::new("hdiutil")
        .args(["detach", "-force"])
        .arg(&mount)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = fs::remove_dir(&mount);
    copied
}

#[cfg(not(target_os = "macos"))]
fn dmg(archive: &Path, _dest: &Path) -> Result<()> {
    Err(refuse(archive, "a disk image opens on macOS only".into()))
}

/// The visible top level of a mounted image into `dest`.
#[cfg(target_os = "macos")]
fn copy_volume(archive: &Path, volume: &Path, dest: &Path) -> Result<()> {
    for e in fs::read_dir(volume).map_err(|e| Error::io("read", volume, e))? {
        let e = e.map_err(|e| Error::io("read", volume, e))?;
        let name = e.file_name();
        let link = e.file_type().is_ok_and(|t| t.is_symlink());
        if link || name.to_string_lossy().starts_with('.') {
            continue;
        }
        copy_tree(archive, &e.path(), &dest.join(&name), Path::new(&name))?;
    }
    Ok(())
}

/// `src` to `out`, links recreated as they are when they stay under the root (`rel` is `src`
/// relative to it), files copied with their mode.
#[cfg(target_os = "macos")]
fn copy_tree(archive: &Path, src: &Path, out: &Path, rel: &Path) -> Result<()> {
    let kind = fs::symlink_metadata(src)
        .map_err(|e| Error::io("stat", src, e))?
        .file_type();
    if kind.is_symlink() {
        let target = fs::read_link(src).map_err(|e| Error::io("readlink", src, e))?;
        let base = rel.parent().unwrap_or(Path::new(""));
        if target.as_os_str().is_empty() || !link_stays_inside(base, &target) {
            return Err(refuse(
                archive,
                format!(
                    "link {} -> {} leaves the image",
                    rel.display(),
                    target.display()
                ),
            ));
        }
        std::os::unix::fs::symlink(&target, out).map_err(|e| Error::io("link", out, e))
    } else if kind.is_dir() {
        fs::create_dir_all(out).map_err(|e| Error::io("create", out, e))?;
        for e in fs::read_dir(src).map_err(|e| Error::io("read", src, e))? {
            let name = e.map_err(|e| Error::io("read", src, e))?.file_name();
            copy_tree(
                archive,
                &src.join(&name),
                &out.join(&name),
                &rel.join(&name),
            )?;
        }
        Ok(())
    } else if kind.is_file() {
        fs::copy(src, out).map_err(|e| Error::io("copy", src, e))?;
        Ok(())
    } else {
        Err(refuse(
            archive,
            format!("entry {} is a device or a pipe", rel.display()),
        ))
    }
}

/// The AppImage that is all an archive held (shadPS4 zips one). A link is not one.
fn lone_appimage(dest: &Path) -> Option<PathBuf> {
    let mut entries = fs::read_dir(dest).ok()?.flatten();
    let only = entries.next()?.path();
    if entries.next().is_some() || !fs::symlink_metadata(&only).ok()?.is_file() {
        return None;
    }
    let lower = only.file_name()?.to_string_lossy().to_ascii_lowercase();
    lower.ends_with(".appimage").then_some(only)
}

fn refuse(archive: &Path, why: String) -> Error {
    Error::Place {
        what: archive.display().to_string(),
        why,
    }
}

/// Whether a link at `base` (a folder relative to the root) pointing at `target` stays under
/// the root, `..` resolved as it goes.
fn link_stays_inside(base: &Path, target: &Path) -> bool {
    let mut depth = base
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count();
    for c in target.components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

/// Whether `rel`, or a folder on its way, is a link already in `dest`: writing there would
/// write wherever the link points. Off Unix a zip's link is a plain file, so a file where
/// `rel` needs a folder counts too.
fn through_link(dest: &Path, rel: &Path) -> bool {
    let mut at = dest.to_path_buf();
    let mut parts = rel.components().peekable();
    while let Some(c) = parts.next() {
        at.push(c);
        match fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => return true,
            Ok(m) if m.is_file() && parts.peek().is_some() => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

fn untar<R: Read>(archive: &Path, dest: &Path, decode: impl FnOnce(fs::File) -> R) -> Result<()> {
    let file = fs::File::open(archive).map_err(|e| Error::io("open", archive, e))?;
    let mut tar = tar::Archive::new(decode(file));
    let entries = tar
        .entries()
        .map_err(|e| refuse(archive, format!("tar: {e}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| refuse(archive, format!("tar entry: {e}")))?;
        let rel = entry
            .path()
            .map(|p| p.into_owned())
            .map_err(|e| refuse(archive, format!("tar entry name: {e}")))?;
        if !is_safe_relative(&rel) {
            return Err(refuse(
                archive,
                format!("entry {} escapes the archive", rel.display()),
            ));
        }
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            let target = entry
                .link_name()
                .map_err(|e| refuse(archive, format!("tar link {}: {e}", rel.display())))?
                .map(|t| t.into_owned())
                .unwrap_or_default();
            // A symlink is read from its own folder, a hard link from the archive's root.
            let base = match kind.is_symlink() {
                true => rel.parent().unwrap_or(Path::new("")),
                false => Path::new(""),
            };
            if target.as_os_str().is_empty() || !link_stays_inside(base, &target) {
                return Err(refuse(
                    archive,
                    format!(
                        "link {} -> {} leaves the archive",
                        rel.display(),
                        target.display()
                    ),
                ));
            }
        } else if kind.is_character_special() || kind.is_block_special() || kind.is_fifo() {
            return Err(refuse(
                archive,
                format!("entry {} is a device or a pipe", rel.display()),
            ));
        }
        if through_link(dest, &rel) {
            return Err(refuse(
                archive,
                format!("entry {} lies through a link", rel.display()),
            ));
        }
        entry
            .unpack_in(dest)
            .map_err(|e| refuse(archive, format!("tar unpack {}: {e}", rel.display())))?;
    }
    Ok(())
}

/// An AppImage unpacks itself (`--appimage-extract`), so the copy runs without FUSE. It runs
/// here as the copy will run later, after the digest check when the channel publishes one.
/// The classic runtime writes `squashfs-root/`; newer ones write `AppDir/` with
/// `squashfs-root` as a link to it, which goes so one folder is left.
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
    let mut zip = zip::ZipArchive::new(file).map_err(|e| refuse(archive, format!("zip: {e}")))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| refuse(archive, format!("zip entry {i}: {e}")))?;
        let name = entry
            .name()
            .map_err(|e| refuse(archive, format!("zip entry {i}: {e}")))?;
        // The name as stored, `\` read as a separator: no `..`, no root, no drive.
        let rel = PathBuf::from(name.replace('\\', "/"));
        if name.contains('\0') || !is_safe_relative(&rel) {
            return Err(refuse(archive, format!("entry {name} escapes the archive")));
        }
        if through_link(dest, &rel) {
            return Err(refuse(
                archive,
                format!("entry {} lies through a link", rel.display()),
            ));
        }
        let out = dest.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| Error::io("create", &out, e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
        }
        if entry.is_symlink() {
            // The target is the entry's content.
            let mut target = String::new();
            entry
                .read_to_string(&mut target)
                .map_err(|e| refuse(archive, format!("zip link {}: {e}", rel.display())))?;
            let base = rel.parent().unwrap_or(Path::new(""));
            if target.is_empty() || !link_stays_inside(base, Path::new(&target)) {
                return Err(refuse(
                    archive,
                    format!("link {} -> {target} leaves the archive", rel.display()),
                ));
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &out).map_err(|e| Error::io("link", &out, e))?;
            // Elsewhere a file that holds the target, as other unzippers write it.
            #[cfg(not(unix))]
            fs::write(&out, &target).map_err(|e| Error::io("write", &out, e))?;
            continue;
        }
        let mut f = fs::File::create(&out).map_err(|e| Error::io("create", &out, e))?;
        io::copy(&mut entry, &mut f).map_err(|e| Error::io("write", &out, e))?;
        // Only whether it runs: no setuid, no world-writable, whatever the archive said.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let mode = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
            let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// `dest/Only-Dir/…` becomes `dest/…`. A link is never stripped, whatever it points at, and
/// neither is an app: `X.app/Contents/…` without its folder is no app.
fn strip_single_top_dir(dest: &Path) -> Result<()> {
    let entries: Vec<PathBuf> = fs::read_dir(dest)
        .map_err(|e| Error::io("read", dest, e))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    let [only] = entries.as_slice() else {
        return Ok(());
    };
    let app = only
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"));
    if app || !fs::symlink_metadata(only).is_ok_and(|m| m.is_dir()) {
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

    /// One tar entry, named as given: the builder's own checks are bypassed on purpose.
    #[derive(Clone, Copy)]
    enum T<'a> {
        File(&'a str, &'a [u8]),
        Dir(&'a str),
        Link(&'a str, &'a str),
        Hard(&'a str, &'a str),
        Fifo(&'a str),
    }

    fn tar_gz(entries: &[T]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar_of(entries)).unwrap();
        gz.finish().unwrap()
    }

    fn tar_xz(entries: &[T]) -> Vec<u8> {
        let mut xz = lzma_rust2::XzWriter::new(Vec::new(), Default::default()).unwrap();
        xz.write_all(&tar_of(entries)).unwrap();
        xz.finish().unwrap()
    }

    fn tar_of(entries: &[T]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for e in entries {
            let (name, kind, data, link): (&str, tar::EntryType, &[u8], Option<&str>) = match *e {
                T::File(n, d) => (n, tar::EntryType::Regular, d, None),
                T::Dir(n) => (n, tar::EntryType::Directory, b"", None),
                T::Link(n, t) => (n, tar::EntryType::Symlink, b"", Some(t)),
                T::Hard(n, t) => (n, tar::EntryType::Link, b"", Some(t)),
                T::Fifo(n) => (n, tar::EntryType::Fifo, b"", None),
            };
            let mut h = tar::Header::new_gnu();
            h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            if let Some(t) = link {
                h.set_link_name_literal(t).unwrap();
            }
            h.set_entry_type(kind);
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append(&h, data).unwrap();
        }
        b.into_inner().unwrap()
    }

    /// A zip of files and links, named as given.
    fn zip_links(files: &[(&str, &[u8])], links: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, bytes) in files {
                w.start_file(*name, opts).unwrap();
                w.write_all(bytes).unwrap();
            }
            for (name, target) in links {
                w.add_symlink(*name, *target, opts).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    fn sevenz_with(name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut w = sevenz_rust2::ArchiveWriter::new(io::Cursor::new(Vec::new())).unwrap();
        w.push_archive_entry(sevenz_rust2::ArchiveEntry::new_file(name), Some(bytes))
            .unwrap();
        w.finish().unwrap().into_inner()
    }

    /// A 7z of entries with a Unix mode each, as p7zip writes them on a Mac.
    fn sevenz_unix(entries: &[(&str, u32, &[u8])]) -> Vec<u8> {
        let mut w = sevenz_rust2::ArchiveWriter::new(io::Cursor::new(Vec::new())).unwrap();
        for (name, mode, bytes) in entries {
            let mut e = sevenz_rust2::ArchiveEntry::new_file(name);
            e.has_windows_attributes = true;
            e.windows_attributes = 0x8000 | (mode << 16);
            w.push_archive_entry(e, Some(*bytes)).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[cfg(unix)]
    #[test]
    fn a_7z_keeps_links_and_whether_a_file_runs() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, out, r) = try_extract(
            "rpcs3_macos.7z",
            &sevenz_unix(&[
                ("RPCS3.app/Contents/MacOS/rpcs3", 0o100755, b"run"),
                ("RPCS3.app/Contents/Info.plist", 0o100644, b"plist"),
                (
                    "RPCS3.app/Contents/Frameworks/Qt.framework/Versions/A/Qt",
                    0o100755,
                    b"lib",
                ),
                (
                    "RPCS3.app/Contents/Frameworks/Qt.framework/Versions/Current",
                    0o120755,
                    b"A",
                ),
            ]),
        );
        r.unwrap();
        let mode = |p: &str| fs::metadata(out.join(p)).unwrap().permissions().mode() & 0o111;
        assert_eq!(mode("RPCS3.app/Contents/MacOS/rpcs3"), 0o111);
        assert_eq!(mode("RPCS3.app/Contents/Info.plist"), 0);
        let current = out.join("RPCS3.app/Contents/Frameworks/Qt.framework/Versions/Current");
        assert_eq!(fs::read_link(current).unwrap(), Path::new("A"));
        refused("a.7z", &sevenz_unix(&[("Top", 0o120755, b"..")]));
    }

    /// Extracts `bytes` as `<name>` into `<tmp>/in/out`; `<tmp>` itself is "outside".
    fn try_extract(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf, Result<()>) {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join(name);
        fs::write(&a, bytes).unwrap();
        let out = dir.path().join("in/out");
        let r = extract(&a, &out);
        (dir, out, r)
    }

    fn refused(name: &str, bytes: &[u8]) {
        let (dir, _, r) = try_extract(name, bytes);
        assert!(matches!(r, Err(Error::Place { .. })), "{name}: {r:?}");
        for stray in ["evil", "in/evil"] {
            assert!(!dir.path().join(stray).exists(), "{name} wrote {stray}");
        }
    }

    #[test]
    fn entries_outside_the_tree_are_refused() {
        let abs = std::env::temp_dir().join("hermir-abs-evil");
        let abs = abs.to_str().unwrap();
        refused("a.tar.gz", &tar_gz(&[T::File("../evil", b"x")]));
        refused("a.tar.gz", &tar_gz(&[T::File("ok/../../evil", b"x")]));
        refused("a.tar.gz", &tar_gz(&[T::File(abs, b"x")]));
        refused("a.zip", &zip_with(&[("../evil", b"x")]));
        refused("a.zip", &zip_with(&[("ok/../../evil", b"x")]));
        refused("a.zip", &zip_with(&[(abs, b"x")]));
        refused("a.7z", &sevenz_with("../evil", b"x"));
        refused("a.7z", &sevenz_with(abs, b"x"));
        assert!(!Path::new(abs).exists());
    }

    #[test]
    fn links_that_leave_the_tree_are_refused() {
        refused("a.tar.gz", &tar_gz(&[T::Link("evil", "../../outside")]));
        refused("a.tar.gz", &tar_gz(&[T::Link("sub/evil", "../../x")]));
        refused("a.tar.gz", &tar_gz(&[T::Link("evil", "/etc")]));
        refused(
            "a.tar.gz",
            &tar_gz(&[T::File("x", b"1"), T::Hard("evil", "../x")]),
        );
        refused("a.tar.gz", &tar_gz(&[T::Hard("evil", "/etc/passwd")]));
        refused("a.tar.gz", &tar_gz(&[T::Fifo("pipe")]));
        refused("a.zip", &zip_links(&[], &[("evil", "../../outside")]));
        refused("a.zip", &zip_links(&[], &[("evil", "/etc")]));
        // A link that stays inside, then an entry written through it.
        refused(
            "a.tar.gz",
            &tar_gz(&[T::Dir("sub"), T::Link("d", "sub"), T::File("d/x", b"1")]),
        );
        refused(
            "a.zip",
            &zip_links_then_file(&[("d", "sub")], ("d/x", b"1")),
        );
    }

    fn zip_links_then_file(links: &[(&str, &str)], file: (&str, &[u8])) -> Vec<u8> {
        let mut buf = io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, target) in links {
                w.add_symlink(*name, *target, opts).unwrap();
            }
            w.start_file(file.0, opts).unwrap();
            w.write_all(file.1).unwrap();
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    /// An AppImage-like tree: links inside it are kept as links.
    #[cfg(unix)]
    #[test]
    fn links_inside_the_tree_are_kept() {
        let (_dir, out, r) = try_extract(
            "a.tar.gz",
            &tar_gz(&[
                T::File("usr/bin/emu", b"elf"),
                T::Link("AppRun", "usr/bin/emu"),
                T::Dir("usr/lib"),
                T::Link("usr/lib64", "lib"),
                T::Link("usr/share/emu", "../bin/emu"),
                T::Hard("usr/bin/emu2", "usr/bin/emu"),
            ]),
        );
        r.unwrap();
        assert_eq!(
            fs::read_link(out.join("AppRun")).unwrap(),
            Path::new("usr/bin/emu")
        );
        assert!(
            fs::symlink_metadata(out.join("usr/lib64"))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(fs::read(out.join("usr/bin/emu2")).unwrap(), b"elf");

        let (_dir, out, r) = try_extract(
            "a.zip",
            &zip_links(&[("bin/emu", b"elf")], &[("AppRun", "bin/emu")]),
        );
        r.unwrap();
        assert_eq!(
            fs::read_link(out.join("AppRun")).unwrap(),
            Path::new("bin/emu")
        );
    }

    /// A lone top-level link is never taken for the archive's folder.
    #[cfg(unix)]
    #[test]
    fn a_single_top_level_link_is_not_stripped() {
        let (_dir, out, r) = try_extract("a.tar.gz", &tar_gz(&[T::Link("Top", ".")]));
        r.unwrap();
        assert!(
            fs::symlink_metadata(&out).unwrap().is_dir(),
            "still a folder"
        );
        assert!(fs::symlink_metadata(out.join("Top")).unwrap().is_symlink());
        refused("a.tar.gz", &tar_gz(&[T::Link("Top", "/")]));
        refused("a.zip", &zip_links(&[], &[("Top", "..")]));
    }

    #[cfg(unix)]
    #[test]
    fn unzip_keeps_only_whether_a_file_runs() {
        use std::os::unix::fs::PermissionsExt;
        let mut buf = io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("run", opts.unix_permissions(0o4777)).unwrap();
            w.write_all(b"x").unwrap();
            w.start_file("data", opts.unix_permissions(0o666)).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let (_dir, out, r) = try_extract("a.zip", &buf.into_inner());
        r.unwrap();
        let mode = |n: &str| fs::metadata(out.join(n)).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode("run"), 0o755);
        assert_eq!(mode("data"), 0o644);
    }

    #[test]
    fn a_lone_app_stays_an_app_and_apple_doubles_go() {
        let (_dir, out, r) = try_extract(
            "emu-macos.zip",
            &zip_with(&[
                ("Emu.app/Contents/MacOS/Emu", b"run"),
                ("Emu.app/Contents/Info.plist", b"plist"),
                ("__MACOSX/Emu.app/Contents/._Info.plist", b"fork"),
            ]),
        );
        r.unwrap();
        assert!(out.join("Emu.app/Contents/MacOS/Emu").is_file());
        assert!(!out.join("__MACOSX").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_tar_xz_unpacks_as_a_tar_gz_does() {
        let (_dir, out, r) = try_extract(
            "pcsx2-v2-macos-Qt.tar.xz",
            &tar_xz(&[
                T::File("PCSX2-v2/PCSX2.app/Contents/MacOS/PCSX2", b"run"),
                T::File("PCSX2-v2/PCSX2.app/Contents/MacOS/._PCSX2", b"xattrs"),
                T::Link("PCSX2-v2/PCSX2.app/Contents/Current", "MacOS"),
            ]),
        );
        r.unwrap();
        let app = out.join("PCSX2.app/Contents");
        assert!(app.join("MacOS/PCSX2").is_file());
        assert!(!app.join("MacOS/._PCSX2").exists());
        assert!(
            fs::symlink_metadata(app.join("Current"))
                .unwrap()
                .is_symlink()
        );
        refused("a.tar.xz", &tar_xz(&[T::File("../evil", b"x")]));
    }

    /// A drag-to-install image, built by `hdiutil` as release managers do.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_disk_image_gives_its_app_without_the_applications_link() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let macos = src.join("Emu.app/Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        fs::write(macos.join("Emu"), b"run").unwrap();
        fs::set_permissions(macos.join("Emu"), fs::Permissions::from_mode(0o755)).unwrap();
        symlink("MacOS", src.join("Emu.app/Contents/Current")).unwrap();
        symlink("/Applications", src.join("Applications")).unwrap();
        fs::write(src.join(".VolumeIcon.icns"), b"icon").unwrap();
        let dmg = dir.path().join("Emu-macos.dmg");
        let made = std::process::Command::new("hdiutil")
            .args([
                "create",
                "-quiet",
                "-fs",
                "HFS+",
                "-format",
                "UDZO",
                "-srcfolder",
            ])
            .arg(&src)
            .arg(&dmg)
            .status()
            .unwrap();
        assert!(made.success());
        let out = dir.path().join("out");
        extract(&dmg, &out).unwrap();
        let exe = out.join("Emu.app/Contents/MacOS/Emu");
        assert_eq!(
            fs::metadata(&exe).unwrap().permissions().mode() & 0o111,
            0o111
        );
        assert!(
            fs::symlink_metadata(out.join("Emu.app/Contents/Current"))
                .unwrap()
                .is_symlink()
        );
        assert!(!out.join("Applications").exists() && !out.join(".VolumeIcon.icns").exists());
        let mounts = fs::read_dir(std::env::temp_dir()).unwrap().flatten();
        assert!(
            !mounts
                .into_iter()
                .any(|e| e.file_name().to_string_lossy().ends_with("out.mount"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_link_out_of_an_image_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let vol = dir.path().join("vol");
        fs::create_dir_all(vol.join("Emu.app")).unwrap();
        std::os::unix::fs::symlink("../../..", vol.join("Emu.app/up")).unwrap();
        let out = dir.path().join("out");
        let r = copy_volume(Path::new("x.dmg"), &vol, &out);
        assert!(matches!(r, Err(Error::Place { .. })), "{r:?}");
    }
}
