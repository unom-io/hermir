//! The types every layer shares. Catalog entries are what `catalog/emulators/*.json` deserialize
//! into, strictly; `Install`/`Installed` describe emulators on a machine. Everything here is
//! plain data: serde both ways, JSON Schema, no I/O.
use std::collections::BTreeMap;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The operating systems hermir knows. `macos` has schema slots and no entries yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Linux,
    Windows,
    Macos,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Macos
        } else {
            Os::Linux
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Windows => "windows",
            Os::Macos => "macos",
        }
    }
}

impl std::fmt::Display for Os {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Os {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Os, String> {
        match s {
            "linux" => Ok(Os::Linux),
            "windows" => Ok(Os::Windows),
            "macos" => Ok(Os::Macos),
            other => Err(format!("unknown os {other}; linux, windows or macos")),
        }
    }
}

/// One slot per OS. A missing key is `None`: honest "not offered here".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PerOs<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linux: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windows: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub macos: Option<T>,
}

impl<T> Default for PerOs<T> {
    fn default() -> Self {
        PerOs {
            linux: None,
            windows: None,
            macos: None,
        }
    }
}

impl<T> PerOs<T> {
    pub fn get(&self, os: Os) -> Option<&T> {
        match os {
            Os::Linux => self.linux.as_ref(),
            Os::Windows => self.windows.as_ref(),
            Os::Macos => self.macos.as_ref(),
        }
    }
}

/// A catalog entry: one emulator, as `catalog/emulators/<id>.json` spells it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Lowercase, `[a-z0-9-]`, equal to the file name.
    pub id: String,
    pub name: String,
    /// Platform ids from `catalog/platforms.json`.
    pub platforms: Vec<String>,
    /// SPDX expression of the emulator's own licence.
    pub license: String,
    /// Why this entry carries no install channel, when that is deliberate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_install: Option<String>,
    /// Where to get it, per OS. Empty means detect-and-configure only.
    #[serde(default)]
    pub channels: PerOs<Channel>,
    /// How to find a copy the user installed, per OS.
    #[serde(default)]
    pub detect: PerOs<Detect>,
    /// Where the emulator keeps its config and data, per install kind. `<app>` is the exe's
    /// directory, `~` the home, `%VAR%` and `$VAR` environment variables.
    #[serde(default)]
    pub roots: Roots,
    pub launch: Launch,
    /// Where the emulator reads firmware, and what each platform needs there. The folder is
    /// created at install, so a consumer can be granted exactly that one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<Firmware>,
    /// The answers to what a fresh copy asks before it plays anything (a setup wizard, a
    /// welcome box), per OS. `prepare` writes them; each one is what clicking through writes.
    #[serde(default)]
    pub first_run: PerOs<Vec<FirstRun>>,
    /// How a copy is configured: its settings files, where each neutral knob lands in them,
    /// how its player bindings are written. Absent while nothing is described yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Config>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// What `apply` may write into a copy, as the catalog describes it: data, so an emulator with
/// plain `key = value` settings needs no code at all.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Named files under the config root. Knobs, adapters and `Patch::native` refer to them
    /// by name; `main` is the conventional name of the settings file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, ConfigFile>,
    /// Knob → where it lands, or why it cannot. The knobs are [`KNOBS`]; one not listed here
    /// is reported as not described.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub knobs: BTreeMap<String, Knob>,
    /// How player bindings are written. Absent: not described.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub players: Option<PlayersSupport>,
}

/// The neutral knobs, in the order `support` lists them.
pub const KNOBS: &[&str] = &[
    "video.fullscreen",
    "video.scale",
    "video.vsync",
    "video.aspect",
    "region",
];

/// One settings file of an emulator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    /// Relative to the config root, for every OS or per OS where the layout differs.
    /// `{config}/` at the start is the settings directory beside a data root: `~/.config/<x>`
    /// for `~/.local/share/<x>`, a Flatpak's `config/<x>` for its `data/<x>`, the root itself
    /// elsewhere.
    pub path: FilePath,
    pub format: Format,
    /// XML: the document element the keys hang off (Cemu's `content`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// INI: the emulator matches section and key names whatever their case (Dolphin), so a
    /// file that spells `[ui]` is edited there rather than given a second `[UI]`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub case_insensitive: bool,
}

