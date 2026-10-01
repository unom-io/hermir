//! The transaction under every write: a file is snapshotted before its first edit, the edits
//! of one file are applied to its text in order, and the result lands atomically. `revert`
//! puts every snapshotted file back byte for byte and forgets the snapshot. The first snapshot
//! of a file is the one that stays: a second `apply` writes over hermir's own text, not the
//! player's, and one `revert` undoes the whole session.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{ini, json, xml, yaml};
use crate::model::{ConfigFile, Format, PrepareStep, StepOutcome};
use crate::prepare::write_atomic;

/// One change to one file.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Edit {
    /// `key` in `section` set to `value`, as `format` spells it. For XML the section is the
    /// element path from the document element; for JSON the object under the root, or none.
    Set {
        file: PathBuf,
        format: Format,
        section: String,
        key: String,
        value: String,
    },
    /// The whole file, ours.
    Whole { file: PathBuf, content: String },
}

impl Edit {
    pub fn file(&self) -> &Path {
        match self {
            Edit::Set { file, .. } | Edit::Whole { file, .. } => file,
        }
    }

    /// `key` in `section` of the catalog's `file` at `path` set to `value`: one edit, or two
    /// for Qt's ini, whose `key\default=false` companion must say the value is deliberate.
    pub fn set(path: &Path, file: &ConfigFile, section: &str, key: &str, value: &str) -> Vec<Edit> {
        let section = match (file.format, file.root.as_deref()) {
            (Format::Xml, Some(root)) if section.is_empty() => root.to_string(),
            (Format::Xml, Some(root)) => format!("{root}/{section}"),
            _ => section.to_string(),
        };
        let one = |key: String, value: String| Edit::Set {
            file: path.to_path_buf(),
            format: file.format,
            section: section.clone(),
            key,
            value,
        };
        let mut out = vec![one(key.into(), value.into())];
        if file.format == Format::Qt {
            out.push(one(format!("{key}\\default"), "false".into()));
        }
        out
    }

    /// `text` after this edit; `Ok(None)` when it already holds it.
    fn patch(&self, text: &str) -> Result<Option<String>, String> {
        match self {
            Edit::Whole { content, .. } => Ok((text != content).then(|| content.clone())),
            Edit::Set {
                format,
                section,
                key,
                value,
                ..
            } => match format {
                Format::Ini => Ok(ini::set(text, section, key, value)),
                // Qt spells `key=value`; a file Qt wrote says so itself, an empty one cannot.
                Format::Qt => Ok(ini::set_with(text, section, key, value, "=")),
                Format::Yaml => yaml::set(text, section, key, value, false),
                Format::Bml => yaml::set(text, section, key, value, true),
                Format::Xml => {
                    let path: Vec<&str> = section
                        .split('/')
                        .filter(|s| !s.is_empty())
                        .chain(std::iter::once(key.as_str()))
                        .collect();
                    xml::set(text, &path, value)
                }
                Format::Json => json::set(text, section, key, value),
            },
        }
    }
}

