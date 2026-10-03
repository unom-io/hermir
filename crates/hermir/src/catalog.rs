//! The catalog: every emulator hermir knows, embedded at build time from `catalog/` and
//! loadable from a directory for validation. Parsing is strict (unknown keys fail) and
//! `validate` holds the rules a schema cannot: ids match file names, platforms exist, and a
//! Switch emulator never carries an install channel.
use std::collections::BTreeMap;
use std::path::Path;

use crate::channel::extract::is_safe_relative;
use crate::config::adapters;
use crate::error::{Error, Result};
use crate::model::{
    Channel, Config, Entry, FirstRun, Format, KNOBS, Knob, Os, Platform, PlayersSupport, Scale,
};

/// `(id, json)` for every file under `catalog/emulators/`. A test checks the directory listing
/// against this list, so a new file cannot be forgotten.
const EMULATORS: &[(&str, &str)] = &[
    ("ares", include_str!("../catalog/emulators/ares.json")),
    ("azahar", include_str!("../catalog/emulators/azahar.json")),
    ("cemu", include_str!("../catalog/emulators/cemu.json")),
    ("dolphin", include_str!("../catalog/emulators/dolphin.json")),
    (
        "dosbox-staging",
        include_str!("../catalog/emulators/dosbox-staging.json"),
    ),
    (
        "duckstation",
        include_str!("../catalog/emulators/duckstation.json"),
    ),
    ("eden", include_str!("../catalog/emulators/eden.json")),
    ("flycast", include_str!("../catalog/emulators/flycast.json")),
    ("melonds", include_str!("../catalog/emulators/melonds.json")),
    ("mgba", include_str!("../catalog/emulators/mgba.json")),
    ("pcsx2", include_str!("../catalog/emulators/pcsx2.json")),
    ("ppsspp", include_str!("../catalog/emulators/ppsspp.json")),
    (
        "retroarch",
        include_str!("../catalog/emulators/retroarch.json"),
    ),
    ("rmg", include_str!("../catalog/emulators/rmg.json")),
    ("rpcs3", include_str!("../catalog/emulators/rpcs3.json")),
    ("ryujinx", include_str!("../catalog/emulators/ryujinx.json")),
    ("scummvm", include_str!("../catalog/emulators/scummvm.json")),
    ("shadps4", include_str!("../catalog/emulators/shadps4.json")),
    ("snes9x", include_str!("../catalog/emulators/snes9x.json")),
    (
        "supermodel",
        include_str!("../catalog/emulators/supermodel.json"),
    ),
    ("vita3k", include_str!("../catalog/emulators/vita3k.json")),
    ("xemu", include_str!("../catalog/emulators/xemu.json")),
    (
        "xenia-canary",
        include_str!("../catalog/emulators/xenia-canary.json"),
    ),
];
const PLATFORMS: &str = include_str!("../catalog/platforms.json");

/// Platforms whose emulators are never installed by hermir (D12).
const NO_INSTALL_PLATFORMS: &[&str] = &["switch"];

#[derive(Clone, Debug)]
/// Every emulator and platform hermir knows: the entries under `catalog/`, embedded at build
/// time, parsed strictly and checked against the rules a schema cannot express.
pub struct Catalog {
    entries: Vec<Entry>,
    platforms: Vec<Platform>,
}

impl Catalog {
    /// The catalog this binary was built with.
    pub fn embedded() -> Result<Catalog> {
        let mut entries = Vec::with_capacity(EMULATORS.len());
        for (id, json) in EMULATORS {
            entries.push(parse_entry(id, json)?);
        }
        let platforms = serde_json::from_str(PLATFORMS).map_err(|e| Error::Catalog {
            entry: "platforms.json".into(),
            why: e.to_string(),
        })?;
        let c = Catalog { entries, platforms };
        c.validate()?;
        Ok(c)
    }