/// A path for every OS, or one per OS.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum FilePath {
    Same(String),
    PerOs(PerOs<String>),
}

impl FilePath {
    pub fn get(&self, os: Os) -> Option<&str> {
        match self {
            FilePath::Same(p) => Some(p),
            FilePath::PerOs(p) => p.get(os).map(String::as_str),
        }
    }

    /// Every path spelled, for validation.
    pub fn all(&self) -> Vec<&str> {
        match self {
            FilePath::Same(p) => vec![p],
            FilePath::PerOs(p) => [&p.linux, &p.windows, &p.macos]
                .into_iter()
                .flatten()
                .map(String::as_str)
                .collect(),
        }
    }
}

/// How a settings file is patched. Each is a line editor that leaves everything else alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Format {
    /// `key = value` under `[section]`: ini, flat TOML, RetroArch's cfg. An empty section is
    /// the top of the file.
    Ini,
    /// Qt's ini: every key written gets its `key\default=false` companion, or Qt ignores it.
    Qt,
    /// Two-level YAML: `Section:` then `  Key: value`; an empty section is the top level.
    Yaml,
    /// ares's BML: `Section` then `  Key: value`.
    Bml,
    /// Elements by path under the file's `root`; `section` is the path between root and key,
    /// `/`-separated, empty for a child of the root.
    Xml,
    /// A key of the root object, or of the object `section` names directly under it.
    Json,
}

/// Where a neutral knob lands in an emulator's files, or why it cannot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
#[non_exhaustive]
pub enum Knob {
    /// The emulator has no such setting; the note is what a UI shows.
    Unsupported {
        unsupported: String,
    },
    Bound(Box<Binding>),
}

/// One knob bound to one key, with the spelling the emulator expects. Exactly one of `bool`,
/// `values` and `scale` is present, by the knob's type: `bool` for `video.fullscreen` and
/// `video.vsync`, `values` for `video.aspect` and `region`, `scale` for `video.scale`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// A name from `files`.
    pub file: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub section: String,
    pub key: String,
    /// `[true, false]` as the emulator spells them (`True`/`False`, `1`/`0`, `yes`/`no`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bool: Option<[String; 2]>,
    /// Neutral value → the emulator's literal, quotes included where its format wants them.
    /// A neutral value missing here is unsupported, and says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Scale>,
    /// Keys written alongside every write of this knob (the renderer a scale needs, a second
    /// axis, a mode string beside a flag).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also: Vec<Also>,
    /// A caveat that makes the knob partial: it is written, and this is what to know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A key written alongside a knob. `value` is a literal, with `{value}` standing for the
/// knob's own rendered value; `bool` and `values` spell the knob's neutral value for this key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Also {
    /// Another file name; the knob's own by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Another section; the knob's own by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bool: Option<[String; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<BTreeMap<String, String>>,
}

/// How `video.scale`, a multiplier `n` of the console's resolution, is spelled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Scale {
    /// `n` itself, `min..=max`.
    Multiplier {
        #[serde(default = "one")]
        min: u8,
        #[serde(default = "eight")]
        max: u8,
    },
    /// `n × 100`, a percentage of the console's resolution (RPCS3).
    Percent {
        #[serde(default = "one")]
        min: u8,
        #[serde(default = "eight")]
        max: u8,
    },
    /// `n × base`, vertical lines (Flycast's 480).
    Lines {
        base: u32,
        #[serde(default = "eight")]
        max: u8,
    },
    /// `n` (as a decimal string, JSON's key) → the emulator's own value; an `n` missing here
    /// is unsupported.
    Map(BTreeMap<String, String>),
}

fn one() -> u8 {
    1
}

fn eight() -> u8 {
    8
}

impl Scale {
    /// The literal for `n`, or `None` when `n` is out of range.
    pub fn render(&self, n: u8) -> Option<String> {
        match self {
            Scale::Multiplier { min, max } => (*min..=*max).contains(&n).then(|| n.to_string()),
            Scale::Percent { min, max } => (*min..=*max)
                .contains(&n)
                .then(|| (u32::from(n) * 100).to_string()),
            Scale::Lines { base, max } => (1..=*max)
                .contains(&n)
                .then(|| (u32::from(n) * base).to_string()),
            Scale::Map(map) => map.get(&n.to_string()).cloned(),
        }
    }

