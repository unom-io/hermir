//! The command line, as clap parses it. Nouns first, verbs second; every verb takes `--json`.
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use hermir::{Aspect, Os, Region};

#[derive(Parser)]
#[command(
    name = "hermir",
    version,
    about = "One interface for managing emulators",
    after_help = "Exit codes: 0 ok · 1 I/O · 2 catalog, policy or an impossible request · 3 network · \
                  4 verification · 5 extract or place · 6 not offered on this OS · 7 a config file \
                  could not be written · 8 the prefix is locked · 9 not on this machine"
)]
pub struct Cli {
    /// Print data as JSON on stdout: one document per run, errors included.
    #[arg(long, global = true)]
    pub json: bool,
    /// The managed prefix (default: the OS data directory + hermir).
    #[arg(long, global = true, env = "HERMIR_PREFIX")]
    pub prefix: Option<PathBuf>,
    /// Resolve for another OS. Only `resolve` and `catalog` are meaningful across OSes.
    #[arg(long, global = true, value_parser = clap::value_parser!(Os))]
    pub os: Option<Os>,
    /// Refuse a download there is nothing to check against (exit 4), instead of installing it
    /// as not verified.
    #[arg(long, global = true, env = "HERMIR_REQUIRE_VERIFIED")]
    pub require_verified: bool,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// The catalog: what hermir knows.
    Catalog {
        #[command(subcommand)]
        cmd: CatalogCmd,
    },
    /// Managed and detected emulators, and whether an update exists.
    Status {
        emulator: Option<String>,
        /// Ask every archive channel whether it moved (one request each).
        #[arg(long)]
        check: bool,
    },
    /// Emulators the user installed, fresh from the machine.
    Detect,
    /// Install (or reinstall) from the channel for this OS.
    Install { emulator: String },
    /// Reinstall when the channel moved.
    Update {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        emulator: Option<String>,
        /// Every managed emulator; one that fails does not stop the others.
        #[arg(long)]
        all: bool,
    },
    /// Remove a managed install; `--purge` removes its data too.
    Remove {
        emulator: String,
        #[arg(long)]
        purge: bool,
    },
    /// The exe and config root of the best copy on this machine.
    Where { emulator: String },
    /// Make every copy ready to play: answer its first-run questions, place or install
    /// `--firmware` for `--platform`.
    Prepare {
        emulator: String,
        #[arg(long)]
        platform: Option<String>,
        #[arg(long)]
        firmware: Vec<PathBuf>,
    },
    /// Pads into every copy's own bindings, seat by seat (`config apply` with pads only), or
    /// with `--revert` the player's settings back. No pad means one Xbox 360 pad in seat 1.
    Players {
        emulator: String,
        #[arg(long, conflicts_with_all = ["pad", "players"])]
        revert: bool,
        #[command(flatten)]
        pads: PadArgs,
    },
    /// A session's settings: write them into every copy, put them back, or see what each
    /// emulator takes.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// The command that starts a game on the best copy: printed, never run.
    Launch(LaunchArgs),
    /// `launch`, then run it: the emulator's exit code is hermir's.
    Run(LaunchArgs),
    /// Profiles: a copy's own settings for a session, where the emulator has a flag for them,
    /// so the player's are never edited.
    Profile {
        #[command(subcommand)]
        cmd: ProfileCmd,
    },
    /// libretro cores for the RetroArch on this machine.
    Core {
        #[command(subcommand)]
        cmd: CoreCmd,
    },
    /// What this machine can and cannot do.
    Doctor,
}

