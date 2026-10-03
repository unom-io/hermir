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
//!
//! Features: `enumerate` adds `pads::enumerate`, the pads connected now as SDL 3 sees them,
//! through the system's SDL; `enumerate-static` builds SDL from source and links it in (CMake
//! and a C compiler). Without them a consumer names its pads itself ([`PadRef`]).
#![warn(missing_docs)]

mod catalog;
mod channel;
mod config;
mod detect;
mod error;
mod launch;
mod model;
#[cfg(feature = "enumerate")]
pub mod pads;
mod players;
mod prepare;
pub mod progress;
mod saves;
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
        let env = self.env.as_ref();
        let same = |a: &Exe, b: &Exe| match (a, b) {
            (Exe::Path(a), Exe::Path(b)) => env.canonical(a) == env.canonical(b),
            (a, b) => a == b,
        };
        for d in self.detect() {
            if !out.iter().any(|i| same(&i.exe, &d.exe)) {
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
            // A build without a portable marker keeps the layout of the OS: an AppImage the
            // native one, a Windows zip `%APPDATA%`, as detection resolves them.
            (Exe::Path(_), Some(e)) if !marks_portable(e, self.os) => match self.os {
                Os::Windows => e.roots.windows.as_deref(),
                _ => e.roots.native.as_deref(),
            }
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
            .ok_or_else(|| Error::NotInstalled("retroarch".into()))?;
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
        self.mine(install)?;
        patch.validate().map_err(Error::Invalid)?;
        let _lock = self.h.store.lock()?;
        Ok(config::apply(
            self.entry,
            self.h.os,
            install,
            None,
            patch,
            &self.h.snapshots(),
        ))
    }

    /// The profiles this copy has, by name.
    pub fn profiles(&self, install: &Install) -> Result<Vec<String>> {
        self.mine(install)?;
        let mut names: Vec<String> = std::fs::read_dir(self.profiles_dir(install)?)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        Ok(names)
    }

    /// Where this copy's profiles live: a Flatpak's own data folder, which its sandbox can
    /// always read, or the prefix.
    fn profiles_dir(&self, install: &Install) -> Result<PathBuf> {
        Ok(match &install.exe {
            Exe::FlatpakRun(app) => self
                .h
                .env
                .home()
                .ok_or_else(|| Error::Invalid("no home directory to keep profiles in".into()))?
                .join(".var/app")
                .join(app)
                .join("data/hermir/profiles"),
            Exe::Path(_) => self.h.store.root().join("profiles").join(&self.entry.id),
        })
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
        self.mine(install)?;
        config::get(self.entry, self.h.os, install, None).map_err(Error::Invalid)
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
        self.mine(install)?;
        config::get_native(self.entry, self.h.os, install, file, section, key)
            .map_err(Error::Invalid)
    }

    /// Where `install` keeps the player's saves for `platform`, or for every platform the
    /// emulator runs: each folder resolved, one the player moved in the emulator's settings
    /// followed. hermir reads; it never copies or syncs saves. A platform the emulator does not
    /// run is [`Error::Invalid`].
    pub fn saves(&self, install: &Install, platform: Option<&str>) -> Result<Vec<Saves>> {
        self.mine(install)?;
        saves::saves(
            self.entry,
            self.h.os,
            install,
            self.h.env.as_ref(),
            platform,
        )
        .map_err(Error::Invalid)
    }

    /// The command that starts `install` with `req`: the catalog's template rendered, the
    /// game's folder granted to a Flatpak, launch-only knobs from the request's patch. hermir
    /// never runs it; the consumer does, its own way.
    pub fn launch(&self, install: &Install, req: &LaunchRequest) -> Result<LaunchSpec> {
        self.mine(install)?;
        let profile = match &req.profile {
            Some(name) => {
                let p = self.profile(install, name)?;
                if !p.supported() {
                    return Err(Error::Invalid(format!(
                        "{} has no profiles: a profile's settings were written in place, so \
                         launch without one",
                        self.entry.id
                    )));
                }
                if !p.exists() {
                    return Err(Error::Invalid(format!(
                        "profile {name} does not exist yet: apply a patch to it, or create it"
                    )));
                }
                Some(p.dir)
            }
            None => None,
        };
        launch::launch(self.entry, self.h.os, install, req, profile.as_deref())
            .map_err(Error::Invalid)
    }

    /// An install of another emulator would be patched with this one's keys.
    fn mine(&self, install: &Install) -> Result<()> {
        if install.emulator == self.entry.id {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "that copy is {}, not {}",
                install.emulator, self.entry.id
            )))
        }
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
        let env = self.h.env.as_ref();
        let copies: Vec<Install> = self.copies()?;
        // A managed row whose program is gone (deleted by hand) is not a copy to act on.
        Ok(copies
            .iter()
            .find(|i| match &i.exe {
                Exe::Path(p) => env.exists(p),
                Exe::FlatpakRun(_) => true,
            })
            .or(copies.first())
            .cloned())
    }
}