    /// `(min, max)` of what renders.
    pub fn range(&self) -> (u8, u8) {
        match self {
            Scale::Multiplier { min, max } | Scale::Percent { min, max } => (*min, *max),
            Scale::Lines { max, .. } => (1, *max),
            Scale::Map(map) => {
                let keys = map.keys().filter_map(|k| k.parse::<u8>().ok());
                (keys.clone().min().unwrap_or(1), keys.max().unwrap_or(1))
            }
        }
    }
}

/// How an emulator's player bindings come to be.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
#[non_exhaustive]
pub enum PlayersSupport {
    /// A Rust adapter writes them: `crates/hermir/src/config/adapters/<adapter>.rs`.
    Adapter {
        adapter: String,
    },
    /// The emulator picks up SDL pads itself, in the order they appear; there is nothing to
    /// write. The note says so in the emulator's terms.
    Automatic {
        automatic: String,
    },
    Unsupported {
        unsupported: String,
    },
}

/// A pad as the consumer sees it: its USB identity and the order it appeared in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PadRef {
    /// The kernel's device name (`Microsoft X-Box 360 pad`), what evdev shows.
    pub name: String,
    /// USB (3) unless the pad says otherwise; part of SDL's GUID.
    #[serde(default = "usb")]
    pub bus: u16,
    pub vendor: u16,
    pub product: u16,
    #[serde(default)]
    pub version: u16,
    /// Position among the pads at launch, 0-based. SDL and evdev number devices that way.
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evdev: Option<PathBuf>,
}

fn usb() -> u16 {
    3
}

/// One seat: player `seat` (1-based) plays on `pad`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Player {
    pub seat: u8,
    pub pad: PadRef,
}

/// What a consumer wants a copy to be for a session. Every field left out means "leave as
/// is"; `apply` says per knob what it did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Patch {
    /// Pads in seat order, into the emulator's bindings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub players: Option<Vec<Player>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<Video>,
    /// The console region the emulator should present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// Keys the model does not cover, written as given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native: Vec<Native>,
}

impl Patch {
    /// What makes the patch impossible to write, before anything is: a seat that is not at
    /// least 1 or appears twice, and a control character (a newline above all) in a pad's name
    /// or in a native key, section or value, where it would end the line and start another.
    pub fn validate(&self) -> Result<(), String> {
        fn text(what: &str, s: &str) -> Result<(), String> {
            match s.chars().find(|c| c.is_control()) {
                Some(c) => Err(format!("{what} {s:?} holds the control character {c:?}")),
                None => Ok(()),
            }
        }
        let mut seats = std::collections::BTreeSet::new();
        for p in self.players.iter().flatten() {
            if p.seat == 0 {
                return Err("seats count from 1".into());
            }
            if !seats.insert(p.seat) {
                return Err(format!("seat {} is given twice", p.seat));
            }
            text("the pad name", &p.pad.name)?;
        }
        for n in &self.native {
            if n.key.trim().is_empty() {
                return Err(format!("a native key of {} has no name", n.file));
            }
            text("the native file", &n.file)?;
            text("the native section", &n.section)?;
            text("the native key", &n.key)?;
            text("the native value", &n.value)?;
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.players.is_none()
            && self.video.as_ref().is_none_or(Video::is_empty)
            && self.region.is_none()
            && self.native.is_empty()
    }
}

/// The display side of a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Video {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fullscreen: Option<bool>,
    /// Internal resolution as a multiple of the console's own, 1 for native.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vsync: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect: Option<Aspect>,
}

impl Video {
    pub fn is_empty(&self) -> bool {
        self.fullscreen.is_none()
            && self.scale.is_none()
            && self.vsync.is_none()
            && self.aspect.is_none()
    }
}

/// How the picture fills the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub enum Aspect {
    /// The game's own.
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "4:3")]
    FourThree,
    #[serde(rename = "16:9")]
    SixteenNine,
    /// Fill the window, whatever the shape.
    #[serde(rename = "stretch")]
    Stretch,
}

