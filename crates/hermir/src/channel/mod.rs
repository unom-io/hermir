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
use crate::model::{Channel, Entry, Exe, Installed, Os, Portable, Resolved, Verified};
use crate::progress::{Event, Progress};
use crate::store::{Store, now_rfc3339, sha256_file};
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

/// Why a resolved archive has nothing to be checked against.
fn unverified_why(resolved: &Resolved) -> String {
    match resolved.channel.as_str() {
        "url" => "the catalog pins no sha256 for it".into(),
        _ => format!(
            "GitHub publishes no digest for {}",
            resolved.file_name.as_deref().unwrap_or("the asset")
        ),
    }
}

/// Installs (or reinstalls) `entry` into `store`. The caller holds the store lock. With
/// `require_verified`, an archive there is nothing to check against is refused before it is
/// downloaded.
pub fn install(
    entry: &Entry,
    os: Os,
    store: &Store,
    http: &dyn Http,
    runner: &dyn Runner,
    progress: &dyn Progress,
    require_verified: bool,
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
                sha256: None,
                verified: Verified::Flatpak,
                kept: Vec::new(),
                installed_at: now_rfc3339(),
            }
        }
        Channel::Github { exe, portable, .. } | Channel::Url { exe, portable, .. } => {
            if require_verified && resolved.digest.is_none() {
                return Err(Error::Unverified {
                    what: entry.id.clone(),
                    why: unverified_why(&resolved),
                });
            }
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

/// `<file>.<id>.part`, where the id stands for the release and its size: a part never resumes
/// into another build, even one rebuilt under the same name.
fn part_name(file_name: &str, resolved: &Resolved) -> String {
    use sha2::{Digest, Sha256};
    let size = resolved.size.map(|s| s.to_string()).unwrap_or_default();
    let id = Sha256::digest(format!("{}\n{size}", resolved.release).as_bytes());
    let id: String = id[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{file_name}.{id}.part")
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
    for rel in std::iter::once(exe_rel).chain(portable.map(|p| match p {
        Portable::File { file } => file.as_str(),
        Portable::Dir { dir } => dir.as_str(),
    })) {
        if !extract::is_safe_relative(Path::new(rel)) {
            return Err(Error::Catalog {
                entry: entry.id.clone(),
                why: format!("{rel} is not a relative path"),
            });
        }
    }
    let file_name = resolved
        .file_name
        .clone()
        .unwrap_or_else(|| format!("{}.archive", entry.id));
    let tmp = store.tmp_dir();
    std::fs::create_dir_all(&tmp).map_err(|e| Error::io("create", &tmp, e))?;
    let part = tmp.join(part_name(&file_name, resolved));
    http.download(url, &part, resolved.size, progress)?;

    if resolved.digest.is_some() {
        progress.on(Event::Verifying);
    }
    let sha256 = sha256_file(&part)?;
    let verified = match &resolved.digest {
        Some(expected) if !expected.eq_ignore_ascii_case(&sha256) => {
            let _ = std::fs::remove_file(&part);
            return Err(Error::Verify {
                what: file_name,
                expected: expected.clone(),
                actual: sha256,
            });
        }
        Some(_) if resolved.channel == "url" => Verified::Pinned,
        Some(_) => Verified::Published,
        None => {
            progress.on(Event::NotVerified {
                why: unverified_why(resolved),
            });
            Verified::None
        }
    };
    let archive = tmp.join(&file_name);
    std::fs::rename(&part, &archive).map_err(|e| Error::io("rename", &archive, e))?;

    progress.on(Event::Extracting);
    let fresh = tmp.join(format!("{}.extract", entry.id));
    let _ = std::fs::remove_dir_all(&fresh);
    let extracted = extract::extract(&archive, &fresh);
    let _ = std::fs::remove_file(&archive);
    extracted?;
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
    if let Some(Portable::File { file }) = portable {
        let p = fresh.join(file);
        std::fs::write(&p, b"").map_err(|e| Error::io("write", &p, e))?;
    }
    let placed = store.place(&entry.id, &fresh)?;
    // A marker folder and the firmware folder exist from the start, so the copy runs portable
    // and a consumer can be granted the folder before the emulator ever ran. After placing:
    // the store moves files, not empty folders.
    let app = store.app_dir(&entry.id);
    let marker = match portable {
        Some(Portable::Dir { dir }) => Some(app.join(dir)),
        _ => None,
    };
    let fw_dir = entry.firmware.as_ref().and_then(|f| f.dir.as_ref());
    let firmware = match (fw_dir, portable_root(entry, &app)) {
        (Some(dir), Some(root)) => Some(root.join(dir)),
        _ => None,
    };
    for p in marker.iter().chain(firmware.iter()) {
        std::fs::create_dir_all(p).map_err(|e| Error::io("create", p, e))?;
    }
    progress.on(Event::Placed);
    Ok(Installed {
        emulator: entry.id.clone(),
        channel: resolved.channel.clone(),
        exe: Exe::Path(app.join(exe_rel)),
        version: resolved.version.clone(),
        release: Some(resolved.release.clone()),
        sha256: Some(sha256),
        verified,
        kept: placed.kept_modified,
        installed_at: now_rfc3339(),
    })
}

/// Removes a managed install; with `purge`, its data too (for a Flatpak, `~/.var/app/<id>`).
pub fn remove(
    entry: &Entry,
    row: &Installed,
    store: &Store,
    runner: &dyn Runner,
    purge: bool,
) -> Result<()> {
    if let Exe::FlatpakRun(id) = &row.exe {
        flatpak::remove(runner, id, purge)?;
    } else {
        store.remove_files(&entry.id, purge)?;
    }
    store.forget(&entry.id)
}

/// `true` when the channel points somewhere else than what is installed: for GitHub, another
/// asset, or the same one uploaded again.
pub fn update_available(row: &Installed, resolved: &Resolved) -> bool {
    row.channel != "flatpak" && row.release.as_deref() != Some(resolved.release.as_str())
}

/// `roots.portable` with `<app>` resolved to the extracted tree, when the entry has one.
fn portable_root(entry: &Entry, app: &Path) -> Option<std::path::PathBuf> {
    let template = entry.roots.portable.as_deref()?;
    let rest = template.strip_prefix("<app>")?;
    let rest = rest.trim_start_matches(['/', '\\']);
    Some(if rest.is_empty() {
        app.to_path_buf()
    } else {
        app.join(rest)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::channel::flatpak::fake::FakeRunner;
    use crate::channel::http::fake::FakeHttp;
    use crate::progress::Quiet;
    use serde_json::json;
    use std::cell::RefCell;
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

    /// The latest release of `repo` with one matching asset, `zip`, at `asset_url`.
    fn github_release(
        repo: &str,
        asset: &str,
        zip: Vec<u8>,
        digest: Option<String>,
        updated_at: &str,
    ) -> FakeHttp {
        let mut http = FakeHttp::default();
        let url = format!("https://dl/{asset}");
        http.json.insert(
            format!("https://api.github.com/repos/{repo}/releases/latest"),
            json!({ "tag_name": "latest", "published_at": "2026-09-20T00:00:00Z", "assets": [
                { "name": asset, "size": zip.len(), "id": 41, "updated_at": updated_at,
                  "browser_download_url": url,
                  "digest": digest.map(|d| format!("sha256:{d}")) },
                { "name": "duckstation-windows-x64-installer.exe", "size": 1, "browser_download_url": "https://dl/i.exe" }
            ]}),
        );
        http.files.insert(url, zip);
        http
    }

    fn github_fixture(digest: Option<String>) -> FakeHttp {
        github_release(
            "stenzek/duckstation",
            "duckstation-windows-x64-release.zip",
            fixture_zip(),
            digest,
            "2026-09-20T00:01:00Z",
        )
    }

    /// Every event an install reported.
    #[derive(Default)]
    struct Log(RefCell<Vec<Event>>);

    impl Progress for Log {
        fn on(&self, event: Event) {
            self.0.borrow_mut().push(event);
        }
    }

    fn install_windows(id: &str, store: &Store, http: &FakeHttp) -> Result<Installed> {
        let c = Catalog::embedded().unwrap();
        install(
            c.get(id).unwrap(),
            Os::Windows,
            store,
            http,
            &FakeRunner::default(),
            &Quiet,
            false,
        )
    }

    #[test]
    fn archive_install_places_exe_marker_and_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = Catalog::embedded().unwrap();
        let entry = c.get("duckstation").unwrap();
        let http = github_fixture(Some(sha256(&fixture_zip())));
        let log = Log::default();
        let row = install(
            entry,
            Os::Windows,
            &store,
            &http,
            &FakeRunner::default(),
            &log,
            true,
        )
        .unwrap();
        let app = store.app_dir("duckstation");
        assert!(app.join("duckstation-qt-x64-ReleaseLTCG.exe").is_file());
        assert!(app.join("portable.txt").is_file(), "portable marker");
        assert!(
            app.join("bios").is_dir(),
            "firmware folder from the catalog"
        );
        assert_eq!(
            row.exe,
            Exe::Path(app.join("duckstation-qt-x64-ReleaseLTCG.exe"))
        );
        let digest = sha256(&fixture_zip());
        assert_eq!(
            row.release.as_deref(),
            Some(format!("latest@2026-09-20T00:01:00Z#41:{}", &digest[..16]).as_str())
        );
        assert_eq!(row.sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(row.verified, Verified::Published);
        assert!(log.0.borrow().contains(&Event::Verifying));
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
        let http = github_fixture(Some("00".repeat(32)));
        let err = install_windows("duckstation", &store, &http).unwrap_err();
        assert!(matches!(err, Error::Verify { .. }));
        assert!(std::fs::read_dir(store.tmp_dir()).unwrap().next().is_none());
        assert!(store.installed().unwrap().is_empty());
    }

    #[test]
    fn an_asset_without_a_digest_installs_as_not_verified() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = Catalog::embedded().unwrap();
        let entry = c.get("duckstation").unwrap();
        let http = github_fixture(None);
        let log = Log::default();
        let row = install(
            entry,
            Os::Windows,
            &store,
            &http,
            &FakeRunner::default(),
            &log,
            false,
        )
        .unwrap();
        assert_eq!(row.verified, Verified::None);
        assert_eq!(row.sha256.as_deref(), Some(sha256(&fixture_zip()).as_str()));
        let events = log.0.borrow();
        assert!(!events.contains(&Event::Verifying), "nothing was checked");
        assert!(events.iter().any(|e| matches!(
            e,
            Event::NotVerified { why } if why.contains("duckstation-windows-x64-release.zip")
        )));
    }

    #[test]
    fn require_verified_refuses_before_anything_is_fetched() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = Catalog::embedded().unwrap();
        let http = github_fixture(None);
        let err = install(
            c.get("duckstation").unwrap(),
            Os::Windows,
            &store,
            &http,
            &FakeRunner::default(),
            &Quiet,
            true,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Unverified { .. }), "{err}");
        assert_eq!(err.exit_code(), 4);
        assert!(!store.tmp_dir().exists(), "nothing downloaded");
        assert!(!store.app_dir("duckstation").exists(), "nothing placed");
        assert!(store.installed().unwrap().is_empty());
    }

    #[test]
    fn a_url_channel_checks_its_pinned_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = Catalog::embedded().unwrap();
        let mut entry = c.get("scummvm").unwrap().clone();
        let zip = zip_with(&[("scummvm-2026.3.0-win32-x86_64/scummvm.exe", b"exe")]);
        let pin = |sha256: String| Channel::Url {
            url: "https://dl/scummvm.zip".into(),
            version: "2026.3.0".into(),
            sha256: Some(sha256),
            exe: "scummvm.exe".into(),
            portable: None,
        };
        let mut http = FakeHttp::default();
        http.files
            .insert("https://dl/scummvm.zip".into(), zip.clone());

        entry.channels.windows = Some(pin(sha256(&zip)));
        let fake = FakeRunner::default();
        let row = install(&entry, Os::Windows, &store, &http, &fake, &Quiet, true).unwrap();
        assert_eq!(row.verified, Verified::Pinned);
        assert_eq!(row.release.as_deref(), Some("2026.3.0"));

        entry.channels.windows = Some(pin("11".repeat(32)));
        let err = install(&entry, Os::Windows, &store, &http, &fake, &Quiet, false).unwrap_err();
        assert!(matches!(err, Error::Verify { .. }));
    }

    #[test]
    fn a_marker_folder_arrives_and_stays() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let zip = zip_with(&[
            ("Cemu_2.6/Cemu.exe", b"exe"),
            ("Cemu_2.6/resources/x", b"r"),
        ]);
        let digest = sha256(&zip);
        let http = github_release(
            "cemu-project/Cemu",
            "cemu-2.6-windows-x64.zip",
            zip,
            Some(digest),
            "2026-09-20T00:01:00Z",
        );
        install_windows("cemu", &store, &http).unwrap();
        let app = store.app_dir("cemu");
        assert!(app.join("Cemu.exe").is_file());
        assert!(app.join("portable").is_dir(), "Cemu runs portable");
        // A reinstall prunes empty folders, and the marker comes back.
        install_windows("cemu", &store, &http).unwrap();
        assert!(app.join("portable").is_dir());
    }

    #[test]
    fn an_edited_file_is_kept_and_named_in_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let http = github_fixture(Some(sha256(&fixture_zip())));
        install_windows("duckstation", &store, &http).unwrap();
        let app = store.app_dir("duckstation");
        std::fs::write(app.join("data/x"), "edited").unwrap();
        let row = install_windows("duckstation", &store, &http).unwrap();
        assert_eq!(row.kept, vec!["data/x"]);
        assert_eq!(
            std::fs::read_to_string(app.join("data/x")).unwrap(),
            "edited"
        );
        assert_eq!(
            std::fs::read_to_string(app.join("data/x.new")).unwrap(),
            "1"
        );
        assert_eq!(store.installed().unwrap()[0].kept, vec!["data/x"]);
    }

    #[test]
    fn the_same_asset_uploaded_again_is_an_update() {
        let digest = sha256(&fixture_zip());
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let http = github_fixture(Some(digest.clone()));
        let row = install_windows("duckstation", &store, &http).unwrap();
        let c = Catalog::embedded().unwrap();
        let rebuilt = github_release(
            "stenzek/duckstation",
            "duckstation-windows-x64-release.zip",
            fixture_zip(),
            Some(digest),
            "2026-09-20T17:00:00Z",
        );
        let r = resolve(c.get("duckstation").unwrap(), Os::Windows, &rebuilt).unwrap();
        assert!(update_available(&row, &r));
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
            true,
        )
        .unwrap();
        assert_eq!(row.exe, Exe::FlatpakRun("net.pcsx2.PCSX2".into()));
        assert_eq!(row.version.as_deref(), Some("2.8.2"));
        assert_eq!(row.verified, Verified::Flatpak);

        remove(entry, &row, &store, &runner, true).unwrap();
        let calls = runner.calls.borrow();
        let last = calls.last().unwrap();
        assert_eq!(last[1], "uninstall");
        assert!(last.iter().any(|a| a == "--delete-data"));
        assert!(store.installed().unwrap().is_empty());
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
            resolve(c.get("pcsx2").unwrap(), Os::Macos, &http),
            Err(Error::UnsupportedOs { .. })
        ));
    }
}