    /// A catalog directory on disk (`emulators/*.json` + `platforms.json`), for validating a
    /// change before it is embedded.
    pub fn from_dir(dir: &Path) -> Result<Catalog> {
        let emus = dir.join("emulators");
        let mut entries = Vec::new();
        let rd = std::fs::read_dir(&emus).map_err(|e| Error::io("read", &emus, e))?;
        let mut files: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        files.sort();
        for path in files
            .iter()
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
        {
            let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let json = std::fs::read_to_string(path).map_err(|e| Error::io("read", path, e))?;
            entries.push(parse_entry(id, &json)?);
        }
        let pj = dir.join("platforms.json");
        let json = std::fs::read_to_string(&pj).map_err(|e| Error::io("read", &pj, e))?;
        let platforms = serde_json::from_str(&json).map_err(|e| Error::Catalog {
            entry: "platforms.json".into(),
            why: e.to_string(),
        })?;
        let c = Catalog { entries, platforms };
        c.validate()?;
        Ok(c)
    }

    /// Every entry, in id order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry with this id.
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Every platform, with its aliases and its emulators in order of preference.
    pub fn platforms(&self) -> &[Platform] {
        &self.platforms
    }

    /// The platform with this id.
    pub fn platform(&self, id: &str) -> Option<&Platform> {
        self.platforms.iter().find(|p| p.id == id)
    }

    /// Entries with an install channel on `os`.
    pub fn offered_on(&self, os: Os) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(move |e| e.channels.get(os).is_some())
    }

    /// The JSON Schema of one emulator entry, for consumers and editors.
    pub fn entry_schema() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(Entry)).unwrap_or_default()
    }

    fn validate(&self) -> Result<()> {
        let bad = |entry: &str, why: String| Error::Catalog {
            entry: entry.into(),
            why,
        };
        let mut seen = std::collections::BTreeSet::new();
        for e in &self.entries {
            if !seen.insert(&e.id) {
                return Err(bad(&e.id, "duplicate id".into()));
            }
            if e.id.is_empty()
                || !e
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                return Err(bad(&e.id, "id must be [a-z0-9-]".into()));
            }
            if e.platforms.is_empty() {
                return Err(bad(&e.id, "no platforms".into()));
            }
            for p in &e.platforms {
                if self.platform(p).is_none() {
                    return Err(bad(&e.id, format!("unknown platform {p}")));
                }
            }
            let has_channel = [Os::Linux, Os::Windows, Os::Macos]
                .iter()
                .any(|&os| e.channels.get(os).is_some());
            let banned = e
                .platforms
                .iter()
                .find(|p| NO_INSTALL_PLATFORMS.contains(&p.as_str()));
            if let (Some(p), true) = (banned, has_channel) {
                return Err(bad(
                    &e.id,
                    format!("{p} emulators carry no install channel"),
                ));
            }
            if let Some(fw) = &e.firmware {
                for p in fw.platforms.keys() {
                    if !e.platforms.contains(p) {
                        return Err(bad(
                            &e.id,
                            format!("firmware for {p}, a platform it does not play"),
                        ));
                    }
                }
                if fw.dir.is_none() && fw.install.is_none() {
                    return Err(bad(&e.id, "firmware with neither dir nor install".into()));
                }
            }
            // Everything prepare writes stays under the config root.
            let written = e
                .firmware
                .iter()
                .flat_map(|f| {
                    f.dir
                        .iter()
                        .chain(f.install.iter().map(|i| &i.done))
                        .chain(f.unpack.iter().map(|u| &u.into))
                })
                .chain(
                    [Os::Linux, Os::Windows, Os::Macos]
                        .iter()
                        .flat_map(|&os| e.first_run.get(os).into_iter().flatten())
                        .flat_map(|a| match a {
                            FirstRun::Ini { ini, .. } => vec![ini],
                            FirstRun::Seed { seed, .. } => vec![seed],
                            FirstRun::Dir { dir } => vec![dir],
                            // What is copied comes from beside the exe, so it stays in there too.
                            FirstRun::Copy { copy, to } => vec![copy, to],
                        }),
                );
            for rel in written {
                if rel.is_empty() || !is_safe_relative(Path::new(rel)) {
                    return Err(bad(
                        &e.id,
                        format!("{rel} is not a path under the config root"),
                    ));
                }
            }
            if let Some(cfg) = &e.config {
                validate_config(&e.id, cfg)?;
            }
            crate::launch::validate(e).map_err(|why| bad(&e.id, why))?;
            // A `url` channel pins the file's checksum beside its version.
            for os in [Os::Linux, Os::Windows, Os::Macos] {
                if let Some(Channel::Url { sha256, .. }) = e.channels.get(os) {
                    match sha256 {
                        Some(h) if h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()) => {}
                        Some(h) => {
                            return Err(bad(
                                &e.id,
                                format!("{os}: sha256 {h} is not 64 hex digits"),
                            ));
                        }
                        None => {
                            return Err(bad(&e.id, format!("{os}: a url channel pins its sha256")));
                        }
                    }
                }
            }
            if e.no_install.is_some() && has_channel {
                return Err(bad(
                    &e.id,
                    "no_install set, yet a channel is present".into(),
                ));
            }
            if !has_channel && e.no_install.is_none() {
                return Err(bad(&e.id, "no channel and no no_install reason".into()));
            }
        }
        for p in &self.platforms {
            for id in &p.emulators {
                if self.get(id).is_none() {
                    return Err(bad(&p.id, format!("unknown emulator {id}")));
                }
            }
        }
        Ok(())
    }
}