impl Aspect {
    /// The neutral spelling, the key into a binding's `values`.
    pub fn as_str(self) -> &'static str {
        match self {
            Aspect::Auto => "auto",
            Aspect::FourThree => "4:3",
            Aspect::SixteenNine => "16:9",
            Aspect::Stretch => "stretch",
        }
    }
}

impl std::str::FromStr for Aspect {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Aspect, String> {
        match s.to_ascii_lowercase().as_str() {
            "auto" | "native" => Ok(Aspect::Auto),
            "4:3" | "4x3" => Ok(Aspect::FourThree),
            "16:9" | "16x9" | "wide" => Ok(Aspect::SixteenNine),
            "stretch" | "fill" => Ok(Aspect::Stretch),
            other => Err(format!(
                "unknown aspect {other}; auto, 4:3, 16:9 or stretch"
            )),
        }
    }
}

/// The console region: what the emulated system reports to the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub enum Region {
    /// The emulator's own choice, usually the game's.
    #[serde(rename = "auto")]
    Auto,
    /// Japan, NTSC-J.
    #[serde(rename = "jp")]
    Japan,
    /// The Americas, NTSC-U.
    #[serde(rename = "us")]
    Usa,
    /// Europe and Australia, PAL.
    #[serde(rename = "eu")]
    Europe,
}

impl Region {
    /// The neutral spelling, the key into a binding's `values`.
    pub fn as_str(self) -> &'static str {
        match self {
            Region::Auto => "auto",
            Region::Japan => "jp",
            Region::Usa => "us",
            Region::Europe => "eu",
        }
    }
}

impl std::str::FromStr for Region {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Region, String> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Region::Auto),
            "jp" | "japan" | "ntsc-j" | "ntscj" => Ok(Region::Japan),
            "us" | "usa" | "ntsc-u" | "ntscu" | "ntsc" => Ok(Region::Usa),
            "eu" | "europe" | "pal" => Ok(Region::Europe),
            other => Err(format!("unknown region {other}; auto, jp, us or eu")),
        }
    }
}

/// A key written as given into a file the catalog names, for what the model does not cover.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Native {
    /// A name from the entry's `config.files`; `main` when left out.
    #[serde(default = "main", skip_serializing_if = "is_main")]
    pub file: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub section: String,
    pub key: String,
    /// The literal, in the file's own spelling (quotes included where it wants them).
    pub value: String,
}

fn main() -> String {
    "main".into()
}

fn is_main(s: &str) -> bool {
    s == "main"
}

/// What `apply` did: one line per knob the patch carried, one step per file it touched.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Applied {
    pub emulator: String,
    pub knobs: Vec<KnobChange>,
    pub steps: Vec<PrepareStep>,
}

impl Applied {
    /// A file could not be written.
    pub fn failed(&self) -> bool {
        self.steps.iter().any(|s| s.outcome == StepOutcome::Failed)
    }
}

/// One knob of a patch, and what became of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KnobChange {
    /// `video.fullscreen`, `video.scale`, `video.vsync`, `video.aspect`, `region`, `players`,
    /// or `native:<file>:<key>`.
    pub knob: String,
    pub support: Support,
    /// Why not, or what to know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The file the knob lands in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

/// One knob as a copy has it now: what `get` reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KnobValue {
    /// `video.fullscreen`, `video.scale`, `video.vsync`, `video.aspect` or `region`.
    pub knob: String,
    /// The neutral value (`true`, `3`, `16:9`, `eu`), when the file holds one hermir can name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// What the file holds, as the emulator spells it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal: Option<String>,
    /// The file the knob lives in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Why there is no value: not set, not described, unsupported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Whether a knob reached the emulator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Support {
    /// Written, as asked.
    Applied,
    /// Written, with the note's caveat.
    Partial,
    /// Not written; the note says why.
    Unsupported,
    /// Meant to be written, and the write failed; the note says why.
    Failed,
}

/// Firmware the emulator reads from a folder under its config root, or installs itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Firmware {
    /// Relative to the config root: `bios`, `system`, `data`, or `.` for the root itself.
    /// Absent when the emulator only takes firmware through its own installer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// The emulator's own installer, for firmware that is not loose files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<FirmwareInstall>,
    /// Firmware that comes as an archive, unpacked into a folder of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpack: Option<FirmwareUnpack>,
    /// Per platform id: what satisfies it.
    #[serde(default)]
    pub platforms: BTreeMap<String, FirmwareNeed>,
}

