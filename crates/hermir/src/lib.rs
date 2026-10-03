//! hermir — one interface for managing emulators.
//!
//! ```no_run
//! use hermir::{Hermir, Options, Patch, Region, Video, progress::Quiet};
//! # fn main() -> hermir::Result<()> {
//! let h = Hermir::open(Options::default())?;
//! let pcsx2 = h.emulator("pcsx2")?;
//! let row = pcsx2.install(&Quiet)?;
//! println!("{}", row.exe);
//! let copy = pcsx2.best()?.expect("installed above");
//! let done = pcsx2.apply(&copy, &Patch {
//!     video: Some(Video { fullscreen: Some(true), scale: Some(3), ..Default::default() }),
//!     region: Some(Region::Europe),
//!     ..Default::default()
//! })?;
//! for k in &done.knobs {
//!     println!("{} {:?} {}", k.knob, k.support, k.note.as_deref().unwrap_or(""));
//! }
//! pcsx2.revert(false)?;
//! # Ok(())
//! # }
//! ```
//!
//! `Hermir` holds the catalog, the prefix and the machine; an `EmulatorHandle` is one entry
//! of the catalog on this machine: install it, prepare it, `apply` a session's players and
//! settings to it and `revert` them. Every type is serde and JSON Schema, so the CLI's
//! `--json` is the same contract as the library.
#![warn(missing_docs)]

mod catalog;
mod channel;
mod config;
mod detect;
mod error;
mod model;
mod players;
mod prepare;
pub mod progress;
mod store;

use std::path::PathBuf;

pub use catalog::Catalog;
pub use channel::flatpak::{Process, Runner};
pub use channel::http::{Http, Ureq};
pub use detect::{Env, RealEnv};
pub use error::{Error, Result};
pub use model::*;
use progress::Progress;
pub use store::{Lock, Placed, Store};

/// How to open hermir. Every field has a default that fits the machine it runs on.
pub struct Options {
    /// The managed prefix; `Store::default_root()` when unset.
    pub prefix: Option<PathBuf>,
    /// The OS to resolve for; the running one when unset. Another OS is useful only to resolve
    /// or dry-run — an install for another OS makes files nothing here can run.
    pub os: Option<Os>,
    /// How hermir fetches release metadata and downloads: [`Ureq`] by default.
    pub http: Box<dyn Http>,
    /// How hermir runs `flatpak`: [`Process`] by default.
    pub runner: Box<dyn Runner>,
    /// What detection looks at (files, `PATH`, environment variables): the machine itself,
    /// [`RealEnv`], when unset.
    pub env: Option<Box<dyn Env>>,
    /// Refuse a download there is nothing to check against (a GitHub asset without a digest,
    /// a libretro core) with [`Error::Unverified`], before anything is fetched. Off by default:
    /// such an install then records `verified: none`.
    pub require_verified: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            prefix: None,
            os: None,
            http: Box::new(Ureq::new()),
            runner: Box::new(Process),
            env: None,
            require_verified: false,
        }
    }
}

/// The catalog, the prefix and the machine: everything hermir does starts here. It is `Send`
/// and `Sync`, so a host can share one behind an `Arc` and call it from `spawn_blocking`.
pub struct Hermir {
    catalog: Catalog,
    store: Store,
    os: Os,
    http: Box<dyn Http>,
    runner: Box<dyn Runner>,
    env: Box<dyn Env>,
    require_verified: bool,
}

impl Hermir {
    /// hermir for this machine, with the embedded catalog. Nothing is written until something
    /// is installed or applied.
    pub fn open(opts: Options) -> Result<Hermir> {
        let os = opts.os.unwrap_or_else(Os::current);
        Ok(Hermir {
            catalog: Catalog::embedded()?,
            store: Store::open(opts.prefix.unwrap_or_else(Store::default_root))?,
            os,
            http: opts.http,
            runner: opts.runner,
            env: opts.env.unwrap_or_else(|| Box::new(RealEnv::new(os))),
            require_verified: opts.require_verified,
        })
    }

    /// What hermir knows.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The prefix: managed installs, snapshots, the lock.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The OS hermir resolves for.
    pub fn os(&self) -> Os {
        self.os
    }

    /// Where `apply` keeps what it overwrote.
    fn snapshots(&self) -> PathBuf {
        self.store.root().join(".snapshots")
    }

