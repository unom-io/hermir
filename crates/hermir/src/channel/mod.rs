//! Getting an emulator onto the machine: resolve where the channel points, fetch, verify,
//! extract, place, record. Archive channels share one path; Flatpak delegates to `flatpak`.
pub mod extract;
pub mod flatpak;
pub mod github;
pub mod http;
pub mod libretro;
pub mod url;

use std::path::Path;

use crate::error::{Error, Result};
use crate::model::{Channel, Entry, Exe, Installed, Os, Portable, Resolved};
use crate::progress::{Event, Progress};
use crate::store::{Store, now_rfc3339};
use flatpak::Runner;
use http::Http;

/// The channel for `entry` on `os`, or the reason there is none.
pub fn channel_for(entry: &Entry, os: Os) -> Result<&Channel> {
    entry
        .channels
        .get(os)
        .ok_or_else(|| match &entry.no_install {
            Some(why) => Error::Policy {
                emulator: entry.id.clone(),
                why: why.clone(),
            },
            None => Error::UnsupportedOs {
                emulator: entry.id.clone(),
                os: os.to_string(),
            },
        })
}

/// Where the channel points right now. No download.
pub fn resolve(entry: &Entry, os: Os, http: &dyn Http) -> Result<Resolved> {
    Ok(match channel_for(entry, os)? {
        Channel::Flatpak { flatpak } => Resolved {
            emulator: entry.id.clone(),
            channel: "flatpak".into(),
            url: Some(format!("https://flathub.org/apps/{flatpak}")),
            file_name: None,
            size: None,
            version: None,
            release: format!("flathub:{flatpak}"),
            digest: None,
        },
        Channel::Github { github, asset, .. } => github::resolve(http, &entry.id, github, asset)?,
        Channel::Url {
            url: u,
            version,
            sha256,
            ..
        } => url::resolve(&entry.id, u, version, sha256.as_deref()),
    })
}

/// Installs (or reinstalls) `entry` into `store`. The caller holds the store lock.
pub fn install(
    entry: &Entry,
    os: Os,
    store: &Store,
    http: &dyn Http,
    runner: &dyn Runner,
    progress: &dyn Progress,
) -> Result<Installed> {
    let channel = channel_for(entry, os)?;
    let resolved = resolve(entry, os, http)?;
    progress.on(Event::Resolved {
        release: resolved.release.clone(),
        file_name: resolved.file_name.clone(),
        size: resolved.size,
    });
    let row = match channel {
        Channel::Flatpak { flatpak } => {
            flatpak::install(runner, flatpak)?;
            progress.on(Event::Placed);
            Installed {
                emulator: entry.id.clone(),
                channel: "flatpak".into(),
                exe: Exe::FlatpakRun(flatpak.clone()),
                version: flatpak::version(runner, flatpak),
                release: Some(resolved.release),
                digest: None,
                installed_at: now_rfc3339(),
            }
        }
        Channel::Github { exe, portable, .. } | Channel::Url { exe, portable, .. } => {
            install_archive(
                entry,
                &resolved,
                exe,
                portable.as_ref(),
                store,
                http,
                progress,
            )?
        }
    };
    store.record(row.clone())?;
    Ok(row)
}

fn install_archive(
    entry: &Entry,
    resolved: &Resolved,
    exe_rel: &str,
    portable: Option<&Portable>,
    store: &Store,
    http: &dyn Http,
    progress: &dyn Progress,
) -> Result<Installed> {
    let url = resolved.url.as_deref().ok_or_else(|| Error::Catalog {
        entry: entry.id.clone(),
        why: "channel resolved without a URL".into(),
    })?;
    let file_name = resolved
        .file_name
        .clone()
        .unwrap_or_else(|| format!("{}.archive", entry.id));
    let tmp = store.tmp_dir();
    std::fs::create_dir_all(&tmp).map_err(|e| Error::io("create", &tmp, e))?;
    // The release id is in the part name, so a stale part from another release never resumes.
    let part = tmp.join(format!("{}.{}.part", file_name, safe(&resolved.release)));
    http.download(url, &part, progress)?;

    progress.on(Event::Verifying);
    let actual = http::sha256_file(&part)?;
    if let Some(expected) = &resolved.digest
        && !expected.eq_ignore_ascii_case(&actual)
    {
        let _ = std::fs::remove_file(&part);
        return Err(Error::Verify {
            what: file_name,
            expected: expected.clone(),
            actual,
        });
    }
    let archive = tmp.join(&file_name);
    std::fs::rename(&part, &archive).map_err(|e| Error::io("rename", &archive, e))?;

    progress.on(Event::Extracting);
    let fresh = tmp.join(format!("{}.extract", entry.id));
    let _ = std::fs::remove_dir_all(&fresh);
    let extracted = extract::extract(&archive, &fresh);
    let _ = std::fs::remove_file(&archive);
    extracted?;
    if !extract::is_safe_relative(Path::new(exe_rel)) {
        return Err(Error::Catalog {
            entry: entry.id.clone(),
            why: format!("exe {exe_rel} is not a relative path"),
        });
    }
    if !fresh.join(exe_rel).is_file() {
        let top: Vec<String> = std::fs::read_dir(&fresh)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(Error::Place {
            what: file_name,
            why: format!(
                "{exe_rel} not in the archive; top level: {}",
                top.join(", ")
            ),
        });
    }
    match portable {
        Some(Portable::File { file }) => {
            let p = fresh.join(file);
            std::fs::write(&p, b"").map_err(|e| Error::io("write", &p, e))?;
        }
        Some(Portable::Dir { dir }) => {
            let p = fresh.join(dir);
            std::fs::create_dir_all(&p).map_err(|e| Error::io("create", &p, e))?;
        }
        None => {}
    }
    store.place(&entry.id, &fresh)?;
    progress.on(Event::Placed);
    Ok(Installed {
        emulator: entry.id.clone(),
        channel: resolved.channel.clone(),
        exe: Exe::Path(store.app_dir(&entry.id).join(exe_rel)),
        version: resolved.version.clone(),
        release: Some(resolved.release.clone()),
        digest: Some(actual),
        installed_at: now_rfc3339(),
    })
}