/// An archive of firmware files (a Switch system update as a `.zip` of NCAs), unpacked once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FirmwareUnpack {
    /// Archive names (`*.zip`). A match is unpacked, never copied into `dir`.
    pub any_of: Vec<String>,
    /// Relative to the config root: where the archive's files go.
    pub into: String,
    /// A name pattern that, found in `into`, means firmware is installed. Nothing is unpacked
    /// over it, so firmware the player installed stays.
    pub present: String,
}

/// Firmware the emulator installs from one file (RPCS3's `--installfw <PUP>`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FirmwareInstall {
    /// The argv after the exe; `{file}` is the firmware file.
    pub args: Vec<String>,
    /// A file under the config root that exists once firmware is installed. It, not the exit
    /// code, says whether the install worked.
    pub done: String,
}

/// One answer to a first-run question, as a file under the config root.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
#[non_exhaustive]
pub enum FirstRun {
    /// `key` in `[section]` of an ini (or flat TOML) file, set to `value` in place. The file and
    /// section are created when missing; every other line stays as it was. `{root}` in `value`
    /// is the config root's absolute path.
    Ini {
        ini: String,
        section: String,
        key: String,
        value: String,
        /// Only when the key is missing or empty: a value the user chose is kept.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        when_unset: bool,
    },
    /// A file written only when it does not exist yet: the emulator's own first-run check.
    Seed { seed: String, content: String },
    /// A folder under the config root that must exist before the emulator writes into it.
    Dir { dir: String },
    /// A file the emulator ships beside its exe, copied under the config root once (data an
    /// emulator refuses to start without). Only a copy hermir placed has an exe to copy from.
    Copy { copy: String, to: String },
}

/// What `prepare` did, step by step. A consumer shows this; it never guesses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Prepared {
    pub emulator: String,
    pub steps: Vec<PrepareStep>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PrepareStep {
    /// `first_run`, `firmware` or `firmware_install`.
    pub kind: String,
    /// The file the step is about.
    pub target: PathBuf,
    pub outcome: StepOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StepOutcome {
    /// Written or installed now.
    Applied,
    /// Already so; nothing was touched.
    Present,
    /// Nothing to do for this emulator; the note says why.
    Skipped,
    /// Changed by someone else since hermir wrote it, so left as it is; the note says what to
    /// do about it.
    Conflict,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FirmwareNeed {
    /// File name patterns (`*.bin`, `scph*.bin`); one match is enough.
    pub any_of: Vec<String>,
    /// Where the files come from, in a phrase a UI can show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The emulator also runs without it (an HLE BIOS); its absence is no failure.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

/// One way to obtain the emulator on one OS.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
#[non_exhaustive]
pub enum Channel {
    /// `flatpak install --user flathub <id>`.
    Flatpak { flatpak: String },
    /// The latest GitHub release of `owner/repo`, one asset of it, extracted into the prefix.
    Github {
        github: String,
        asset: AssetFilter,
        /// Path of the executable inside the extracted tree.
        exe: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        portable: Option<Portable>,
    },
    /// A fixed URL, for projects without GitHub releases. Bumped by catalog PRs.
    Url {
        url: String,
        version: String,
        /// Hex sha256 of the file, bumped with `version` in the same reviewed PR. Validation
        /// rejects a `url` channel without one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha256: Option<String>,
        exe: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        portable: Option<Portable>,
    },
}

impl Channel {
    pub fn kind(&self) -> &'static str {
        match self {
            Channel::Flatpak { .. } => "flatpak",
            Channel::Github { .. } => "github",
            Channel::Url { .. } => "url",
        }
    }
}

/// Which release asset: a fixed name, or substrings that must all appear and none that may.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
#[non_exhaustive]
pub enum AssetFilter {
    Name {
        name: String,
    },
    Filter {
        all: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        none: Vec<String>,
    },
}

impl AssetFilter {
    /// Case-insensitive, as release managers are.
    pub fn matches(&self, asset: &str) -> bool {
        let a = asset.to_ascii_lowercase();
        match self {
            AssetFilter::Name { name } => a == name.to_ascii_lowercase(),
            AssetFilter::Filter { all, none } => {
                all.iter().all(|s| a.contains(&s.to_ascii_lowercase()))
                    && !none.iter().any(|s| a.contains(&s.to_ascii_lowercase()))
            }
        }
    }
}

/// What makes the emulator keep its files beside the exe: a marker file or a directory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
#[non_exhaustive]
pub enum Portable {
    File { file: String },
    Dir { dir: String },
}

/// Detection rules for one OS. Every list is tried; every hit is an `Install`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Detect {
    /// Program names looked up on `PATH`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
    /// A Flathub app id, looked for in the user and system installations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flatpak: Option<String>,
    /// Absolute paths of the executable, with `~`, `%VAR%` and `$VAR`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// A file beside the exe that marks a portable install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portable_marker: Option<String>,
}

/// Config root per install kind.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Roots {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flatpak: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<String>,
}