impl<'a> EmulatorHandle<'a> {
    /// One of this copy's profiles, by name (letters, digits, `.`, `_`, `-`): settings of its
    /// own, the player's left alone, where the emulator has a flag for that.
    pub fn profile(&self, install: &Install, name: &str) -> Result<Profile<'a>> {
        self.mine(install)?;
        if name.is_empty()
            || name.starts_with('.')
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        {
            return Err(Error::Invalid(format!(
                "{name:?}: a profile name is letters, digits, `.`, `_` and `-`"
            )));
        }
        Ok(Profile {
            h: self.h,
            entry: self.entry,
            install: install.clone(),
            name: name.into(),
            dir: self.profiles_dir(install)?.join(name),
        })
    }
}

/// One profile of one copy: a folder of settings the emulator runs on when launched with it,
/// so a session never edits the player's own. Made from the player's files the first time it is
/// applied to (or created), then hermir's: `revert` puts it back to how it was made.
pub struct Profile<'a> {
    h: &'a Hermir,
    entry: &'a Entry,
    install: Install,
    name: String,
    dir: PathBuf,
}

impl Profile<'_> {
    /// Its name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its folder.
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// Whether the emulator has a flag for profiles. Without one a profile is the in-place,
    /// snapshotted patch, and says so.
    pub fn supported(&self) -> bool {
        self.entry.profile.is_some()
    }

    /// Whether it has been made.
    pub fn exists(&self) -> bool {
        self.dir.is_dir()
    }

    /// Makes it from the player's own files (the emulator's defaults when `fresh`), keeping
    /// any file it already has. One step per file.
    pub fn create(&self, fresh: bool) -> Result<Vec<PrepareStep>> {
        let _lock = self.h.store.lock()?;
        config::seed_profile(self.entry, self.h.os, &self.install, &self.dir, fresh)
            .map_err(Error::Invalid)
    }

    /// Makes it again: what it holds goes, the player's files are copied anew.
    pub fn reset(&self, fresh: bool) -> Result<Vec<PrepareStep>> {
        self.remove()?;
        self.create(fresh)
    }

    /// Removes it, with everything the emulator kept in it.
    pub fn remove(&self) -> Result<()> {
        let _lock = self.h.store.lock()?;
        match std::fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(Error::io("remove", &self.dir, e))
            }
            _ => Ok(()),
        }
    }

    /// Writes `patch` into the profile, made first if it does not exist yet. Files the profile
    /// does not keep are patched where they are, snapshotted as ever. Where the emulator has no
    /// profiles, this is [`EmulatorHandle::apply`], and [`Applied::note`] says so.
    pub fn apply(&self, patch: &Patch) -> Result<Applied> {
        patch.validate().map_err(Error::Invalid)?;
        if !self.supported() {
            let mut done = EmulatorHandle {
                h: self.h,
                entry: self.entry,
            }
            .apply(&self.install, patch)?;
            done.note = Some(format!(
                "{} has no profiles: written in place, snapshotted; `revert` puts it back",
                self.entry.id
            ));
            return Ok(done);
        }
        if !self.exists() {
            self.create(false)?;
        }
        let _lock = self.h.store.lock()?;
        Ok(config::apply(
            self.entry,
            self.h.os,
            &self.install,
            Some(&self.dir),
            patch,
            &self.h.snapshots(),
        ))
    }

    /// What the profile holds for each knob, as [`EmulatorHandle::get`] reads a copy.
    pub fn get(&self) -> Result<Vec<KnobValue>> {
        let dir = self.supported().then_some(self.dir.as_path());
        config::get(self.entry, self.h.os, &self.install, dir).map_err(Error::Invalid)
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

    #[test]
    fn a_managed_copy_keeps_its_os_layout_and_one_whose_program_is_gone_is_not_the_best() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("prefix");
        let env = crate::detect::fake::FakeEnv::new(Os::Windows, "C:\\Users\\u")
            .var("APPDATA", "C:\\Users\\u\\AppData\\Roaming")
            .var("ProgramFiles", "C:\\Program Files")
            .file("C:\\Program Files\\xemu\\xemu.exe");
        let h = Hermir::open(Options {
            prefix: Some(prefix.clone()),
            os: Some(Os::Windows),
            http: Box::new(FakeHttp::default()),
            runner: Box::new(FakeRunner::default()),
            env: Some(Box::new(env)),
            require_verified: false,
        })
        .unwrap();
        {
            let _lock = h.store().lock().unwrap();
            h.store()
                .record(Installed {
                    emulator: "xemu".into(),
                    channel: "github".into(),
                    exe: Exe::Path(prefix.join("xemu/app/xemu.exe")),
                    version: None,
                    release: None,
                    sha256: None,
                    verified: Verified::None,
                    kept: Vec::new(),
                    installed_at: "2026-10-03T00:00:00Z".into(),
                })
                .unwrap();
        }
        let e = h.emulator("xemu").unwrap();
        let copies = e.copies().unwrap();
        assert_eq!(copies.len(), 2);
        // xemu's Windows zip has no portable marker: its settings are under %APPDATA%.
        assert_eq!(copies[0].kind, InstallKind::Managed);
        assert_eq!(
            copies[0].config_root.as_deref(),
            Some(std::path::Path::new(
                "C:\\Users\\u\\AppData\\Roaming\\xemu\\xemu"
            ))
        );
        // The managed exe was deleted by hand; the installed copy is the one to act on.
        let best = e.best().unwrap().unwrap();
        assert_eq!(best.kind, InstallKind::Native);
        // And an install of another emulator is not this one's to patch.
        let pcsx2 = h.emulator("pcsx2").unwrap();
        let patch = Patch {
            region: Some(Region::Europe),
            ..Default::default()
        };
        assert!(matches!(pcsx2.apply(&best, &patch), Err(Error::Invalid(_))));
    }

    fn on_linux(tmp: &std::path::Path) -> Hermir {
        let env = crate::detect::fake::FakeEnv::new(Os::Linux, tmp.join("home").to_str().unwrap());
        Hermir::open(Options {
            prefix: Some(tmp.join("prefix")),
            os: Some(Os::Linux),
            http: Box::new(FakeHttp::default()),
            runner: Box::new(FakeRunner::default()),
            env: Some(Box::new(env)),
            require_verified: false,
        })
        .unwrap()
    }

    #[test]
    fn a_profile_leaves_the_players_files_alone_and_launches_with_its_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let h = on_linux(tmp.path());
        let e = h.emulator("dolphin").unwrap();
        let own = tmp
            .path()
            .join("home/.var/app/org.DolphinEmu.dolphin-emu/config/dolphin-emu");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("GFX.ini"), "[Settings]\nInternalResolution = 1\n").unwrap();
        let copy = Install::new(
            "dolphin",
            InstallKind::Flatpak,
            Exe::FlatpakRun("org.DolphinEmu.dolphin-emu".into()),
            Some(own.clone()),
        );
        let p = e.profile(&copy, "punktfunk").unwrap();
        assert!(p.supported() && !p.exists());
        let patch = Patch {
            video: Some(Video {
                scale: Some(3),
                fullscreen: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let done = p.apply(&patch).unwrap();
        assert!(!done.failed(), "{done:?}");
        // The profile is the app's own data folder, where its sandbox reads; its files are
        // Dolphin's user-folder layout.
        let dir = tmp
            .path()
            .join("home/.var/app/org.DolphinEmu.dolphin-emu/data/hermir/profiles/punktfunk");
        assert_eq!(p.dir(), dir);
        let gfx = std::fs::read_to_string(dir.join("Config/GFX.ini")).unwrap();
        assert!(gfx.contains("InternalResolution = 3"), "{gfx}");
        assert_eq!(
            std::fs::read_to_string(own.join("GFX.ini")).unwrap(),
            "[Settings]\nInternalResolution = 1\n",
            "the player's own file is untouched"
        );
        assert!(!own.join("Dolphin.ini").exists());
        let read = p.get().unwrap();
        let scale = read.iter().find(|k| k.knob == "video.scale").unwrap();
        assert_eq!(scale.value.as_deref(), Some("3"));
        assert_eq!(e.profiles(&copy).unwrap(), ["punktfunk"]);

        let req = LaunchRequest {
            file: Some("/games/Metroid.rvz".into()),
            profile: Some("punktfunk".into()),
            ..Default::default()
        };
        let spec = e.launch(&copy, &req).unwrap();
        assert_eq!(
            spec.args[..2],
            ["-u".to_string(), dir.display().to_string()]
        );

        p.remove().unwrap();
        assert!(!p.exists());
        assert!(matches!(e.launch(&copy, &req), Err(Error::Invalid(_))));
        assert!(e.profile(&copy, "../escape").is_err());
    }

    #[test]
    fn a_profile_where_the_emulator_has_none_is_the_in_place_patch_and_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let h = on_linux(tmp.path());
        let e = h.emulator("duckstation").unwrap();
        let own = tmp.path().join("duck");
        let copy = Install::new(
            "duckstation",
            InstallKind::Native,
            Exe::Path("/usr/bin/duckstation-qt".into()),
            Some(own.clone()),
        );
        let p = e.profile(&copy, "x").unwrap();
        assert!(!p.supported());
        let done = p
            .apply(&Patch {
                region: Some(Region::Europe),
                ..Default::default()
            })
            .unwrap();
        assert!(done.note.as_deref().unwrap().contains("in place"));
        assert!(
            std::fs::read_to_string(own.join("settings.ini"))
                .unwrap()
                .contains("PAL")
        );
    }
}
