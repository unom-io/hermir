//! hermir — one interface for managing emulators.
//!
//! ```no_run
//! use hermir::{Hermir, Options, progress::Quiet};
//! let h = Hermir::open(Options::default()).unwrap();
//! let pcsx2 = h.emulator("pcsx2").unwrap();
//! let row = pcsx2.install(&Quiet).unwrap();
//! println!("{}", row.exe);
//! ```
//!
//! `Hermir` holds the catalog, the prefix and the machine; an `EmulatorHandle` is one entry
//! of the catalog on this machine. Every type is serde and JSON Schema, so the CLI's `--json`
//! is the same contract as the library.
pub mod catalog;
pub mod channel;
pub mod detect;
pub mod error;
pub mod model;
pub mod progress;
pub mod store;

use std::path::PathBuf;

pub use catalog::Catalog;
pub use error::{Error, Result};
pub use model::*;
use progress::Progress;
pub use store::Store;

/// How to open hermir. Every field has a default that fits the machine it runs on.
pub struct Options {
    /// The managed prefix; `Store::default_root()` when unset.
    pub prefix: Option<PathBuf>,
    /// The OS to resolve for; the running one when unset. Another OS is useful only to resolve
    /// or dry-run — an install for another OS makes files nothing here can run.
    pub os: Option<Os>,
    pub http: Box<dyn channel::http::Http>,
    pub runner: Box<dyn channel::flatpak::Runner>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            prefix: None,
            os: None,
            http: Box::new(channel::http::Ureq::new()),
            runner: Box::new(channel::flatpak::Process),
        }
    }
}

pub struct Hermir {
    catalog: Catalog,
    store: Store,
    os: Os,
    http: Box<dyn channel::http::Http>,
    runner: Box<dyn channel::flatpak::Runner>,
    env: Box<dyn detect::Env>,
}

impl Hermir {
    pub fn open(opts: Options) -> Result<Hermir> {
        let os = opts.os.unwrap_or_else(Os::current);
        Ok(Hermir {
            catalog: Catalog::embedded()?,
            store: Store::open(opts.prefix.unwrap_or_else(Store::default_root))?,
            os,
            http: opts.http,
            runner: opts.runner,
            env: Box::new(detect::RealEnv::new(os)),
        })
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn os(&self) -> Os {
        self.os
    }

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
            let update = match (&row, check_updates) {
                (Some(r), true) if r.channel != "flatpak" => {
                    channel::resolve(e, self.os, self.http.as_ref())
                        .ok()
                        .filter(|res| channel::update_available(r, res))
                        .map(|res| res.release)
                }
                _ => None,
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
            });
        }
        if only.is_some() && out.is_empty() {
            return Err(Error::NotInCatalog(only.unwrap_or_default().into()));
        }
        Ok(out)
    }

    /// Installs a libretro core into the RetroArch this machine has (managed first).
    pub fn install_core(&self, core: &str, progress: &dyn Progress) -> Result<PathBuf> {
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

/// One catalog entry on this machine.
pub struct EmulatorHandle<'a> {
    h: &'a Hermir,
    entry: &'a Entry,
}

impl EmulatorHandle<'_> {
    pub fn entry(&self) -> &Entry {
        self.entry
    }

    /// Where the channel points now; nothing is downloaded.
    pub fn resolve(&self) -> Result<Resolved> {
        channel::resolve(self.entry, self.h.os, self.h.http.as_ref())
    }

    pub fn managed(&self) -> Result<Option<Installed>> {
        self.h.store.installed_one(&self.entry.id)
    }

    /// Installs, or reinstalls, from the channel for this OS.
    pub fn install(&self, progress: &dyn Progress) -> Result<Installed> {
        let _lock = self.h.store.lock()?;
        channel::install(
            self.entry,
            self.h.os,
            &self.h.store,
            self.h.http.as_ref(),
            self.h.runner.as_ref(),
            progress,
        )
    }

    /// Reinstalls when the channel moved; `Ok(None)` when it did not. A Flatpak asks
    /// `flatpak update`.
    pub fn update(&self, progress: &dyn Progress) -> Result<Option<Installed>> {
        let row = self.managed()?.ok_or_else(|| Error::Place {
            what: self.entry.id.clone(),
            why: "not installed by hermir".into(),
        })?;
        if let Exe::FlatpakRun(id) = &row.exe {
            let _lock = self.h.store.lock()?;
            channel::flatpak::update(self.h.runner.as_ref(), id)?;
            let fresh = Installed {
                version: channel::flatpak::version(self.h.runner.as_ref(), id),
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
        self.install(progress).map(Some)
    }

    pub fn remove(&self, purge: bool) -> Result<()> {
        let row = self.managed()?.ok_or_else(|| Error::Place {
            what: self.entry.id.clone(),
            why: "not installed by hermir".into(),
        })?;
        let _lock = self.h.store.lock()?;
        channel::remove(
            self.entry,
            &row,
            &self.h.store,
            self.h.runner.as_ref(),
            purge,
        )
    }

    /// Where this copy reads firmware, when the catalog knows.
    pub fn firmware_dir(&self, install: &Install) -> Option<PathBuf> {
        let fw = self.entry.firmware.as_ref()?;
        let root = install.config_root.as_ref()?;
        Some(if fw.dir == "." {
            root.clone()
        } else {
            root.join(&fw.dir)
        })
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