/// The argv template after the exe. `{file}` is the game, `{fullscreen:-f}` expands to `-f`
/// when fullscreen is asked for. Rendered in P1; carried as data now.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    #[serde(default)]
    pub args: Vec<String>,
}

/// A platform (console) as `catalog/platforms.json` spells it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Platform {
    pub id: String,
    pub name: String,
    /// The same platform in other vocabularies, so a consumer maps once at its edge.
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Emulator ids in order of preference.
    #[serde(default)]
    pub emulators: Vec<String>,
}

/// How an emulator got onto this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum InstallKind {
    /// Ours, in the prefix.
    Managed,
    Flatpak,
    /// Found on `PATH` or at a known install path.
    Native,
    /// Found with a portable marker beside the exe.
    Portable,
}

/// What runs: a file, or a Flatpak app the consumer turns into `flatpak run <id>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Exe {
    Path(PathBuf),
    FlatpakRun(String),
}

impl std::fmt::Display for Exe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Exe::Path(p) => write!(f, "{}", p.display()),
            Exe::FlatpakRun(id) => write!(f, "flatpak run {id}"),
        }
    }
}

/// An emulator that exists on this machine, however it got there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Install {
    pub emulator: String,
    pub kind: InstallKind,
    pub exe: Exe,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the emulator reads its config, when the catalog knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_root: Option<PathBuf>,
}

/// What the bytes that arrived were checked against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Verified {
    /// The sha256 GitHub publishes for the asset matched.
    Published,
    /// The sha256 the catalog pins matched.
    Pinned,
    /// `flatpak` checked it: Flathub signs what it serves.
    Flatpak,
    /// Nothing to check against: `sha256` is what arrived, no more.
    #[default]
    None,
}

impl Install {
    /// A copy of `emulator` that hermir did not find itself: a host that knows where its
    /// emulator lives, or a test. `config_root` is where it reads its settings.
    pub fn new(
        emulator: impl Into<String>,
        kind: InstallKind,
        exe: Exe,
        config_root: Option<PathBuf>,
    ) -> Install {
        Install {
            emulator: emulator.into(),
            kind,
            exe,
            version: None,
            config_root,
        }
    }
}

/// One row of `<prefix>/installed.json`: what hermir installed and from where.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Installed {
    pub emulator: String,
    pub channel: String,
    pub exe: Exe,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The release as the channel identifies it (for GitHub, the tag plus the asset's upload
    /// time, id and digest, so a rolling asset rebuilt in place is a new release); an update is
    /// due when it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    /// Hex sha256 of the archive that arrived; none for a Flatpak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// What `sha256` was checked against. A row from before this field reads as `none`.
    #[serde(default)]
    pub verified: Verified,
    /// Shipped files the user had edited (or made at a shipped path), kept as they were; the
    /// new ones are beside them as `<file>.new`. Relative to the copy's directory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<String>,
    /// RFC 3339, UTC.
    pub installed_at: String,
}

