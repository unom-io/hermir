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
mod content;
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
mod units;

use std::path::{Path, PathBuf};

pub use catalog::Catalog;
pub use channel::flatpak::{Output, Process, Runner};
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
    /// How hermir runs a program (`flatpak`, an emulator's own installer): [`Process`] by
    /// default. A consumer's own [`Runner`] chooses the account it runs as.
    pub runner: Box<dyn Runner>,
    /// What detection looks at (files, `PATH`, environment variables): the machine itself,
    /// [`RealEnv`] in [`Self::home`], when unset.
    pub env: Option<Box<dyn Env>>,
    /// Another account's home, for a host running as a service on the player's behalf: config
    /// roots under `~`, `%APPDATA%` and `Documents` resolve there ([`RealEnv::with_home`]; a
    /// given `env` still decides), and every [`LaunchSpec::env`] carries that home's per-user
    /// variables. The process's own when unset.
    pub home: Option<PathBuf>,
    /// Refuse a download there is nothing to check against (a GitHub asset without a digest,
    /// a libretro core) with [`Error::Unverified`], before anything is fetched. Off by default:
    /// such an install then records `verified: none`.
    pub require_verified: bool,
    /// The Flatpak installation a new install goes into; [`FlatpakInstallation::User`] by
    /// default. A managed Flatpak is updated and removed where its row says it went.
    pub flatpak_installation: FlatpakInstallation,
    /// Place a Windows archive for every account on the machine: without its portable marker,
    /// so each account keeps its own config and saves under `roots.windows` (`%APPDATA%`,
    /// `Documents`). An emulator whose `roots.windows` is `<app>` (RPCS3, Snes9x, Supermodel)
    /// keeps its data beside the exe, shared. Off by default; nothing changes on Linux.
    pub shared_install: bool,
    /// One account's own state, for a prefix several accounts share: the snapshots `apply`
    /// keeps and `revert` restores from, under a lock of their own, so one account's
    /// `revert_all` never undoes another's session and both can apply at once. `import_unit`'s
    /// `.save-backups` lives here too, since a replaced save belongs to the account it came from.
    /// Installs and their lock stay in the prefix. The prefix, and its lock, when unset.
    pub state_dir: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            prefix: None,
            os: None,
            http: Box::new(Ureq::new()),
            runner: Box::new(Process),
            env: None,
            home: None,
            require_verified: false,
            flatpak_installation: FlatpakInstallation::User,
            shared_install: false,
            state_dir: None,
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
    /// `Options::home`'s per-user variables, for every launch.
    home_vars: Vec<(&'static str, PathBuf)>,
    require_verified: bool,
    flatpak_installation: FlatpakInstallation,
    shared_install: bool,
    /// `Options::state_dir`, or the prefix.
    state: PathBuf,
}

impl Hermir {
    /// hermir for this machine, with the embedded catalog. Nothing is written until something
    /// is installed or applied.
    pub fn open(opts: Options) -> Result<Hermir> {
        let os = opts.os.unwrap_or_else(Os::current);
        let real = match opts.home {
            Some(home) => RealEnv::with_home(os, home),
            None => RealEnv::new(os),
        };
        let prefix = opts.prefix.unwrap_or_else(Store::default_root);
        Ok(Hermir {
            catalog: Catalog::embedded()?,
            state: opts.state_dir.unwrap_or_else(|| prefix.clone()),
            store: Store::open(prefix)?,
            os,
            http: opts.http,
            runner: opts.runner,
            home_vars: real.home_vars(),
            env: opts.env.unwrap_or_else(|| Box::new(real)),
            require_verified: opts.require_verified,
            flatpak_installation: opts.flatpak_installation,
            shared_install: opts.shared_install,
        })
    }

    /// What hermir knows.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// What a library needs of the catalog on this machine's OS.
    pub fn registry(&self) -> Registry {
        self.catalog.registry(self.os)
    }

    /// The prefix: managed installs, the lock; snapshots too without [`Options::state_dir`].
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The OS hermir resolves for.
    pub fn os(&self) -> Os {
        self.os
    }

    /// Where `apply` keeps what it overwrote.
    fn snapshots(&self) -> PathBuf {
        self.state.join(".snapshots")
    }

