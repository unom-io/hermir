//! The transaction under every write: a file is snapshotted before its first edit, the edits
//! of one file are applied to its text in order, and the result lands atomically. `revert`
//! puts every snapshotted file back byte for byte and forgets the snapshot. The first snapshot
//! of a file is the one that stays: a second `apply` writes over hermir's own text, not the
//! player's, and one `revert` undoes the whole session.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{ini, json, xml, yaml};
use crate::model::{ConfigFile, Format, PrepareStep, StepOutcome};

/// One change to one file.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Edit {
    /// `key` in `section` set to `value`, as `format` spells it. For XML the section is the
    /// element path from the document element; for JSON the object under the root, or none.
    Set {
        file: PathBuf,
        format: Format,
        /// INI section and key names match whatever their case.
        case_insensitive: bool,
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
            case_insensitive: file.case_insensitive,
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
                case_insensitive,
                section,
                key,
                value,
                ..
            } => match format {
                Format::Ini => {
                    let opts = ini::IniOpts {
                        case_insensitive: *case_insensitive,
                        ..ini::IniOpts::default()
                    };
                    ini::set_with(text, section, key, value, opts)
                }
                // Qt spells `key=value`; a file Qt wrote says so itself, an empty one cannot.
                Format::Qt => {
                    let opts = ini::IniOpts {
                        sep: "=",
                        case_insensitive: *case_insensitive,
                    };
                    ini::set_with(text, section, key, value, opts)
                }
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

/// What `key` in `section` of `file` (the catalog's description of the file at `path`) holds,
/// as the file spells it; `None` when the file or the key is not there.
pub(crate) fn read(
    path: &Path,
    file: &ConfigFile,
    section: &str,
    key: &str,
) -> Result<Option<String>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let opts = |sep| ini::IniOpts {
        sep,
        case_insensitive: file.case_insensitive,
    };
    Ok(match file.format {
        Format::Ini => ini::get_with(text, section, key, opts(" = ")),
        Format::Qt => ini::get_with(text, section, key, opts("=")),
        Format::Yaml => yaml::get(text, section, key, false),
        Format::Bml => yaml::get(text, section, key, true),
        Format::Xml => {
            let mut path: Vec<&str> = file.root.as_deref().into_iter().collect();
            path.extend(section.split('/').filter(|s| !s.is_empty()));
            path.push(key);
            xml::get(text, &path)
        }
        Format::Json => json::get(text, section, key),
    })
}

/// Applies `edits` file by file. A file whose text changes is snapshotted under
/// `snapshots/<entry_id>/` first, unless it already is; one whose text stays is not touched at
/// all. One step per file: applied, present (nothing changed) or failed.
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
        let failed = |why: String| step("config", file, StepOutcome::Failed, Some(why));
        let before = match std::fs::read(file) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(t) => t,
                Err(_) => {
                    steps.push(failed("not UTF-8 text; left as it is".into()));
                    continue;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                steps.push(failed(e.to_string()));
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
                    steps.push(failed(why));
                    continue 'files;
                }
            }
        }
        if text == body {
            steps.push(step("config", file, StepOutcome::Present, None));
            continue;
        }
        if let Err(e) = snap.keep(file) {
            steps.push(failed(e));
            continue;
        }
        let bytes = format!("{bom}{text}").into_bytes();
        match write_atomic(file, &bytes) {
            Ok(()) => {
                // What hermir wrote, so a revert can tell whether anyone changed it since.
                let noted = snap.wrote(file, &bytes);
                steps.push(match noted {
                    Ok(()) => step("config", file, StepOutcome::Applied, None),
                    Err(e) => failed(e),
                });
            }
            Err(e) => steps.push(failed(io_note(&e))),
        }
    }
    steps
}