/// Where a channel resolved to, before anything is downloaded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Resolved {
    pub emulator: String,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    /// Bytes, when the channel says; it sets the download's time budget and checks a resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// What an update compares: it changes whenever the bytes behind `url` do.
    pub release: String,
    /// Hex sha256 the download must match: published by GitHub, or pinned by the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// `status` for one emulator: catalog, managed row, detected copies, whether an update exists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Status {
    pub emulator: String,
    pub name: String,
    pub offered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed: Option<Installed>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detected: Vec<Install>,
    /// The release the channel offers now, when it is newer than the managed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
    /// Why the channel could not be asked, when `status` was asked to check it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_filters_are_case_insensitive_and_exclusive() {
        let f = AssetFilter::Filter {
            all: vec!["windows-x64".into(), ".7z".into()],
            none: vec!["installer".into()],
        };
        assert!(f.matches("pcsx2-v2.8.2-Windows-x64-Qt.7z"));
        assert!(!f.matches("pcsx2-v2.8.2-windows-x64-installer.7z"));
        assert!(!f.matches("pcsx2-v2.8.2-linux-appimage-x64-Qt.AppImage"));
        let n = AssetFilter::Name {
            name: "windows-latest.zip".into(),
        };
        assert!(n.matches("Windows-Latest.zip"));
        assert!(!n.matches("windows-latest.zip.sha256"));
    }

    #[test]
    fn knobs_deserialize_by_shape_and_scales_render() {
        let k: Knob = serde_json::from_str(r#"{"unsupported":"no such setting"}"#).unwrap();
        assert!(matches!(k, Knob::Unsupported { .. }));
        let k: Knob = serde_json::from_str(
            r#"{"file":"main","section":"EmuCore/GS","key":"upscale_multiplier","scale":{"multiplier":{"max":8}}}"#,
        )
        .unwrap();
        let Knob::Bound(b) = k else { panic!() };
        assert_eq!(b.scale.as_ref().unwrap().render(3).as_deref(), Some("3"));
        assert_eq!(b.scale.as_ref().unwrap().render(9), None);
        assert!(serde_json::from_str::<Knob>(r#"{"file":"main","key":"k","nope":1}"#).is_err());
        let s: Scale = serde_json::from_str(r#"{"percent":{}}"#).unwrap();
        assert_eq!(s.render(2).as_deref(), Some("200"));
        let s: Scale = serde_json::from_str(r#"{"lines":{"base":480}}"#).unwrap();
        assert_eq!(s.render(2).as_deref(), Some("960"));
        let s: Scale = serde_json::from_str(r#"{"map":{"1":"2","2":"4"}}"#).unwrap();
        assert_eq!(s.render(2).as_deref(), Some("4"));
        assert_eq!(s.render(3), None);
        assert_eq!(s.range(), (1, 2));
        let p: FilePath =
            serde_json::from_str(r#"{"linux":"Dolphin.ini","windows":"Config/Dolphin.ini"}"#)
                .unwrap();
        assert_eq!(p.get(Os::Windows), Some("Config/Dolphin.ini"));
        assert_eq!(p.get(Os::Macos), None);
    }

    #[test]
    fn patch_spellings_are_the_cli_ones() {
        let p: Patch = serde_json::from_str(
            r#"{"video":{"fullscreen":true,"scale":3,"aspect":"16:9"},"region":"eu","native":[{"key":"k","value":"v"}]}"#,
        )
        .unwrap();
        assert_eq!(p.video.unwrap().aspect, Some(Aspect::SixteenNine));
        assert_eq!(p.region, Some(Region::Europe));
        assert_eq!(p.native[0].file, "main");
        assert_eq!(serde_json::to_string(&Region::Usa).unwrap(), "\"us\"");
        assert_eq!("PAL".parse::<Region>(), Ok(Region::Europe));
        assert_eq!("4x3".parse::<Aspect>(), Ok(Aspect::FourThree));
        assert!(Patch::default().is_empty());
    }

    #[test]
    fn channels_deserialize_by_shape() {
        let c: Channel = serde_json::from_str(r#"{"flatpak":"net.pcsx2.PCSX2"}"#).unwrap();
        assert_eq!(c.kind(), "flatpak");
        let c: Channel = serde_json::from_str(
            r#"{"github":"PCSX2/pcsx2","asset":{"all":["x64"]},"exe":"pcsx2-qt.exe","portable":{"file":"portable.ini"}}"#,
        )
        .unwrap();
        assert_eq!(c.kind(), "github");
        assert!(serde_json::from_str::<Channel>(r#"{"github":"x","exe":"y"}"#).is_err());
    }
}