/// Applies `edits` file by file, each file snapshotted under `snapshots/<entry_id>/` unless it
/// already is. One step per file: applied, present (nothing changed) or failed.
pub(crate) fn apply_edits(snapshots: &Path, entry_id: &str, edits: &[Edit]) -> Vec<PrepareStep> {
    let mut steps = Vec::new();
    let mut snap = match Snapshot::open(snapshots, entry_id) {
        Ok(s) => s,
        Err(e) => {
            steps.push(step("config", snapshots, StepOutcome::Failed, Some(e)));
            return steps;
        }
    };
    let mut files: Vec<&Path> = Vec::new();
    for e in edits {
        if !files.contains(&e.file()) {
            files.push(e.file());
        }
    }
    'files: for file in files {
        if let Err(e) = snap.keep(file) {
            steps.push(step("config", file, StepOutcome::Failed, Some(e)));
            continue;
        }
        let before = match std::fs::read_to_string(file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                steps.push(step(
                    "config",
                    file,
                    StepOutcome::Failed,
                    Some(e.to_string()),
                ));
                continue;
            }
        };
        // A byte-order mark stays where it was and never reaches the editors.
        let (bom, body) = match before.strip_prefix('\u{feff}') {
            Some(b) => ("\u{feff}", b),
            None => ("", before.as_str()),
        };
        let mut text = body.to_string();
        for e in edits.iter().filter(|e| e.file() == file) {
            match e.patch(&text) {
                Ok(Some(next)) => text = next,
                Ok(None) => {}
                Err(why) => {
                    steps.push(step("config", file, StepOutcome::Failed, Some(why)));
                    continue 'files;
                }
            }
        }
        if text == body {
            steps.push(step("config", file, StepOutcome::Present, None));
            continue;
        }
        let written = write_atomic(file, format!("{bom}{text}").as_bytes());
        steps.push(match written {
            Ok(()) => step("config", file, StepOutcome::Applied, None),
            Err(e) => step("config", file, StepOutcome::Failed, Some(e.to_string())),
        });
    }
    steps
}

/// Puts back every file `apply` snapshotted for `entry_id`, then forgets the snapshot. One
/// step per file; none when nothing was outstanding.
pub fn revert(snapshots: &Path, entry_id: &str) -> Vec<PrepareStep> {
    let snap = match Snapshot::open(snapshots, entry_id) {
        Ok(s) => s,
        Err(e) => return vec![step("revert", snapshots, StepOutcome::Failed, Some(e))],
    };
    let mut steps = Vec::new();
    for (file, kept) in &snap.manifest {
        let file = Path::new(file);
        let restored = match kept {
            Some(name) => std::fs::read(snap.dir.join("files").join(name))
                .and_then(|bytes| write_atomic(file, &bytes)),
            None => match std::fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        };
        steps.push(match restored {
            Ok(()) => step("revert", file, StepOutcome::Applied, None),
            Err(e) => step("revert", file, StepOutcome::Failed, Some(e.to_string())),
        });
    }
    if steps.iter().all(|s| s.outcome != StepOutcome::Failed) {
        let _ = std::fs::remove_dir_all(&snap.dir);
    }
    steps
}

/// Emulators with a snapshot outstanding.
pub fn outstanding(snapshots: &Path) -> Vec<String> {
    std::fs::read_dir(snapshots)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().join("manifest.json").is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// `<snapshots>/<emulator>/manifest.json`: file → the copy of it under `files/`, or `None`
/// for a file that did not exist.
struct Snapshot {
    dir: PathBuf,
    manifest: BTreeMap<String, Option<String>>,
}

impl Snapshot {
    fn open(snapshots: &Path, entry_id: &str) -> Result<Snapshot, String> {
        let dir = snapshots.join(entry_id);
        let path = dir.join("manifest.json");
        let manifest = match std::fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        Ok(Snapshot { dir, manifest })
    }

    fn keep(&mut self, file: &Path) -> Result<(), String> {
        let key = file.to_string_lossy().into_owned();
        if self.manifest.contains_key(&key) {
            return Ok(());
        }
        let files = self.dir.join("files");
        std::fs::create_dir_all(&files).map_err(|e| format!("{}: {e}", files.display()))?;
        let kept = match std::fs::read(file) {
            Ok(bytes) => {
                let name = format!("{}.bak", self.manifest.len());
                let at = files.join(&name);
                std::fs::write(&at, bytes).map_err(|e| format!("{}: {e}", at.display()))?;
                Some(name)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", file.display())),
        };
        self.manifest.insert(key, kept);
        let path = self.dir.join("manifest.json");
        let json = serde_json::to_vec_pretty(&self.manifest).map_err(|e| e.to_string())?;
        write_atomic(&path, &json).map_err(|e| format!("{}: {e}", path.display()))
    }
}

fn step(kind: &str, target: &Path, outcome: StepOutcome, note: Option<String>) -> PrepareStep {
    PrepareStep {
        kind: kind.into(),
        target: target.to_path_buf(),
        outcome,
        note,
    }
}