    /// The lock `apply`, `revert` and `import_unit` hold: the state dir's own, or the prefix
    /// lock without [`Options::state_dir`].
    fn state_lock(&self) -> Result<Lock> {
        store::lock_dir(&self.state)
    }

    /// Restores every emulator with a session's changes outstanding: what a consumer calls
    /// when the game it prepared has exited. A file changed since hermir wrote it is left as it
    /// is (a [`StepOutcome::Conflict`]) unless `force`. Holds the state lock while it writes.
    pub fn revert_all(&self, force: bool) -> Result<Vec<(String, Vec<PrepareStep>)>> {
        let _lock = self.state_lock()?;
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

    /// Copies the user installed, fresh from the machine, and the ones the operator adopted.
    pub fn detect(&self) -> Vec<Install> {
        let env = self.env.as_ref();
        let mut found = detect::detect(&self.catalog, env);
        for a in self.store.adopted().unwrap_or_default() {
            let Some(entry) = self.catalog.get(&a.emulator) else {
                continue;
            };
            let known = found.iter().any(|i| {
                i.emulator == a.emulator
                    && matches!(&i.exe, Exe::Path(p) if env.canonical(p) == env.canonical(&a.exe))
            });
            if !known && env.exists(&a.exe) {
                found.push(detect::adopted(entry, a.exe, env));
            }
        }
        found
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
            // A build without a portable marker, none in its channel or left out of a shared
            // copy, keeps the layout of the OS: an AppImage the native one, a Windows zip
            // `roots.windows` (`<app>` for RPCS3), as detection resolves them.
            (Exe::Path(p), Some(e)) if r.shared || !marks_portable(e, self.os) => match self.os {
                Os::Windows => e.roots.windows.as_deref(),
                _ => e.roots.native.as_deref(),
            }
            .map(|t| detect::expand(t, self.env.as_ref(), p.parent())),
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

    /// Installs, or reinstalls, from the channel for this OS, as `Options` says.
    pub fn install(&self, progress: &dyn Progress) -> Result<Installed> {
        let _lock = self.h.store.lock()?;
        self.install_locked(progress, self.h.shared_install)
    }

    fn install_locked(&self, progress: &dyn Progress, shared: bool) -> Result<Installed> {
        channel::install(
            self.entry,
            self.h.os,
            &self.h.store,
            self.h.http.as_ref(),
            self.h.runner.as_ref(),
            progress,
            self.h.require_verified,
            self.h.flatpak_installation,
            shared,
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

    /// Reinstalls when the channel moved, keeping the copy shared or not as its row says;
    /// `Ok(None)` when it did not. A Flatpak asks `flatpak update`, in the installation its row
    /// names.
    pub fn update(&self, progress: &dyn Progress) -> Result<Option<Installed>> {
        let _lock = self.h.store.lock()?;
        let row = self.managed_row()?;
        if let Exe::FlatpakRun(id) = &row.exe {
            let at = row.installation.unwrap_or_default();
            channel::flatpak::update(self.h.runner.as_ref(), id, at)?;
            let version = channel::flatpak::version(self.h.runner.as_ref(), id, at);
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
        self.install_locked(progress, row.shared).map(Some)
    }

    /// Removes the managed copy, a Flatpak from the installation its row names; with `purge`,
    /// its data too (a Flatpak's `~/.var/app/<id>`).
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
    /// state lock ([`Options::state_dir`]) is held while it writes.
    pub fn apply(&self, install: &Install, patch: &Patch) -> Result<Applied> {
        self.mine(install)?;
        patch.validate().map_err(Error::Invalid)?;
        let _lock = self.h.state_lock()?;
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
    /// snapshot kept (a [`StepOutcome::Conflict`]), unless `force`. The state lock is held
    /// while it writes.
    pub fn revert(&self, force: bool) -> Result<Vec<PrepareStep>> {
        let _lock = self.h.state_lock()?;
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

    /// Points hermir at a copy no rule finds (a portable build unpacked anywhere), so it is
    /// detected like any other from now on; `keep: false` forgets it again. The program must be
    /// an absolute path to a file that is there.
    pub fn adopt(&self, exe: &Path, keep: bool) -> Result<Option<Install>> {
        let env = self.h.env.as_ref();
        if keep && (!exe.is_absolute() || !env.exists(exe)) {
            return Err(Error::Invalid(format!(
                "{} is not a program on this machine",
                exe.display()
            )));
        }
        let _lock = self.h.store.lock()?;
        let row = Adopted {
            emulator: self.entry.id.clone(),
            exe: exe.to_path_buf(),
        };
        self.h.store.set_adopted(row, keep)?;
        Ok(keep.then(|| detect::adopted(self.entry, exe.to_path_buf(), env)))
    }

    /// The libretro cores a RetroArch copy has, by the name a launch asks for (`snes9x`).
    pub fn cores(&self, install: &Install) -> Vec<String> {
        let Some(dir) = install.config_root.as_ref().map(|r| r.join("cores")) else {
            return Vec::new();
        };
        let mut out: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let stem = name
                    .strip_suffix(".so")
                    .or_else(|| name.strip_suffix(".dll"))
                    .or_else(|| name.strip_suffix(".dylib"))?;
                stem.strip_suffix("_libretro").map(str::to_string)
            })
            .collect();
        out.sort();
        out
    }

    /// Every save unit of `platform` on this copy, each with a stamp that changes with it.
    /// `game` is the game's folder, for an emulator that saves beside its games.
    pub fn units(
        &self,
        install: &Install,
        platform: &str,
        game: Option<&Path>,
    ) -> Result<Vec<SaveUnit>> {
        self.mine(install)?;
        units::list(
            self.entry,
            self.h.os,
            install,
            self.h.env.as_ref(),
            platform,
            game,
        )
        .map_err(Error::Invalid)
    }

    /// Writes one unit into the folder `out`, named for the unit: the save file, or a tar of
    /// the folder with nothing in its headers that differs between machines.
    #[allow(clippy::too_many_arguments)]
    pub fn export_unit(
        &self,
        install: &Install,
        platform: &str,
        game: Option<&Path>,
        kind: SaveKind,
        name: &str,
        out: &Path,
    ) -> Result<ExportedUnit> {
        self.mine(install)?;
        units::export(
            self.entry,
            self.h.os,
            install,
            self.h.env.as_ref(),
            platform,
            game,
            kind,
            name,
            out,
        )
        .map_err(Error::Invalid)
    }

    /// Puts the unit in the file `from` where this copy keeps it, in the first folder of `kinds`
    /// that has a place for its name; what was there is kept under `.save-backups` in
    /// [`Options::state_dir`], or the prefix. `others` are the names a server holds for the same
    /// game. A unit this copy has no place for yet is a step that says so, not an error. Holds
    /// the state lock.
    #[allow(clippy::too_many_arguments)]
    pub fn import_unit(
        &self,
        install: &Install,
        platform: &str,
        game: Option<&Path>,
        kinds: &[SaveKind],
        name: &str,
        from: &Path,
        others: &[String],
    ) -> Result<PrepareStep> {
        self.mine(install)?;
        let _lock = self.h.state_lock()?;
        units::import(
            self.entry,
            self.h.os,
            install,
            self.h.env.as_ref(),
            platform,
            game,
            kinds,
            name,
            from,
            others,
            &self.h.state.join(".save-backups").join(&self.entry.id),
        )
        .map_err(Error::Invalid)
    }

    /// Installs a game's add-ons (`kind` `update` or `dlc`) the way this emulator takes them;
    /// one step per add-on. An emulator with no way says why ([`Error::Invalid`]). Holds the
    /// prefix lock.
    pub fn install_content(
        &self,
        install: &Install,
        platform: &str,
        kind: &str,
        files: &[PathBuf],
    ) -> Result<Vec<ContentStep>> {
        self.mine(install)?;
        let _lock = self.h.store.lock()?;
        content::install(
            self.entry,
            self.h.os,
            install,
            platform,
            kind,
            files,
            self.h.runner.as_ref(),
        )
        .map_err(Error::Invalid)
    }

    /// Whether this copy has `platform`'s firmware, each file hashed against the good dumps the
    /// catalog knows. `None` when the platform needs none.
    pub fn firmware_status(
        &self,
        install: &Install,
        platform: &str,
    ) -> Result<Option<FirmwareStatus>> {
        self.mine(install)?;
        Ok(prepare::status(self.entry, install, platform))
    }

    /// The command that starts `install` with `req`: the catalog's template rendered, the
    /// game's folder granted to a Flatpak, launch-only knobs from the request's patch, and
    /// `Options::home`'s per-user variables in `env`. hermir never runs it; the consumer does,
    /// its own way.
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
        let mut spec = launch::launch(self.entry, self.h.os, install, req, profile.as_deref())
            .map_err(Error::Invalid)?;
        spec.env.extend(
            self.h
                .home_vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string_lossy().into_owned())),
        );
        Ok(spec)
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
        let _lock = self.h.state_lock()?;
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
            require_verified,
            ..Options::default()
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

    /// Two accounts on one prefix: each its own snapshots, backups and lock.
    #[test]
    fn each_state_dir_keeps_its_own_session() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("prefix");
        let seat = |state: Option<&str>| {
            Hermir::open(Options {
                prefix: Some(prefix.clone()),
                state_dir: state.map(|s| tmp.path().join(s)),
                os: Some(Os::Windows),
                http: Box::new(FakeHttp::default()),
                runner: Box::new(FakeRunner::default()),
                ..Options::default()
            })
            .unwrap()
        };
        let (a, b, own) = (seat(Some("a")), seat(Some("b")), seat(None));
        let root = |who: &str| tmp.path().join(who).join("PCSX2");
        let ini = |who: &str| root(who).join("inis/PCSX2.ini");
        let patch = Patch {
            video: Some(Video {
                fullscreen: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let apply = |h: &Hermir, who: &str| {
            h.emulator("pcsx2")
                .unwrap()
                .apply(&pcsx2_copy(&root(who)), &patch)
        };
        for (h, who) in [(&a, "a-home"), (&b, "b-home"), (&own, "own-home")] {
            assert!(!apply(h, who).unwrap().failed());
        }
        assert!(
            tmp.path()
                .join("a/.snapshots/pcsx2/manifest.json")
                .is_file()
        );
        assert!(prefix.join(".snapshots/pcsx2/manifest.json").is_file());

        // One account's revert leaves the other sessions as they are.
        assert_eq!(a.revert_all(true).unwrap().len(), 1);
        assert!(!ini("a-home").exists());
        assert!(ini("b-home").is_file() && ini("own-home").is_file());
        assert_eq!(config::outstanding(&b.snapshots()), ["pcsx2"]);
        assert_eq!(own.revert_all(true).unwrap().len(), 1);
        assert!(ini("b-home").is_file());

        // Neither the prefix lock nor another account's blocks this one.
        let _box = own.store().lock().unwrap();
        let _a = a.state_lock().unwrap();
        assert!(matches!(apply(&a, "a-home"), Err(Error::Locked(_))));
        assert!(apply(&b, "b-home").is_ok());
        assert_eq!(b.revert_all(true).unwrap().len(), 1);

        // A save `import_unit` replaces is backed up with the account's state.
        std::fs::create_dir_all(root("b-home").join("memcards")).unwrap();
        std::fs::write(root("b-home").join("memcards/Mcd001.ps2"), "old").unwrap();
        let from = tmp.path().join("Mcd001.ps2");
        std::fs::write(&from, "new").unwrap();
        let step = b
            .emulator("pcsx2")
            .unwrap()
            .import_unit(
                &pcsx2_copy(&root("b-home")),
                "ps2",
                None,
                &[SaveKind::Memcard],
                "Mcd001.ps2",
                &from,
                &[],
            )
            .unwrap();
        assert_eq!(step.outcome, StepOutcome::Applied, "{step:?}");
        assert!(tmp.path().join("b/.save-backups/pcsx2").is_dir());
        assert!(!prefix.join(".save-backups").exists());
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
            ..Options::default()
        })
        .unwrap();
        {
            let _lock = h.store().lock().unwrap();
            h.store()
                .record(archive_row("xemu", prefix.join("xemu/app/xemu.exe"), false))
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

    fn archive_row(id: &str, exe: PathBuf, shared: bool) -> Installed {
        Installed {
            emulator: id.into(),
            channel: "github".into(),
            exe: Exe::Path(exe),
            version: None,
            release: None,
            sha256: None,
            verified: Verified::None,
            kept: Vec::new(),
            installation: None,
            shared,
            installed_at: "2026-10-03T00:00:00Z".into(),
        }
    }

    /// The config root a managed row of `id` resolves to.
    fn managed_root(h: &Hermir, id: &str, shared: bool) -> Option<PathBuf> {
        let exe = h.store().app_dir(id).join(format!("{id}.exe"));
        {
            let _lock = h.store().lock().unwrap();
            h.store().record(archive_row(id, exe, shared)).unwrap();
        }
        h.emulator(id).unwrap().copies().unwrap()[0]
            .config_root
            .clone()
    }

    #[test]
    fn a_shared_copy_reads_each_accounts_folder_and_rpcs3_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let env = crate::detect::fake::FakeEnv::new(Os::Windows, "C:\\Users\\u")
            .var("USERPROFILE", "C:\\Users\\u");
        let h = Hermir::open(Options {
            prefix: Some(tmp.path().to_path_buf()),
            os: Some(Os::Windows),
            http: Box::new(FakeHttp::default()),
            runner: Box::new(FakeRunner::default()),
            env: Some(Box::new(env)),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(
            managed_root(&h, "duckstation", true),
            Some(PathBuf::from("C:\\Users\\u\\Documents\\DuckStation"))
        );
        assert_eq!(
            managed_root(&h, "duckstation", false),
            Some(h.store().app_dir("duckstation"))
        );
        // RPCS3 has no per-account layout: its data stays beside the exe.
        assert_eq!(
            managed_root(&h, "rpcs3", false),
            Some(h.store().app_dir("rpcs3"))
        );
    }

    #[test]
    fn a_home_moves_the_accounts_folders_and_rides_along_with_every_launch() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("player");
        let h = Hermir::open(Options {
            prefix: Some(tmp.path().join("prefix")),
            os: Some(Os::Windows),
            http: Box::new(FakeHttp::default()),
            runner: Box::new(FakeRunner::default()),
            home: Some(home.clone()),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(
            managed_root(&h, "duckstation", true),
            Some(PathBuf::from(format!(
                "{}\\Documents\\DuckStation",
                home.display()
            )))
        );
        let e = h.emulator("duckstation").unwrap();
        let copy = e.copies().unwrap().remove(0);
        let spec = e.launch(&copy, &LaunchRequest::default()).unwrap();
        let var = |k: &str| spec.env.get(k).map(PathBuf::from);
        assert_eq!(var("USERPROFILE"), Some(home.clone()));
        assert_eq!(var("APPDATA"), Some(home.join("AppData").join("Roaming")));
        assert_eq!(
            var("LOCALAPPDATA"),
            Some(home.join("AppData").join("Local"))
        );

        let own = open(&tmp.path().join("prefix"), false);
        let e = own.emulator("duckstation").unwrap();
        let spec = e.launch(&copy, &LaunchRequest::default()).unwrap();
        assert!(spec.env.is_empty(), "{:?}", spec.env);
    }

    #[test]
    fn a_flatpak_is_updated_and_removed_where_it_was_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = std::sync::Arc::new(FakeRunner {
            info_version: Some("2.8.2".into()),
            ..Default::default()
        });
        let open = |at: FlatpakInstallation| {
            Hermir::open(Options {
                prefix: Some(tmp.path().to_path_buf()),
                os: Some(Os::Linux),
                http: Box::new(FakeHttp::default()),
                runner: Box::new(runner.clone()),
                env: Some(Box::new(crate::detect::fake::FakeEnv::new(
                    Os::Linux,
                    "/home/u",
                ))),
                flatpak_installation: at,
                ..Options::default()
            })
            .unwrap()
        };
        let row = open(FlatpakInstallation::System)
            .emulator("pcsx2")
            .unwrap()
            .install(&Quiet)
            .unwrap();
        assert_eq!(row.installation, Some(FlatpakInstallation::System));
        // Opened for the user's installation: the row, not the option, says where the app is.
        let h = open(FlatpakInstallation::User);
        let e = h.emulator("pcsx2").unwrap();
        e.update(&Quiet).unwrap();
        e.remove(false).unwrap();
        let calls = runner.calls.lock().unwrap();
        let verbs: Vec<_> = calls
            .iter()
            .map(|c| (c[1].as_str(), c[2].as_str()))
            .collect();
        assert_eq!(
            verbs,
            [
                ("remote-add", "--system"),
                ("install", "--system"),
                ("info", "--system"),
                ("update", "--system"),
                ("info", "--system"),
                ("uninstall", "--system"),
            ]
        );
    }

    fn on_linux(tmp: &std::path::Path) -> Hermir {
        let env = crate::detect::fake::FakeEnv::new(Os::Linux, tmp.join("home").to_str().unwrap());
        Hermir::open(Options {
            prefix: Some(tmp.join("prefix")),
            os: Some(Os::Linux),
            http: Box::new(FakeHttp::default()),
            runner: Box::new(FakeRunner::default()),
            env: Some(Box::new(env)),
            ..Options::default()
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

    #[test]
    fn an_adopted_copy_is_detected_until_it_is_forgotten() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("Eden-portable/eden");
        let env =
            crate::detect::fake::FakeEnv::new(Os::Linux, "/home/u").file(exe.to_str().unwrap());
        let h = Hermir::open(Options {
            prefix: Some(tmp.path().join("prefix")),
            os: Some(Os::Linux),
            http: Box::new(FakeHttp::default()),
            runner: Box::new(FakeRunner::default()),
            env: Some(Box::new(env)),
            ..Options::default()
        })
        .unwrap();
        let e = h.emulator("eden").unwrap();
        assert!(e.copies().unwrap().is_empty());
        let copy = e.adopt(&exe, true).unwrap().unwrap();
        assert_eq!(copy.exe, Exe::Path(exe.clone()));
        assert_eq!(e.copies().unwrap().len(), 1);
        assert!(e.adopt(Path::new("relative/eden"), true).is_err());
        e.adopt(&exe, false).unwrap();
        assert!(e.copies().unwrap().is_empty());
    }

    #[test]
    fn a_retroarch_copy_lists_its_cores_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let h = on_linux(tmp.path());
        let root = tmp.path().join("retroarch");
        std::fs::create_dir_all(root.join("cores")).unwrap();
        for f in ["snes9x_libretro.so", "mgba_libretro.dll", "readme.txt"] {
            std::fs::write(root.join("cores").join(f), b"").unwrap();
        }
        let copy = Install::new(
            "retroarch",
            InstallKind::Native,
            Exe::Path("/usr/bin/retroarch".into()),
            Some(root),
        );
        assert_eq!(
            h.emulator("retroarch").unwrap().cores(&copy),
            ["mgba", "snes9x"]
        );
    }

    #[test]
    fn firmware_status_hashes_what_is_there_and_a_wrong_dump_is_not_placed() {
        let tmp = tempfile::tempdir().unwrap();
        let h = on_linux(tmp.path());
        let e = h.emulator("retroarch").unwrap();
        let root = tmp.path().join("retroarch");
        let copy = Install::new(
            "retroarch",
            InstallKind::Native,
            Exe::Path("/usr/bin/retroarch".into()),
            Some(root.clone()),
        );
        let src = tmp.path().join("bios");
        std::fs::create_dir_all(&src).unwrap();
        let bad = src.join("sega_101.bin");
        std::fs::write(&bad, b"not a saturn bios").unwrap();
        let done = e.prepare(&copy, Some("saturn"), &[bad]).unwrap();
        let fw = done.steps.iter().find(|s| s.kind == "firmware").unwrap();
        assert_eq!(fw.outcome, StepOutcome::Failed);
        assert!(!root.join("system/sega_101.bin").exists());
        let s = e.firmware_status(&copy, "saturn").unwrap().unwrap();
        assert!(!s.ok && s.found.is_empty());
        // Placed by hand anyway: found, hashed, and not taken as good.
        std::fs::create_dir_all(root.join("system")).unwrap();
        std::fs::write(root.join("system/sega_101.bin"), b"not a saturn bios").unwrap();
        let s = e.firmware_status(&copy, "saturn").unwrap().unwrap();
        assert_eq!(s.found.len(), 1);
        assert_eq!(s.found[0].known, Some(false));
        assert!(!s.ok);
        // A family of names with no hashes: there is enough.
        std::fs::write(root.join("system/scph5501.bin"), b"x").unwrap();
        let s = e.firmware_status(&copy, "psx").unwrap().unwrap();
        assert!(s.ok && s.found[0].known.is_none());
        assert!(e.firmware_status(&copy, "snes").unwrap().is_none());
    }
}