/// The rules of a `config` block: files are under the root, knobs are the known ones and
/// carry the one spelling their type takes, every file named exists, every adapter has code.
fn validate_config(id: &str, cfg: &Config) -> Result<()> {
    let bad = |why: String| Error::Catalog {
        entry: id.into(),
        why,
    };
    for (name, f) in &cfg.files {
        for p in f.path.all() {
            let rel = p
                .strip_prefix("{config}/")
                .or_else(|| p.strip_prefix("{config}\\"))
                .unwrap_or(p);
            if rel.is_empty() || !is_safe_relative(Path::new(rel)) {
                return Err(bad(format!(
                    "file {name}: {p} is not under the config root"
                )));
            }
        }
        match (f.format, &f.root) {
            (Format::Xml, None) => {
                return Err(bad(format!("file {name}: xml needs its root element")));
            }
            (Format::Xml, Some(_)) | (_, None) => {}
            (_, Some(_)) => return Err(bad(format!("file {name}: only xml has a root"))),
        }
    }
    let file_known = |name: &str| cfg.files.contains_key(name);
    for (knob, k) in &cfg.knobs {
        if !KNOBS.contains(&knob.as_str()) {
            return Err(bad(format!(
                "unknown knob {knob}; one of {}",
                KNOBS.join(", ")
            )));
        }
        let Knob::Bound(b) = k else { continue };
        if !file_known(&b.file) {
            return Err(bad(format!("{knob}: no file named {}", b.file)));
        }
        let (want_bool, want_scale, want_values) = match knob.as_str() {
            "video.fullscreen" | "video.vsync" => (true, false, false),
            "video.scale" => (false, true, false),
            _ => (false, false, true),
        };
        if b.bool.is_some() != want_bool
            || b.scale.is_some() != want_scale
            || b.values.is_some() != want_values
        {
            let want = if want_bool {
                "bool"
            } else if want_scale {
                "scale"
            } else {
                "values"
            };
            return Err(bad(format!("{knob} takes `{want}` and nothing else")));
        }
        if let Some(values) = &b.values {
            let allowed = crate::config::neutral_values(knob);
            if values.is_empty() {
                return Err(bad(format!("{knob}: empty values")));
            }
            for v in values.keys() {
                if !allowed.contains(&v.as_str()) {
                    return Err(bad(format!(
                        "{knob}: {v} is not one of {}",
                        allowed.join(", ")
                    )));
                }
            }
        }
        if let Some(scale) = &b.scale {
            let (min, max) = scale.range();
            let sane = match scale {
                Scale::Lines { base, .. } => *base > 0 && max >= 1,
                Scale::Map(m) => {
                    !m.is_empty() && m.keys().all(|k| k.parse::<u8>().is_ok_and(|n| n >= 1))
                }
                _ => min >= 1 && max >= min,
            };
            if !sane || max > 16 {
                return Err(bad(format!("{knob}: a scale runs from 1× to at most 16×")));
            }
        }
        for a in &b.also {
            if a.file.as_deref().is_some_and(|f| !file_known(f)) {
                return Err(bad(format!("{knob}: also {}: no such file", a.key)));
            }
            let spellings = [a.value.is_some(), a.bool.is_some(), a.values.is_some()];
            if spellings.iter().filter(|&&x| x).count() != 1 {
                return Err(bad(format!(
                    "{knob}: also {}: exactly one of value, bool, values",
                    a.key
                )));
            }
            if a.values.as_ref().is_some_and(BTreeMap::is_empty) {
                return Err(bad(format!("{knob}: also {}: empty values", a.key)));
            }
        }
    }
    if let Some(PlayersSupport::Adapter { adapter }) = &cfg.players
        && !adapters::exists(adapter)
    {
        return Err(bad(format!(
            "players adapter {adapter} has no code; one of {}",
            adapters::ADAPTERS
                .iter()
                .map(|a| a.name)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(())
}

fn parse_entry(id: &str, json: &str) -> Result<Entry> {
    let e: Entry = serde_json::from_str(json).map_err(|e| Error::Catalog {
        entry: id.into(),
        why: e.to_string(),
    })?;
    if e.id != id {
        return Err(Error::Catalog {
            entry: id.into(),
            why: format!("id {} does not match the file name", e.id),
        });
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_is_valid() {
        let c = Catalog::embedded().unwrap();
        assert!(c.get("pcsx2").is_some());
        assert!(c.offered_on(Os::Linux).count() > 10);
    }

    #[test]
    fn every_catalog_file_is_embedded() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog/emulators");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .trim_end_matches(".json")
                    .to_string()
            })
            .collect();
        on_disk.sort();
        let embedded: Vec<String> = EMULATORS.iter().map(|(id, _)| id.to_string()).collect();
        assert_eq!(on_disk, embedded, "add the file to EMULATORS in catalog.rs");
    }

    #[test]
    fn from_dir_matches_embedded() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog");
        let c = Catalog::from_dir(&dir).unwrap();
        assert_eq!(
            c.entries().len(),
            Catalog::embedded().unwrap().entries().len()
        );
    }

    #[test]
    fn switch_entries_never_carry_a_channel() {
        let c = Catalog::embedded().unwrap();
        for e in c
            .entries()
            .iter()
            .filter(|e| e.platforms.iter().any(|p| p == "switch"))
        {
            assert!(e.channels.get(Os::Linux).is_none() && e.channels.get(Os::Windows).is_none());
            assert!(e.no_install.is_some());
        }
    }

    #[test]
    fn a_url_channel_pins_its_sha256() {
        let mut c = Catalog::embedded().unwrap();
        let e = c.entries.iter_mut().find(|e| e.id == "dolphin").unwrap();
        let Some(Channel::Url { sha256, .. }) = e.channels.windows.as_mut() else {
            panic!("dolphin's windows channel is a url");
        };
        *sha256 = None;
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("pins its sha256"), "{err}");

        let e = c.entries.iter_mut().find(|e| e.id == "dolphin").unwrap();
        let Some(Channel::Url { sha256, .. }) = e.channels.windows.as_mut() else {
            unreachable!()
        };
        *sha256 = Some("abc".into());
        assert!(c.validate().is_err());
    }

    #[test]
    fn firmware_unpacks_under_the_config_root() {
        let mut c = Catalog::embedded().unwrap();
        let e = c.entries.iter_mut().find(|e| e.id == "eden").unwrap();
        let unpack = e.firmware.as_mut().unwrap().unpack.as_mut().unwrap();
        unpack.into = "../../.ssh".into();
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("not a path under the config root"), "{err}");
    }

    #[test]
    fn schema_is_generated() {
        let s = Catalog::entry_schema();
        assert!(s.get("properties").is_some());
    }

    #[test]
    fn schema_file_is_current() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog/schema/entry.schema.json");
        let on_disk: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            on_disk,
            Catalog::entry_schema(),
            "regenerate with: hermir catalog schema > crates/hermir/catalog/schema/entry.schema.json"
        );
    }
}