#[derive(Subcommand)]
pub enum CatalogCmd {
    /// Every entry, with what it offers on this OS.
    List,
    /// One entry, in full.
    Show { emulator: String },
    /// Parse a catalog directory strictly and apply the rules a schema cannot.
    Validate { dir: PathBuf },
    /// The JSON Schema of an entry.
    Schema,
    /// Where each channel points right now, without downloading. `--all` walks the catalog.
    Resolve {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        emulator: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Write video, region, native keys and pads into every copy of an emulator. Every file
    /// is snapshotted first; `config revert` puts it back.
    Apply(ApplyArgs),
    /// Put the player's own files back: one emulator, or every one with changes outstanding.
    /// A file changed since hermir wrote it is left as it is, unless `--force`.
    Revert {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        emulator: Option<String>,
        #[arg(long)]
        all: bool,
        /// Restore files changed since hermir wrote them too.
        #[arg(long)]
        force: bool,
    },
    /// What `apply` can do, knob by knob, for one emulator or all of them.
    Support { emulator: Option<String> },
    /// What every copy's files hold now: each knob in the neutral spelling `apply` takes, one
    /// knob (`video.scale`), or a native key (`section/key`, in `--file`).
    Get {
        emulator: String,
        /// A knob name, or `section/key` for a key the model does not cover.
        what: Option<String>,
        /// The catalog's name for the file a native key is in.
        #[arg(long, default_value = "main")]
        file: String,
        /// Read this profile instead of the player's own settings.
        #[arg(long)]
        profile: Option<String>,
    },
}

#[derive(Args)]
pub struct ApplyArgs {
    pub emulator: String,
    /// Start in fullscreen: `--fullscreen` alone means yes, `--fullscreen=no` says no.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true",
          value_parser = clap::builder::BoolishValueParser::new())]
    pub fullscreen: Option<bool>,
    /// Internal resolution, as a multiple of the console's: 1 is native, 3 is 3×.
    #[arg(long)]
    pub scale: Option<u8>,
    /// Wait for the display's refresh: `--vsync` alone means yes, `--vsync=no` says no.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true",
          value_parser = clap::builder::BoolishValueParser::new())]
    pub vsync: Option<bool>,
    /// auto, 4:3, 16:9 or stretch.
    #[arg(long, value_parser = clap::value_parser!(Aspect))]
    pub aspect: Option<Aspect>,
    /// auto, jp, us or eu (also ntsc-j, ntsc-u, pal).
    #[arg(long, value_parser = clap::value_parser!(Region))]
    pub region: Option<Region>,
    /// A key the model does not cover, as `section/key=value` (`key=value` at the top
    /// level), written as given into `--file`. The section may itself contain slashes.
    #[arg(long = "set", value_name = "SECTION/KEY=VALUE")]
    pub set: Vec<String>,
    /// The catalog's name for the file `--set` goes into.
    #[arg(long, default_value = "main")]
    pub file: String,
    #[command(flatten)]
    pub pads: PadArgs,
    /// Write into this profile instead of the player's own settings (made on first use).
    #[arg(long)]
    pub profile: Option<String>,
}

/// Who plays where: pads in seat order, or the library's own `Player` list as JSON.
#[derive(Args)]
pub struct PadArgs {
    /// A pad, seat by seat: `VID:PID[:NAME][@INDEX]`, hex ids. NAME is the device name the
    /// kernel reports (needed for any pad but the wired Xbox 360 one); INDEX its position
    /// among the pads, by default the order given here.
    #[arg(long, value_name = "VID:PID[:NAME][@INDEX]")]
    pub pad: Vec<String>,
    /// Players as JSON, `[{ "seat": 1, "pad": { "name", "vendor", "product", "version",
    /// "index" } }]`, from a file or `-` for stdin.
    #[arg(long, value_name = "FILE", conflicts_with = "pad")]
    pub players: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum ProfileCmd {
    /// The profiles the best copy has.
    List { emulator: String },
    /// Make one from the player's own settings (`--fresh`: the emulator's defaults).
    Create {
        emulator: String,
        name: String,
        #[arg(long)]
        fresh: bool,
    },
    /// Make it again from the player's settings; what it held goes.
    Reset {
        emulator: String,
        name: String,
        #[arg(long)]
        fresh: bool,
    },
    /// Remove it, with everything the emulator kept in it.
    Remove { emulator: String, name: String },
    /// Its folder.
    Where { emulator: String, name: String },
}

#[derive(Subcommand)]
pub enum CoreCmd {
    /// Fetch `<core>_libretro` from the buildbot into RetroArch's cores directory.
    Install { core: String },
}

#[derive(Args)]
pub struct LaunchArgs {
    pub emulator: String,
    /// The game; without one the emulator opens on its own.
    pub file: Option<PathBuf>,
    /// The game's platform id (`snes`); RetroArch picks its core by it.
    #[arg(long)]
    pub platform: Option<String>,
    /// Start fullscreen: `--fullscreen` alone means yes, `--fullscreen=no` says no.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true",
          value_parser = clap::builder::BoolishValueParser::new())]
    pub fullscreen: Option<bool>,
    /// RetroArch: the libretro core in place of the platform's default.
    #[arg(long)]
    pub core: Option<String>,
    /// Run on this profile's settings.
    #[arg(long)]
    pub profile: Option<String>,
}