    /// Restores every emulator with a session's changes outstanding: what a consumer calls
    /// when the game it prepared has exited. A file changed since hermir wrote it is left as it
    /// is (a [`StepOutcome::Conflict`]) unless `force`. Holds the prefix lock while it writes.
    pub fn revert_all(&self, force: bool) -> Result<Vec<(String, Vec<PrepareStep>)>> {
        let _lock = self.store.lock()?;
        Ok(config::outstanding(&self.snapshots())
            .into_iter()
            .map(|id| {
                let steps = config::revert(&self.snapshots(), &id, force);
                (id, steps)
            })
            .collect())
    }

    /// The knob × emulator matrix: what `apply` can do for each catalog entry.
    pub fn support(&self) -> Vec<(String, Vec<KnobChange>)> {
        self.catalog
            .entries()
            .iter()
            .map(|e| (e.id.clone(), config::support(e)))
            .collect()
    }

    /// One catalog entry on this machine, by id.
    pub fn emulator(&self, id: &str) -> Result<EmulatorHandle<'_>> {
        let entry = self
            .catalog
            .get(id)
            .ok_or_else(|| Error::NotInCatalog(id.into()))?;
        Ok(EmulatorHandle { h: self, entry })
    }

    /// Copies the user installed, fresh from the machine.
    pub fn detect(&self) -> Vec<Install> {
        detect::detect(&self.catalog, self.env.as_ref())
    }

    /// Managed rows as `Install`s plus detected copies, managed first, one row per exe.
    pub fn installs(&self) -> Result<Vec<Install>> {
        let mut out: Vec<Install> = self
            .store
            .installed()?
            .into_iter()
            .map(|r| self.managed_install(&r))
            .collect();
        for d in self.detect() {
            if !out.iter().any(|i| i.exe == d.exe) {
                out.push(d);
            }
        }
        Ok(out)
    }

    fn managed_install(&self, r: &Installed) -> Install {
        let entry = self.catalog.get(&r.emulator);
        let config_root = match (&r.exe, entry) {
            (Exe::FlatpakRun(_), Some(e)) => e
                .roots
                .flatpak
                .as_deref()
                .map(|t| detect::expand(t, self.env.as_ref(), None)),
            // A Linux build without a portable marker (an AppImage) keeps the native layout.
            (Exe::Path(_), Some(e)) if self.os == Os::Linux && !marks_portable(e, self.os) => e
                .roots
                .native
                .as_deref()
                .map(|t| detect::expand(t, self.env.as_ref(), None)),
            (Exe::Path(p), Some(e)) => e
                .roots
                .portable
                .as_deref()
                .map(|t| detect::expand(t, self.env.as_ref(), p.parent())),
            _ => None,
        };
        Install {
            emulator: r.emulator.clone(),
            kind: if matches!(r.exe, Exe::FlatpakRun(_)) {
                InstallKind::Flatpak
            } else {
                InstallKind::Managed
            },
            exe: r.exe.clone(),
            version: r.version.clone(),
            config_root,
        }
    }

    /// One `Status` per catalog entry (or the one asked for). `check_updates` resolves every
    /// managed archive install against its channel, which costs one request each.
    pub fn status(&self, only: Option<&str>, check_updates: bool) -> Result<Vec<Status>> {
        let managed = self.store.installed()?;
        let detected = self.detect();
        let mut out = Vec::new();
        for e in self.catalog.entries() {
            if only.is_some_and(|id| id != e.id) {
                continue;
            }
            let row = managed.iter().find(|r| r.emulator == e.id).cloned();
            let (update, update_error) = match (&row, check_updates) {
                (Some(r), true) if r.channel != "flatpak" => {
                    match channel::resolve(e, self.os, self.http.as_ref()) {
                        Ok(res) => (
                            channel::update_available(r, &res).then_some(res.release),
                            None,
                        ),
                        Err(err) => (None, Some(err.to_string())),
                    }
                }
                _ => (None, None),
            };
            out.push(Status {
                emulator: e.id.clone(),
                name: e.name.clone(),
                offered: e.channels.get(self.os).is_some(),
                managed: row,
                detected: detected
                    .iter()
                    .filter(|d| d.emulator == e.id)
                    .cloned()
                    .collect(),
                update,
                update_error,
            });
        }
        if only.is_some() && out.is_empty() {
            return Err(Error::NotInCatalog(only.unwrap_or_default().into()));
        }
        Ok(out)
    }

    /// Installs a libretro core into the RetroArch this machine has (managed first). The
    /// buildbot publishes no checksums, so a core is never verified, and with
    /// `Options::require_verified` this refuses.
    pub fn install_core(&self, core: &str, progress: &dyn Progress) -> Result<PathBuf> {
        if self.require_verified {
            return Err(Error::Unverified {
                what: format!("core {core}"),
                why: channel::libretro::UNVERIFIED.into(),
            });
        }
        let ra = self
            .installs()?
            .into_iter()
            .find(|i| i.emulator == "retroarch")
            .ok_or_else(|| Error::Place {
                what: format!("core {core}"),
                why: "no RetroArch on this machine; install it first".into(),
            })?;
        let cores = ra
            .config_root
            .ok_or_else(|| Error::Place {
                what: format!("core {core}"),
                why: "RetroArch's config root is unknown".into(),
            })?
            .join("cores");
        let _lock = self.store.lock()?;
        channel::libretro::install_core(
            self.http.as_ref(),
            self.os,
            core,
            &cores,
            &self.store.tmp_dir(),
            progress,
        )
    }
}