/// Removes a managed install; with `purge`, its data too.
pub fn remove(
    entry: &Entry,
    row: &Installed,
    store: &Store,
    runner: &dyn Runner,
    purge: bool,
) -> Result<()> {
    if let Exe::FlatpakRun(id) = &row.exe {
        flatpak::remove(runner, id)?;
    } else {
        store.remove_files(&entry.id, purge)?;
    }
    store.forget(&entry.id)
}

/// `true` when the channel points somewhere else than what is installed.
pub fn update_available(row: &Installed, resolved: &Resolved) -> bool {
    row.channel != "flatpak" && row.release.as_deref() != Some(resolved.release.as_str())
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::channel::flatpak::fake::FakeRunner;
    use crate::channel::http::fake::FakeHttp;
    use crate::progress::Quiet;
    use serde_json::json;
    use std::io::Write;

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
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

    fn sha256(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn fixture_zip() -> Vec<u8> {
        zip_with(&[
            ("duckstation-qt-x64-ReleaseLTCG.exe", b"exe"),
            ("data/x", b"1"),
        ])
    }

    fn github_fixture(digest: Option<String>) -> FakeHttp {
        let zip = fixture_zip();
        let mut http = FakeHttp::default();
        http.json.insert(
            "https://api.github.com/repos/stenzek/duckstation/releases/latest".into(),
            json!({ "tag_name": "latest", "published_at": "2026-09-20T00:00:00Z", "assets": [
                { "name": "duckstation-windows-x64-release.zip", "size": zip.len(),
                  "browser_download_url": "https://dl/duckstation-windows-x64-release.zip",
                  "digest": digest.map(|d| format!("sha256:{d}")) },
                { "name": "duckstation-windows-x64-installer.exe", "size": 1, "browser_download_url": "https://dl/i.exe" }
            ]}),
        );
        http.files
            .insert("https://dl/duckstation-windows-x64-release.zip".into(), zip);
        http
    }

    #[test]
    fn archive_install_places_exe_marker_and_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = Catalog::embedded().unwrap();
        let entry = c.get("duckstation").unwrap();
        let http = github_fixture(Some(sha256(&fixture_zip())));
        let row = install(
            entry,
            Os::Windows,
            &store,
            &http,
            &FakeRunner::default(),
            &Quiet,
        )
        .unwrap();
        let app = store.app_dir("duckstation");
        assert!(app.join("duckstation-qt-x64-ReleaseLTCG.exe").is_file());
        assert!(app.join("portable.txt").is_file(), "portable marker");
        assert_eq!(
            row.exe,
            Exe::Path(app.join("duckstation-qt-x64-ReleaseLTCG.exe"))
        );
        assert_eq!(row.release.as_deref(), Some("latest@2026-09-20"));
        assert_eq!(store.installed().unwrap().len(), 1);
        assert!(
            std::fs::read_dir(store.tmp_dir()).unwrap().next().is_none(),
            "tmp is clean"
        );

        let again = resolve(entry, Os::Windows, &http).unwrap();
        assert!(!update_available(&row, &again));
        remove(entry, &row, &store, &FakeRunner::default(), false).unwrap();
        assert!(!app.exists());
        assert!(store.installed().unwrap().is_empty());
    }

    #[test]
    fn a_wrong_digest_is_refused_and_the_part_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = Catalog::embedded().unwrap();
        let entry = c.get("duckstation").unwrap();
        let http = github_fixture(Some("00".repeat(32)));
        let err = install(
            entry,
            Os::Windows,
            &store,
            &http,
            &FakeRunner::default(),
            &Quiet,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Verify { .. }));
        assert!(std::fs::read_dir(store.tmp_dir()).unwrap().next().is_none());
        assert!(store.installed().unwrap().is_empty());
    }

    #[test]
    fn flatpak_install_records_the_app_id() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = Catalog::embedded().unwrap();
        let entry = c.get("pcsx2").unwrap();
        let runner = FakeRunner {
            info_version: Some("2.8.2".into()),
            ..Default::default()
        };
        let row = install(
            entry,
            Os::Linux,
            &store,
            &FakeHttp::default(),
            &runner,
            &Quiet,
        )
        .unwrap();
        assert_eq!(row.exe, Exe::FlatpakRun("net.pcsx2.PCSX2".into()));
        assert_eq!(row.version.as_deref(), Some("2.8.2"));
    }

    #[test]
    fn policy_and_os_refusals() {
        let c = Catalog::embedded().unwrap();
        let http = FakeHttp::default();
        assert!(matches!(
            resolve(c.get("ryujinx").unwrap(), Os::Linux, &http),
            Err(Error::Policy { .. })
        ));
        assert!(matches!(
            resolve(c.get("xenia-canary").unwrap(), Os::Linux, &http),
            Err(Error::UnsupportedOs { .. })
        ));
        assert!(matches!(
            resolve(c.get("pcsx2").unwrap(), Os::Macos, &http),
            Err(Error::UnsupportedOs { .. })
        ));
    }
}
