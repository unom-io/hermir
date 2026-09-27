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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Firmware the emulator reads from one folder under its config root.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Firmware {
    /// Relative to the config root: `bios`, `system`, `data`, or `.` for the root itself.
    pub dir: String,
    /// Per platform id: what satisfies it.
    #[serde(default)]
    pub platforms: BTreeMap<String, FirmwareNeed>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FirmwareNeed {
    /// File name patterns (`*.bin`, `scph*.bin`); one match is enough.
    pub any_of: Vec<String>,
    /// Where the files come from, in a phrase a UI can show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One way to obtain the emulator on one OS.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
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
        /// Hex sha256 of the file when the project publishes one; otherwise hermir records the
        /// digest it saw and says so.
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

/// One row of `<prefix>/installed.json`: what hermir installed and from where.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Installed {
    pub emulator: String,
    pub channel: String,
    pub exe: Exe,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The release as the channel identifies it (tag, or tag plus publish date for rolling
    /// tags); an update is due when it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub release: String,
    /// Hex sha256 when the channel publishes one.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
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
