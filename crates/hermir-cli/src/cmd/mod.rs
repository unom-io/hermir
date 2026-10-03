//! One function per verb, each returning an [`Outcome`]: the data (the `--json` document), the
//! lines a person reads, and the exit code. `main` prints it; nothing here exits.
use hermir::{Catalog, Error, Hermir, Install, Options, Result};
use serde::Serialize;

use crate::cli::{CatalogCmd, Cli, Cmd, ConfigCmd, CoreCmd, ProfileCmd, SavesCmd};

mod catalog;
mod check;
mod config;
mod install;
mod launch;
mod machine;
mod pads;
mod profile;

pub use pads::enumerate;

pub struct Outcome {
    /// What `--json` prints: exactly one document.
    pub json: serde_json::Value,
    /// What a person reads on stdout.
    pub human: String,
    /// A line for stderr: what went wrong in a run that still has data to show.
    pub note: Option<String>,
    pub exit: i32,
}

impl Outcome {
    pub fn new(data: &impl Serialize, human: impl Into<String>) -> Outcome {
        Outcome {
            json: serde_json::to_value(data).unwrap_or_default(),
            human: human.into(),
            note: None,
            exit: 0,
        }
    }

    /// A run that failed partway: its data stays, the note says what failed.
    fn failed(mut self, exit: i32, note: impl Into<String>) -> Outcome {
        self.exit = exit;
        self.note = Some(note.into());
        self
    }

    pub fn error(e: &Error) -> Outcome {
        Outcome {
            json: serde_json::json!({ "error": e.to_string(), "exit": e.exit_code() }),
            human: String::new(),
            note: Some(e.to_string()),
            exit: e.exit_code(),
        }
    }
}

/// One copy's result, `install` beside the result's own fields.
#[derive(Serialize)]
struct PerCopy<'a, T: Serialize> {
    install: &'a Install,
    #[serde(flatten)]
    result: &'a T,
}

pub fn run(cli: Cli) -> Result<Outcome> {
    // These need no machine and no prefix.
    match &cli.cmd {
        Cmd::Catalog {
            cmd: CatalogCmd::Schema,
        } => {
            let schema = Catalog::entry_schema();
            let text = serde_json::to_string_pretty(&schema).unwrap_or_default();
            return Ok(Outcome::new(&schema, text));
        }
        Cmd::Catalog {
            cmd: CatalogCmd::Validate { dir },
        } => return catalog::validate(dir),
        Cmd::Pads { watch } => return pads::pads(cli.json, *watch),
        _ => {}
    }
    let h = Hermir::open(Options {
        prefix: cli.prefix,
        os: cli.os,
        require_verified: cli.require_verified,
        ..Options::default()
    })?;
    match cli.cmd {
        Cmd::Catalog { cmd } => match cmd {
            CatalogCmd::List => Ok(catalog::list(&h)),
            CatalogCmd::Show { emulator } => catalog::show(&h, &emulator),
            CatalogCmd::Resolve { emulator, all } => catalog::resolve(&h, emulator, all),
            CatalogCmd::Schema | CatalogCmd::Validate { .. } => unreachable!("handled above"),
        },
        Cmd::Pads { .. } => unreachable!("handled above"),
        Cmd::Status { emulator, check } => machine::status(&h, emulator, check),
        Cmd::Detect => Ok(machine::detect(&h)),
        Cmd::Where { emulator } => machine::where_(&h, &emulator),
        Cmd::Saves {
            cmd: SavesCmd::Where { emulator, platform },
        } => machine::saves(&h, &emulator, platform.as_deref()),
        Cmd::Doctor => machine::doctor(&h),
        Cmd::Check {
            emulator,
            seconds,
            keep,
        } => check::check(&h, &emulator, seconds, keep),
        Cmd::Install { emulator } => install::install(&h, &emulator, cli.json),
        Cmd::Update { emulator, all } => install::update(&h, emulator, all),
        Cmd::Remove { emulator, purge } => install::remove(&h, &emulator, purge),
        Cmd::Core {
            cmd: CoreCmd::Install { core },
        } => install::core(&h, &core),
        Cmd::Profile { cmd } => match cmd {
            ProfileCmd::List { emulator } => profile::list(&h, &emulator),
            ProfileCmd::Create {
                emulator,
                name,
                fresh,
            } => profile::create(&h, &emulator, &name, fresh, false),
            ProfileCmd::Reset {
                emulator,
                name,
                fresh,
            } => profile::create(&h, &emulator, &name, fresh, true),
            ProfileCmd::Remove { emulator, name } => profile::remove(&h, &emulator, &name),
            ProfileCmd::Where { emulator, name } => profile::where_(&h, &emulator, &name),
        },
        Cmd::Launch(a) => launch::launch(&h, &a),
        Cmd::Run(a) => launch::run(&h, &a),
        Cmd::Prepare {
            emulator,
            platform,
            firmware,
        } => config::prepare(&h, &emulator, platform.as_deref(), &firmware),
        Cmd::Players {
            emulator,
            revert,
            pads,
        } => config::players(&h, &emulator, revert, &pads),
        Cmd::Config { cmd } => match cmd {
            ConfigCmd::Apply(a) => config::apply(&h, &a),
            ConfigCmd::Revert {
                emulator,
                all,
                force,
            } => config::revert(&h, emulator, all, force),
            ConfigCmd::Support { emulator } => config::support(&h, emulator.as_deref()),
            ConfigCmd::Get {
                emulator,
                what,
                file,
                profile,
            } => config::get(&h, &emulator, what.as_deref(), &file, profile.as_deref()),
        },
    }
}

/// Every copy of `emulator` on this machine, or why there is none to act on.
fn copies(h: &Hermir, emulator: &str) -> Result<Vec<Install>> {
    let copies = h.emulator(emulator)?.copies()?;
    if copies.is_empty() {
        return Err(Error::NotInstalled(emulator.into()));
    }
    Ok(copies)
}