/// Whether the channel for `os` makes the emulator keep its files beside the exe.
fn marks_portable(entry: &Entry, os: Os) -> bool {
    matches!(
        entry.channels.get(os),
        Some(Channel::Github {
            portable: Some(_),
            ..
        }) | Some(Channel::Url {
            portable: Some(_),
            ..
        })
    )
}

/// One catalog entry on this machine.
pub struct EmulatorHandle<'a> {
    h: &'a Hermir,
    entry: &'a Entry,
}

impl EmulatorHandle<'_> {
    /// The catalog entry.
    pub fn entry(&self) -> &Entry {
        self.entry
    }

    /// Where the channel points now; nothing is downloaded.
    pub fn resolve(&self) -> Result<Resolved> {
        channel::resolve(self.entry, self.h.os, self.h.http.as_ref())
    }

    /// The copy hermir installed, if any.
    pub fn managed(&self) -> Result<Option<Installed>> {
        self.h.store.installed_one(&self.entry.id)
    }

    /// Installs, or reinstalls, from the channel for this OS.
    pub fn install(&self, progress: &dyn Progress) -> Result<Installed> {
        let _lock = self.h.store.lock()?;
        self.install_locked(progress)
    }

    fn install_locked(&self, progress: &dyn Progress) -> Result<Installed> {
        channel::install(
            self.entry,
            self.h.os,
            &self.h.store,
            self.h.http.as_ref(),
            self.h.runner.as_ref(),
            progress,
            self.h.require_verified,
        )
    }

    /// The managed row, or why there is none. Read it under the lock, so no other writer
    /// changes it meanwhile.
    fn managed_row(&self) -> Result<Installed> {
        self.managed()?.ok_or_else(|| Error::Place {
            what: self.entry.id.clone(),
            why: "not installed by hermir".into(),
        })
    }

    /// Reinstalls when the channel moved; `Ok(None)` when it did not. A Flatpak asks
    /// `flatpak update`.
    pub fn update(&self, progress: &dyn Progress) -> Result<Option<Installed>> {
        let _lock = self.h.store.lock()?;
        let row = self.managed_row()?;
        if let Exe::FlatpakRun(id) = &row.exe {
            channel::flatpak::update(self.h.runner.as_ref(), id)?;
            let version = channel::flatpak::version(self.h.runner.as_ref(), id);
            // Flatpak updates in place and says nothing useful about it; the version it reports
            // afterwards is what tells whether anything moved.
            if version == row.version {
                return Ok(None);
            }
            let fresh = Installed {
                version,
                verified: Verified::Flatpak,
                installed_at: store::now_rfc3339(),
                ..row
            };
            self.h.store.record(fresh.clone())?;
            return Ok(Some(fresh));
        }
        let resolved = self.resolve()?;
        if !channel::update_available(&row, &resolved) {
            return Ok(None);
        }
        self.install_locked(progress).map(Some)
    }

    /// Removes the managed copy; with `purge`, its data too (a Flatpak's `~/.var/app/<id>`).
    pub fn remove(&self, purge: bool) -> Result<()> {
        let _lock = self.h.store.lock()?;
        let row = self.managed_row()?;
        channel::remove(
            self.entry,
            &row,
            &self.h.store,
            self.h.runner.as_ref(),
            purge,
        )
    }

    /// Where this copy reads loose firmware files, when the catalog knows.
    pub fn firmware_dir(&self, install: &Install) -> Option<PathBuf> {
        let dir = self.entry.firmware.as_ref()?.dir.as_ref()?;
        let root = install.config_root.as_ref()?;
        Some(if dir == "." {
            root.clone()
        } else {
            root.join(dir)
        })
    }

    /// Answers `install`'s first-run questions (the catalog's `first_run`: a setup wizard's
    /// answers, a welcome box), then puts `firmware` for `platform` in place: copied into the
    /// emulator's firmware folder, handed to its own installer (RPCS3's PUP), or unpacked from
    /// an archive (Eden's system update). `firmware` is what the consumer's source holds for
    /// the platform, companions included. Idempotent; every step is in the result, and a
    /// platform still without its firmware says so. Holds the prefix lock while it writes.
    pub fn prepare(
        &self,
        install: &Install,
        platform: Option<&str>,
        firmware: &[PathBuf],
    ) -> Result<Prepared> {
        let _lock = self.h.store.lock()?;
        Ok(prepare::prepare(
            self.entry,
            self.h.os,
            install,
            platform,
            firmware,
            self.h.runner.as_ref(),
        ))
    }

    /// Writes `patch` into this copy: players into its bindings, video and region into its
    /// settings, native keys as given. Every file is snapshotted before its first edit, so
    /// [`Self::revert`] gives the player's own settings back byte for byte. The result says
    /// per knob whether the emulator took it, and why not when it did not. A patch that
    /// cannot be written as asked ([`Patch::validate`]) is an error before anything is; the
    /// prefix lock is held while it writes.
    pub fn apply(&self, install: &Install, patch: &Patch) -> Result<Applied> {
        patch.validate().map_err(Error::Invalid)?;
        let _lock = self.h.store.lock()?;
        Ok(config::apply(
            self.entry,
            self.h.os,
            install,
            patch,
            &self.h.snapshots(),
        ))
    }

    /// Restores every file [`Self::apply`] changed, for every copy of this emulator, and
    /// forgets what it restored. A file changed since hermir wrote it is left as it is, its
    /// snapshot kept (a [`StepOutcome::Conflict`]), unless `force`. The prefix lock is held
    /// while it writes.
    pub fn revert(&self, force: bool) -> Result<Vec<PrepareStep>> {
        let _lock = self.h.store.lock()?;
        Ok(config::revert(&self.h.snapshots(), &self.entry.id, force))
    }

    /// What this copy's files hold for each knob, in the neutral spelling `apply` takes: a
    /// knob the file does not set reads as unset, with a note.
    pub fn get(&self, install: &Install) -> Result<Vec<KnobValue>> {
        config::get(self.entry, self.h.os, install).map_err(Error::Invalid)
    }

    /// What `key` in `section` of the catalog's `file` holds in this copy, as the file spells
    /// it: the read side of [`Patch::native`].
    pub fn get_native(
        &self,
        install: &Install,
        file: &str,
        section: &str,
        key: &str,
    ) -> Result<Option<String>> {
        config::get_native(self.entry, self.h.os, install, file, section, key)
            .map_err(Error::Invalid)
    }

    /// What [`Self::apply`] can do for this emulator, knob by knob, before asking.
    pub fn support(&self) -> Vec<KnobChange> {
        config::support(self.entry)
    }

    /// Every copy of this emulator on the machine, managed first.
    pub fn copies(&self) -> Result<Vec<Install>> {
        Ok(self
            .h
            .installs()?
            .into_iter()
            .filter(|i| i.emulator == self.entry.id)
            .collect())
    }

    /// The best copy on this machine: managed, else the first detected.
    pub fn best(&self) -> Result<Option<Install>> {
        Ok(self
            .h
            .installs()?
            .into_iter()
            .find(|i| i.emulator == self.entry.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::flatpak::fake::FakeRunner;
    use crate::channel::http::fake::FakeHttp;
    use crate::progress::Quiet;

    fn open(prefix: &std::path::Path, require_verified: bool) -> Hermir {
        Hermir::open(Options {
            prefix: Some(prefix.to_path_buf()),
            os: Some(Os::Windows),
            http: Box::new(FakeHttp::default()),
            runner: Box::new(FakeRunner::default()),
            env: None,
            require_verified,
        })
        .unwrap()
    }

    /// The row is read under the lock: while another writer holds the prefix, the answer is
    /// "busy", never a stale "not installed".
    #[test]
    fn update_and_remove_take_the_lock_first() {
        let dir = tempfile::tempdir().unwrap();
        let h = open(dir.path(), false);
        let e = h.emulator("duckstation").unwrap();
        let held = h.store().lock().unwrap();
        assert!(matches!(e.update(&Quiet), Err(Error::Locked(_))));
        assert!(matches!(e.remove(false), Err(Error::Locked(_))));
        drop(held);
        assert!(matches!(e.remove(false), Err(Error::Place { .. })));
    }

    #[test]
    fn require_verified_refuses_a_core() {
        let dir = tempfile::tempdir().unwrap();
        let err = open(dir.path(), true)
            .install_core("snes9x", &Quiet)
            .unwrap_err();
        assert!(matches!(err, Error::Unverified { .. }));
        assert_eq!(err.exit_code(), 4);
    }

    fn pcsx2_copy(root: &std::path::Path) -> Install {
        Install {
            emulator: "pcsx2".into(),
            kind: InstallKind::Flatpak,
            exe: Exe::FlatpakRun("net.pcsx2.PCSX2".into()),
            version: None,
            config_root: Some(root.to_path_buf()),
        }
    }

    #[test]
    fn a_patch_that_cannot_be_written_as_asked_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let h = open(&tmp.path().join("prefix"), false);
        let e = h.emulator("pcsx2").unwrap();
        let root = tmp.path().join("PCSX2");
        let seat = |seat: u8, name: &str| Player {
            seat,
            pad: PadRef {
                name: name.into(),
                ..PadRef::xbox360(0)
            },
        };
        for players in [
            vec![seat(0, "pad")],
            vec![seat(1, "pad"), seat(1, "pad")],
            vec![seat(1, "pad\n[GCPad2]\nDevice = evil")],
        ] {
            let patch = Patch {
                players: Some(players),
                ..Default::default()
            };
            let err = e.apply(&pcsx2_copy(&root), &patch).unwrap_err();
            assert!(matches!(err, Error::Invalid(_)), "{err}");
            assert_eq!(err.exit_code(), 2);
        }
        let patch = Patch {
            native: vec![Native {
                file: "main".into(),
                section: "UI".into(),
                key: "StartFullscreen".into(),
                value: "true\nevil = 1".into(),
            }],
            ..Default::default()
        };
        assert!(matches!(
            e.apply(&pcsx2_copy(&root), &patch),
            Err(Error::Invalid(_))
        ));
        assert!(!root.exists(), "nothing was written");
    }

    #[test]
    fn writes_wait_for_the_prefix_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let h = open(&tmp.path().join("prefix"), false);
        let e = h.emulator("pcsx2").unwrap();
        let held = h.store().lock().unwrap();
        let patch = Patch {
            region: Some(Region::Europe),
            ..Default::default()
        };
        let root = tmp.path().join("PCSX2");
        assert!(matches!(
            e.apply(&pcsx2_copy(&root), &patch),
            Err(Error::Locked(_))
        ));
        assert!(matches!(e.revert(false), Err(Error::Locked(_))));
        assert!(matches!(h.revert_all(false), Err(Error::Locked(_))));
        assert!(matches!(
            e.prepare(&pcsx2_copy(&root), None, &[]),
            Err(Error::Locked(_))
        ));
        drop(held);
        assert!(e.apply(&pcsx2_copy(&root), &patch).is_ok());
    }

    #[test]
    fn a_knob_whose_file_cannot_be_written_says_it_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let h = open(&tmp.path().join("prefix"), false);
        let e = h.emulator("pcsx2").unwrap();
        let root = tmp.path().join("PCSX2");
        // A directory where the settings file should be: it can be neither read nor replaced.
        std::fs::create_dir_all(root.join("inis/PCSX2.ini")).unwrap();
        let patch = Patch {
            video: Some(Video {
                fullscreen: Some(true),
                ..Default::default()
            }),
            region: Some(Region::Europe),
            ..Default::default()
        };
        let done = e.apply(&pcsx2_copy(&root), &patch).unwrap();
        assert!(done.failed());
        let full = done
            .knobs
            .iter()
            .find(|k| k.knob == "video.fullscreen")
            .unwrap();
        assert_eq!(full.support, Support::Failed);
        assert!(full.note.as_deref().unwrap().contains("PCSX2.ini"));
        // Region was never going to be written, so it stays unsupported.
        let region = done.knobs.iter().find(|k| k.knob == "region").unwrap();
        assert_eq!(region.support, Support::Unsupported);
    }
}
