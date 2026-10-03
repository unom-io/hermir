//! A fixture: what an emulator wrote on its first start (`before/`), how it was captured
//! (`meta.json`), the sessions hermir applies to it (`sessions/`), what hermir wrote for each
//! (`hermir/<session>/`) and, where a person captured it, what the emulator itself wrote when the
//! same settings were set in its UI (`after/<session>/`). `fixtures/README.md` is the long form.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hermir::{InstallKind, Os, Patch, Support};
use serde::{Deserialize, Serialize};

use crate::oracle::Oracle;

/// `fixtures/<emulator>/<version>/<os>/meta.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub emulator: String,
    /// The version the emulator reported, as its channel spells it.
    pub version: String,
    pub os: Os,
    /// How the copy that wrote these files was installed; `apply` is run as that kind.
    pub kind: InstallKind,
    pub source: Source,
    pub captured: Captured,
    /// Catalog file name → path under the config root, for the files the first start wrote.
    pub files: BTreeMap<String, String>,
    /// Catalog file names the first start did not write; hermir creates them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Catalog file name → how to read it.
    pub oracle: BTreeMap<String, Oracle>,
    /// Values replaced at capture time because they differ on every first start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<Mask>,
    /// `section/key` paths a UI session also changes besides the knob (window geometry).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore_after: Vec<String>,
    /// The emulator writes every setting on its first start, not only the changed ones, so every
    /// key the catalog binds must already be in `before/`.
    #[serde(default)]
    pub full_writer: bool,
    /// Every file the first start created, relative to the emulator's home
    /// (`~/.var/app/<id>/` for a Flatpak).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flatpak: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Captured {
    /// `YYYY-MM-DD`.
    pub on: String,
    /// `xtask capture`, `xtask capture --interactive`, or `hand` for a synthetic edge case.
    pub by: String,
}

/// A value replaced by `masked` at capture time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mask {
    /// A catalog file name.
    pub file: String,
    pub section: String,
    pub key: String,
}

/// `sessions/<name>.json`: one `Patch`, and what `apply` should say about it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub patch: Patch,
    /// Knob → the support `apply` reports. Written by `HERMIR_BLESS=1`, reviewed in the PR.
    #[serde(default)]
    pub expect: BTreeMap<String, Support>,
    /// What an adapter owns besides the catalog's keys: `<file>` for a whole file, or
    /// `<file>:<section>` (file as a path under the config root). Players sessions only;
    /// `HERMIR_BLESS=1` fills it once when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_touch: Option<Vec<String>>,
}

/// One fixture directory.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub dir: PathBuf,
    pub meta: Meta,
    /// Session names, sorted.
    pub sessions: Vec<String>,
}

impl Fixture {
    pub fn load(dir: &Path) -> Result<Fixture, String> {
        let meta_path = dir.join("meta.json");
        let meta: Meta = serde_json::from_str(&read(&meta_path)?)
            .map_err(|e| format!("{}: {e}", meta_path.display()))?;
        let mut sessions = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir.join("sessions")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some(stem) = name.strip_suffix(".json") {
                    sessions.push(stem.to_string());
                }
            }
        }
        sessions.sort();
        Ok(Fixture {
            dir: dir.to_path_buf(),
            meta,
            sessions,
        })
    }

    /// `pcsx2/2.8.2/linux`.
    pub fn name(&self) -> String {
        format!(
            "{}/{}/{}",
            self.meta.emulator,
            self.meta.version,
            self.meta.os.as_str()
        )
    }

    pub fn session(&self, name: &str) -> Result<Session, String> {
        let p = self.session_path(name);
        serde_json::from_str(&read(&p)?).map_err(|e| format!("{}: {e}", p.display()))
    }

    pub fn session_path(&self, name: &str) -> PathBuf {
        self.dir.join("sessions").join(format!("{name}.json"))
    }
}

/// Every `fixtures/<emulator>/<version>/<os>/` under `root`, sorted.
pub fn discover(root: &Path) -> Result<Vec<Fixture>, String> {
    let mut dirs = Vec::new();
    for emu in subdirs(root) {
        for version in subdirs(&emu) {
            for os in subdirs(&version) {
                if os.join("meta.json").is_file() {
                    dirs.push(os);
                }
            }
        }
    }
    dirs.sort();
    dirs.iter().map(|d| Fixture::load(d)).collect()
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// The sessions every new fixture starts with.
pub fn standard_sessions() -> Vec<(&'static str, Session)> {
    let s = |json: serde_json::Value| Session {
        patch: serde_json::from_value(json).expect("a standard session is a Patch"),
        ..Default::default()
    };
    let xbox = |index: u32| {
        serde_json::json!({ "name": "Microsoft X-Box 360 pad", "vendor": 0x045e, "product": 0x028e,
                            "version": 0x0110, "index": index })
    };
    // Through SDL's HIDAPI driver, as SDL lists it: a GUID nothing in the USB identity gives.
    let dualsense = |index: u32| {
        serde_json::json!({ "name": "Sony Interactive Entertainment DualSense Wireless Controller",
                            "vendor": 0x054c, "product": 0x0ce6, "version": 0x8111,
                            "index": index, "guid": "030057564c050000e60c000000016800",
                            "gamepad_name": "DualSense Wireless Controller" })
    };
    vec![
        (
            "video-all",
            s(
                serde_json::json!({ "video": { "fullscreen": true, "scale": 3, "vsync": true,
                                             "aspect": "16:9" } }),
            ),
        ),
        (
            "video-off",
            s(
                serde_json::json!({ "video": { "fullscreen": false, "scale": 1, "vsync": false,
                                             "aspect": "auto" } }),
            ),
        ),
        ("region-eu", s(serde_json::json!({ "region": "eu" }))),
        (
            "audio-device",
            s(serde_json::json!({ "audio": { "device": "hermir_test_sink" } })),
        ),
        (
            "audio-latency",
            s(serde_json::json!({ "audio": { "latency_ms": 64 } })),
        ),
        (
            "players-1-xbox",
            s(serde_json::json!({ "players": [ { "seat": 1, "pad": xbox(0) } ] })),
        ),
        (
            "players-2-xbox",
            s(
                serde_json::json!({ "players": [ { "seat": 1, "pad": xbox(0) },
                                               { "seat": 2, "pad": xbox(1) } ] }),
            ),
        ),
        // The seats in another order than SDL's: the Xbox pad, second in SDL's list, is
        // player 1, and the DualSense before it player 2.
        (
            "players-2-mixed",
            s(
                serde_json::json!({ "players": [ { "seat": 1, "pad": xbox(1) },
                                               { "seat": 2, "pad": dualsense(0) } ],
                                    "connected": [ dualsense(0), xbox(1) ] }),
            ),
        ),
    ]
}

pub(crate) fn read(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}