/// Puts back every file `apply` snapshotted for `entry_id`, and forgets what it put back. A
/// file changed since hermir wrote it is left as it is, its snapshot kept, unless `force`.
/// One step per file; none when nothing was outstanding.
pub fn revert(snapshots: &Path, entry_id: &str, force: bool) -> Vec<PrepareStep> {
    let mut snap = match Snapshot::open(snapshots, entry_id) {
        Ok(s) => s,
        Err(e) => return vec![step("revert", snapshots, StepOutcome::Failed, Some(e))],
    };
    let mut steps = Vec::new();
    let mut left = BTreeMap::new();
    for (key, kept) in std::mem::take(&mut snap.manifest) {
        let file = PathBuf::from(&key);
        let now = match std::fs::read(&file) {
            Ok(bytes) => Some(sha256(&bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                steps.push(step(
                    "revert",
                    &file,
                    StepOutcome::Failed,
                    Some(e.to_string()),
                ));
                left.insert(key, kept);
                continue;
            }
        };
        if !force
            && let Some(wrote) = &kept.wrote
            && now.as_ref() != Some(wrote)
        {
            steps.push(step(
                "revert",
                &file,
                StepOutcome::Conflict,
                Some(
                    "changed since hermir wrote it, so left as it is; the snapshot stays \
                     (`config revert --force` restores it anyway)"
                        .into(),
                ),
            ));
            left.insert(key, kept);
            continue;
        }
        let restored = match &kept.kept {
            Some(name) => std::fs::read(snap.dir.join("files").join(name))
                .and_then(|bytes| write_atomic(&file, &bytes)),
            None => match std::fs::remove_file(&file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => {
                    // Folders apply made for the file go too, once empty.
                    for d in &kept.dirs {
                        let _ = std::fs::remove_dir(d);
                    }
                    Ok(())
                }
            },
        };
        match restored {
            Ok(()) => steps.push(step("revert", &file, StepOutcome::Applied, None)),
            Err(e) => {
                steps.push(step(
                    "revert",
                    &file,
                    StepOutcome::Failed,
                    Some(io_note(&e)),
                ));
                left.insert(key, kept);
            }
        }
    }
    snap.manifest = left;
    if snap.manifest.is_empty() {
        let _ = std::fs::remove_dir_all(&snap.dir);
    } else if let Err(e) = snap.save() {
        steps.push(step("revert", &snap.dir, StepOutcome::Failed, Some(e)));
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

/// One snapshotted file: the copy of it under `files/` (none when it did not exist), the
/// sha256 of what hermir last wrote there, and the folders apply created for it.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct Kept {
    #[serde(default)]
    kept: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wrote: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    dirs: Vec<PathBuf>,
}

/// The manifest's rows; a manifest from before hermir recorded what it wrote holds the copy's
/// name alone.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Row {
    Kept(Kept),
    Old(Option<String>),
}

/// `<snapshots>/<emulator>/manifest.json`: file → what was kept of it.
struct Snapshot {
    dir: PathBuf,
    manifest: BTreeMap<String, Kept>,
}

impl Snapshot {
    fn open(snapshots: &Path, entry_id: &str) -> Result<Snapshot, String> {
        let dir = snapshots.join(entry_id);
        let path = dir.join("manifest.json");
        let manifest = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<BTreeMap<String, Row>>(&bytes)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .into_iter()
                .map(|(k, row)| {
                    let kept = match row {
                        Row::Kept(k) => k,
                        Row::Old(kept) => Kept {
                            kept,
                            ..Default::default()
                        },
                    };
                    (k, kept)
                })
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        Ok(Snapshot { dir, manifest })
    }

    fn save(&self) -> Result<(), String> {
        let path = self.dir.join("manifest.json");
        let json = serde_json::to_vec_pretty(&self.manifest).map_err(|e| e.to_string())?;
        write_atomic(&path, &json).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The file's bytes as they are now, unless it already has a snapshot: the first one
    /// stays, so it is the player's own and never hermir's.
    fn keep(&mut self, file: &Path) -> Result<(), String> {
        let key = key(file)?;
        if self.manifest.contains_key(&key) {
            return Ok(());
        }
        let files = self.dir.join("files");
        std::fs::create_dir_all(&files).map_err(|e| format!("{}: {e}", files.display()))?;
        let kept = match std::fs::read(file) {
            Ok(bytes) => {
                let name = format!("{}.bak", self.manifest.len());
                let at = files.join(&name);
                write_atomic(&at, &bytes).map_err(|e| format!("{}: {e}", at.display()))?;
                Kept {
                    kept: Some(name),
                    ..Default::default()
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Kept {
                kept: None,
                wrote: None,
                // Deepest first, so a revert can remove them in order.
                dirs: file
                    .ancestors()
                    .skip(1)
                    .take_while(|d| !d.as_os_str().is_empty() && !d.exists())
                    .map(Path::to_path_buf)
                    .collect(),
            },
            Err(e) => return Err(format!("{}: {e}", file.display())),
        };
        self.manifest.insert(key, kept);
        self.save()
    }

    fn wrote(&mut self, file: &Path, bytes: &[u8]) -> Result<(), String> {
        if let Some(k) = self.manifest.get_mut(&key(file)?) {
            k.wrote = Some(sha256(bytes));
        }
        self.save()
    }
}

/// A path as the manifest spells it. One that is not UTF-8 is refused rather than spelled
/// lossily, which would restore it somewhere else.
fn key(file: &Path) -> Result<String, String> {
    file.to_str()
        .map(String::from)
        .ok_or_else(|| format!("{}: the path is not UTF-8", file.display()))
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// An I/O error as a step says it: a file another program holds is most likely the emulator.
fn io_note(e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied && cfg!(windows) {
        format!("{e} (if the emulator is running, close it and retry)")
    } else {
        e.to_string()
    }
}

/// Writes `bytes` to `path` so that a reader sees the old file or the new one, never half of
/// either. A link is followed and stays a link: the file it points at gets the new text. The
/// file keeps its permissions (a 0600 settings file holding a password stays 0600). The new
/// text is flushed to disk, then renamed over the old, then the folder is flushed; on Windows a
/// rename the emulator's open handle refuses is retried briefly.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);

    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.hermir-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        rename(&tmp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return written;
    }
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e)
                if cfg!(windows)
                    && e.kind() == std::io::ErrorKind::PermissionDenied
                    && tries < 5 =>
            {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100 * tries));
            }
            other => return other,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ini(file: &Path, section: &str, key: &str, value: &str) -> Edit {
        Edit::Set {
            file: file.to_path_buf(),
            format: Format::Ini,
            case_insensitive: false,
            section: section.into(),
            key: key.into(),
            value: value.into(),
        }
    }

    #[test]
    fn a_file_whose_text_stays_is_neither_snapshotted_nor_touched() {
        let tmp = tempfile::tempdir().unwrap();
        let (snaps, file) = (tmp.path().join("snaps"), tmp.path().join("a.ini"));
        std::fs::write(&file, "[UI]\nFullscreen = true\n").unwrap();
        let steps = apply_edits(&snaps, "x", &[ini(&file, "UI", "Fullscreen", "true")]);
        assert_eq!(steps[0].outcome, StepOutcome::Present);
        assert!(outstanding(&snaps).is_empty());
        assert!(revert(&snaps, "x", false).is_empty());
    }

    #[test]
    fn a_revert_leaves_a_file_changed_since_unless_forced() {
        let tmp = tempfile::tempdir().unwrap();
        let (snaps, a, b) = (
            tmp.path().join("snaps"),
            tmp.path().join("a.ini"),
            tmp.path().join("b.ini"),
        );
        std::fs::write(&a, "[UI]\nFullscreen = false\n").unwrap();
        std::fs::write(&b, "[UI]\nVSync = false\n").unwrap();
        let edits = [
            ini(&a, "UI", "Fullscreen", "true"),
            ini(&b, "UI", "VSync", "true"),
        ];
        apply_edits(&snaps, "x", &edits);
        // The player changes a.ini while the session runs.
        std::fs::write(&a, "[UI]\nFullscreen = true\nTheme = dark\n").unwrap();
        let steps = revert(&snaps, "x", false);
        let by = |f: &Path| steps.iter().find(|s| s.target == f).unwrap().outcome;
        assert_eq!(by(&a), StepOutcome::Conflict);
        assert_eq!(by(&b), StepOutcome::Applied);
        assert_eq!(
            std::fs::read_to_string(&a).unwrap(),
            "[UI]\nFullscreen = true\nTheme = dark\n"
        );
        assert_eq!(
            std::fs::read_to_string(&b).unwrap(),
            "[UI]\nVSync = false\n"
        );
        assert_eq!(outstanding(&snaps), ["x"], "a.ini's snapshot stays");
        let forced = revert(&snaps, "x", true);
        assert_eq!(forced.len(), 1);
        assert_eq!(forced[0].outcome, StepOutcome::Applied);
        assert_eq!(
            std::fs::read_to_string(&a).unwrap(),
            "[UI]\nFullscreen = false\n"
        );
        assert!(outstanding(&snaps).is_empty());
    }

    #[test]
    fn folders_made_for_a_new_file_go_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let snaps = tmp.path().join("snaps");
        let file = tmp.path().join("root/input_configs/global/Default.yml");
        std::fs::create_dir_all(tmp.path().join("root")).unwrap();
        let edit = Edit::Whole {
            file: file.clone(),
            content: "Player 1 Input:\n".into(),
        };
        apply_edits(&snaps, "x", &[edit]);
        assert!(file.is_file());
        revert(&snaps, "x", false);
        assert!(!file.exists());
        assert!(!tmp.path().join("root/input_configs").exists());
        assert!(tmp.path().join("root").is_dir(), "what was there stays");
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_file_keeps_its_link_and_its_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let tmp = tempfile::tempdir().unwrap();
        let snaps = tmp.path().join("snaps");
        let real = tmp.path().join("dotfiles/retroarch.cfg");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "cheevos_password = \"secret\"\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = tmp.path().join("retroarch.cfg");
        symlink(&real, &link).unwrap();
        let steps = apply_edits(
            &snaps,
            "x",
            &[ini(&link, "", "video_fullscreen", "\"true\"")],
        );
        assert_eq!(steps[0].outcome, StepOutcome::Applied);
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert!(
            std::fs::read_to_string(&real)
                .unwrap()
                .contains("video_fullscreen")
        );
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        revert(&snaps, "x", false);
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            "cheevos_password = \"secret\"\n"
        );
        let leftovers: Vec<_> = std::fs::read_dir(real.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            leftovers.len(),
            1,
            "no temp file left behind: {leftovers:?}"
        );
    }

    #[test]
    fn a_manifest_from_before_hermir_recorded_its_writes_still_reverts() {
        let tmp = tempfile::tempdir().unwrap();
        let snaps = tmp.path().join("snaps");
        let file = tmp.path().join("a.ini");
        let made = tmp.path().join("made.ini");
        std::fs::write(&file, "[UI]\nX = 2\n").unwrap();
        std::fs::write(&made, "new\n").unwrap();
        let dir = snaps.join("x");
        std::fs::create_dir_all(dir.join("files")).unwrap();
        std::fs::write(dir.join("files/0.bak"), "[UI]\nX = 1\n").unwrap();
        let manifest = serde_json::json!({
            file.to_str().unwrap(): "0.bak",
            made.to_str().unwrap(): null,
        });
        std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
        let steps = revert(&snaps, "x", false);
        assert!(steps.iter().all(|s| s.outcome == StepOutcome::Applied));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[UI]\nX = 1\n");
        assert!(!made.exists());
    }
}
