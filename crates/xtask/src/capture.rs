//! An emulator's first start, as a fixture: `cargo xtask capture`, and the pieces `drift` and
//! `capture --interactive` share with it.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use hermir::{Catalog, Channel, Entry, InstallKind, Knob, Os};
use hermir_golden::check::{Tree, tree, write_tree};
use hermir_golden::fixture::{Captured, Mask, Meta, Session, Source, standard_sessions};
use hermir_golden::oracle::Oracle;
use serde::Deserialize;

const IMAGE: &str = "hermir-capture";
const VOLUME: &str = "hermir-flatpak";

/// `ci/capture/recipes/<emulator>.json`: what an emulator needs beyond the defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Recipe {
    /// Seconds to let it run at most; 90 by default. It is stopped sooner once the files the
    /// catalog names exist.
    settle: Option<u32>,
    /// Arguments after the app id.
    args: Vec<String>,
    /// From this many seconds on, ask its windows to close, every ten seconds, for an emulator
    /// that writes its settings only on a clean quit.
    close_after: Option<u32>,
    masks: Vec<Mask>,
    full_writer: bool,
    ignore_after: Vec<String>,
    /// Session name → what a person sets in the emulator's UI for it (`capture --interactive`).
    pub checklists: BTreeMap<String, Vec<String>>,
    /// Why the recipe is what it is.
    #[allow(dead_code)]
    notes: Option<String>,
}

/// How the emulator is started.
#[derive(Clone, Copy, Default)]
pub(crate) struct Run {
    /// On this machine, not in the capture image.
    pub host: bool,
    /// Overrides the recipe's settle time.
    pub settle: Option<u32>,
    /// On this desktop, until a person quits it.
    pub interactive: bool,
}

/// What one start of an emulator left: its version, the catalog's files as it wrote them
/// (masked), and the fixture's `meta.json` for them.
pub(crate) struct Start {
    pub version: String,
    pub meta: Meta,
    pub files: Tree,
    /// The script's `exit`: 124 or 143 still running when stopped, 0 quit.
    pub exit: String,
    pub raw: PathBuf,
}

impl Start {
    /// Whether it was still running when it was stopped, or quit cleanly: what a good start
    /// looks like.
    pub fn alive(&self) -> bool {
        matches!(self.exit.trim(), "0" | "124" | "143")
    }
}

pub fn main(args: &[String]) -> Result<(), String> {
    let catalog = Catalog::embedded().map_err(|e| e.to_string())?;
    let mut run = Run::default();
    let mut force = false;
    let mut session: Option<String> = None;
    let mut ids = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--host" => run.host = true,
            "--force" => force = true,
            "--interactive" => run.interactive = true,
            "--session" => session = Some(it.next().ok_or("--session takes a name")?.clone()),
            "--settle" => {
                let s = it.next().ok_or("--settle takes seconds")?;
                run.settle = Some(
                    s.parse()
                        .map_err(|_| format!("--settle {s}: not seconds"))?,
                );
            }
            "--all" => ids.extend(capturable(&catalog)),
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            id => ids.push(id.to_string()),
        }
    }
    if ids.is_empty() {
        return Err(super::USAGE.into());
    }
    if run.interactive {
        let [id] = ids.as_slice() else {
            return Err("--interactive captures one emulator at a time".into());
        };
        let session = session.ok_or("--interactive needs --session <name>")?;
        let entry = catalog
            .get(id)
            .ok_or_else(|| format!("{id} is not in the catalog"))?;
        return crate::interactive::capture(entry, &session, run);
    }
    let mut failed = Vec::new();
    for id in &ids {
        let entry = catalog
            .get(id)
            .ok_or_else(|| format!("{id} is not in the catalog"))?;
        match first_start(entry, run).and_then(|s| write_fixture(entry, &s, force)) {
            Ok(dir) => eprintln!("{id}: {}", dir.display()),
            Err(e) => {
                eprintln!("{id}: {e}");
                failed.push(id.as_str());
            }
        }
    }
    if failed.is_empty() {
        eprintln!(
            "next: HERMIR_BLESS=1 cargo test -p hermir-golden, then review the diff under fixtures/"
        );
        Ok(())
    } else {
        Err(format!("not captured: {}", failed.join(", ")))
    }
}

/// Every entry a capture can start: a Flathub id, config files, and not a Switch emulator.
pub(crate) fn capturable(catalog: &Catalog) -> Vec<String> {
    catalog
        .entries()
        .iter()
        .filter(|e| flatpak_id(e).is_some() && e.no_install.is_none())
        .filter(|e| e.config.as_ref().is_some_and(|c| !c.files.is_empty()))
        .map(|e| e.id.clone())
        .collect()
}

/// The Flathub id hermir installs or detects the emulator as.
pub(crate) fn flatpak_id(e: &Entry) -> Option<&str> {
    match e.channels.get(Os::Linux) {
        Some(Channel::Flatpak { flatpak }) => Some(flatpak),
        _ => e.detect.get(Os::Linux).and_then(|d| d.flatpak.as_deref()),
    }
}

/// The config root's path under the app's home (`config/PCSX2`), from the catalog.
fn under_app<'a>(entry: &'a Entry, app: &str) -> Result<&'a str, String> {
    entry
        .roots
        .flatpak
        .as_deref()
        .ok_or("no flatpak config root in the catalog")?
        .strip_prefix(&format!("~/.var/app/{app}/"))
        .ok_or_else(|| "the flatpak config root is not under ~/.var/app/<id>".into())
}

/// The catalog's files for Linux, by name: path under the config root, and how to read it.
fn files(entry: &Entry) -> Result<Vec<(String, String, Oracle)>, String> {
    let mut out = Vec::new();
    for (name, f) in entry.config.iter().flat_map(|c| &c.files) {
        let Some(rel) = f.path.get(Os::Linux) else {
            continue;
        };
        if rel.starts_with("{config}/") {
            return Err(format!(
                "{name} is under {{config}}/, which fixtures cannot hold yet"
            ));
        }
        out.push((
            name.clone(),
            rel.to_string(),
            Oracle::for_format(f.format, rel),
        ));
    }
    if out.is_empty() {
        return Err("the catalog names no config files for it yet".into());
    }
    Ok(out)
}

/// Starts the emulator once with a fresh home (seeded with `seed`, files under the config
/// root, when given) and reads back what it left.
pub(crate) fn start(entry: &Entry, run: Run, seed: Option<&Tree>) -> Result<Start, String> {
    let app = flatpak_id(entry).ok_or("no Flathub id in the catalog")?;
    let under_app = under_app(entry, app)?;
    let files = files(entry)?;
    let recipe = recipe(&entry.id)?;
    let settle = run.settle.or(recipe.settle).unwrap_or(90);
    let raw = super::root().join("target/capture").join(&entry.id);
    let _ = std::fs::remove_dir_all(&raw);
    std::fs::create_dir_all(&raw).map_err(|e| format!("{}: {e}", raw.display()))?;
    let seed_dir = match seed {
        Some(t) => {
            let dir = raw.join("seed");
            write_tree(&dir.join(".var/app").join(app).join(under_app), t)?;
            Some(dir)
        }
        None => None,
    };
    // What the start should write, relative to the app's home: it is stopped once all of it
    // exists. A seeded start runs its whole settle time: the files are there from the start.
    let wait_for = if seed.is_some() || run.interactive {
        String::new()
    } else {
        files
            .iter()
            .map(|(_, rel, _)| format!("{under_app}/{rel}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    run_script(
        app,
        settle,
        &raw,
        &recipe,
        &wait_for,
        seed_dir.as_deref(),
        run,
    )?;

    let info = std::fs::read_to_string(raw.join("info")).map_err(|e| format!("info: {e}"))?;
    let field = |name: &str| {
        info.lines()
            .find_map(|l| l.trim().strip_prefix(name)?.strip_prefix(':'))
            .map(|v| v.trim().to_string())
    };
    let version = normalise_version(&field("Version").ok_or("flatpak info has no Version")?);
    let exit = std::fs::read_to_string(raw.join("exit")).unwrap_or_default();
    let home = raw.join("home");
    let app_home = home.join(".var/app").join(app);
    let config_root = app_home.join(under_app);
    let mut meta = Meta {
        emulator: entry.id.clone(),
        version: version.clone(),
        os: Os::Linux,
        kind: InstallKind::Flatpak,
        source: Source {
            flatpak: Some(app.to_string()),
            commit: field("Commit"),
            url: None,
        },
        captured: Captured {
            on: super::today(),
            by: if run.interactive {
                "xtask capture --interactive".into()
            } else {
                "xtask capture".into()
            },
        },
        files: BTreeMap::new(),
        missing: Vec::new(),
        oracle: BTreeMap::new(),
        masks: recipe.masks.clone(),
        ignore_after: recipe.ignore_after.clone(),
        full_writer: recipe.full_writer,
        created: tree(&app_home)
            .into_keys()
            .filter(|k| !k.starts_with("cache/") && !k.starts_with(".ld.so/"))
            .collect(),
    };
    let mut out = Tree::new();
    for (name, rel, oracle) in files {
        meta.oracle.insert(name.clone(), oracle);
        match std::fs::read(config_root.join(&rel)) {
            Ok(bytes) => {
                let masked = mask(&bytes, recipe.masks.iter().filter(|m| m.file == name));
                out.insert(rel.clone(), masked);
                meta.files.insert(name, rel);
            }
            Err(_) => meta.missing.push(name),
        }
    }
    Ok(Start {
        version,
        meta,
        files: out,
        exit,
        raw,
    })
}

/// [`start`] with a fresh home: what the emulator writes on its very first start.
pub(crate) fn first_start(entry: &Entry, run: Run) -> Result<Start, String> {
    let s = start(entry, run, None)?;
    if !s.alive() {
        eprintln!(
            "{}: exited {} before it was stopped; see {}",
            entry.id,
            s.exit.trim(),
            s.raw.join("log").display()
        );
    }
    if s.files.is_empty() {
        return Err(format!(
            "the first start wrote none of {}; see {}",
            s.meta.missing.join(", "),
            s.raw.join("log").display()
        ));
    }
    if s.meta.full_writer {
        for w in unbound_keys(entry, &s.meta, &s.files) {
            eprintln!("{}: {w}", entry.id);
        }
    }
    Ok(s)
}

/// `fixtures/<emulator>/<version>/linux/`.
pub(crate) fn fixture_dir(entry_id: &str, version: &str) -> PathBuf {
    super::root()
        .join("fixtures")
        .join(entry_id)
        .join(version)
        .join("linux")
}

/// Writes `s` as the fixture for its version, with sessions; one that exists is kept when its
/// `before/` is the same, replaced only with `force`.
pub(crate) fn write_fixture(entry: &Entry, s: &Start, force: bool) -> Result<PathBuf, String> {
    let dir = fixture_dir(&entry.id, &s.version);
    if dir.join("meta.json").is_file() {
        let old = tree(&dir.join("before"));
        if old == s.files {
            eprintln!("{}: before/ unchanged", entry.id);
        } else if !force {
            return Err(format!(
                "{} exists and its before/ differs; --force replaces it",
                dir.display()
            ));
        }
        let _ = std::fs::remove_dir_all(dir.join("before"));
    }
    write_tree(&dir.join("before"), &s.files)?;
    let json = serde_json::to_string_pretty(&s.meta).map_err(|e| e.to_string())? + "\n";
    std::fs::write(dir.join("meta.json"), json).map_err(|e| e.to_string())?;
    seed_sessions(&dir)?;
    Ok(dir)
}

/// Runs `ci/capture/capture.sh`, in the capture image or on this machine.
fn run_script(
    app: &str,
    settle: u32,
    out: &Path,
    recipe: &Recipe,
    wait_for: &str,
    seed: Option<&Path>,
    run: Run,
) -> Result<(), String> {
    let script = super::root().join("ci/capture/capture.sh");
    let close_after = recipe
        .close_after
        .map(|s| s.to_string())
        .unwrap_or_default();
    let status = if run.host || run.interactive {
        let mut c = Command::new("sh");
        c.env("WAIT_FOR", wait_for)
            .env("CLOSE_AFTER", &close_after)
            .env("INTERACTIVE", if run.interactive { "1" } else { "" });
        if let Some(seed) = seed {
            c.env("SEED_HOME", seed);
        }
        c.arg(&script)
            .arg(app)
            .arg(settle.to_string())
            .arg(out)
            .args(&recipe.args)
            .status()
    } else {
        let have_image = Command::new("docker")
            .args(["image", "inspect", IMAGE])
            .output()
            .is_ok_and(|o| o.status.success());
        if !have_image {
            let built = Command::new("docker")
                .args(["build", "-t", IMAGE])
                .arg(super::root().join("ci/capture"))
                .status()
                .map_err(|e| format!("docker: {e}"))?;
            if !built.success() {
                return Err("docker build of ci/capture failed".into());
            }
        }
        // Extra `docker run` arguments, e.g. `--network host` behind a proxy; and the Flatpak
        // installation, a volume by default, or a host path that already holds one.
        let extra = std::env::var("HERMIR_CAPTURE_DOCKER_ARGS").unwrap_or_default();
        let volume = std::env::var("HERMIR_CAPTURE_VOLUME").unwrap_or_else(|_| VOLUME.into());
        let mut c = Command::new("docker");
        c.args(["run", "--rm", "--privileged", "-v"])
            .arg(format!("{volume}:/fp"))
            .arg("-v")
            .arg(format!("{}:/out", out.display()))
            .arg("-e")
            .arg(format!("WAIT_FOR={wait_for}"))
            .arg("-e")
            .arg(format!("CLOSE_AFTER={close_after}"));
        if seed.is_some() {
            // The seed is under the output folder, which the container sees as /out.
            c.arg("-e").arg("SEED_HOME=/out/seed");
        }
        c.args(extra.split_whitespace())
            .arg(IMAGE)
            .arg(app)
            .arg(settle.to_string())
            .arg("/out")
            .args(&recipe.args)
            .status()
    };
    let status = status.map_err(|e| format!("capture: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("capture script failed ({status})"))
    }
}

pub(crate) fn recipe(id: &str) -> Result<Recipe, String> {
    let p = super::root()
        .join("ci/capture/recipes")
        .join(format!("{id}.json"));
    match std::fs::read_to_string(&p) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display())),
        Err(_) => Ok(Recipe::default()),
    }
}

/// `v2.8.2` → `2.8.2`; anything a directory name should not hold → `-`.
fn normalise_version(v: &str) -> String {
    let v = match v.strip_prefix('v') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => rest,
        _ => v,
    };
    v.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._+-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// `key = value` lines of `[section]` with the value replaced by `masked`.
pub(crate) fn mask<'a>(bytes: &[u8], masks: impl Iterator<Item = &'a Mask>) -> Vec<u8> {
    let masks: Vec<&Mask> = masks.collect();
    if masks.is_empty() {
        return bytes.to_vec();
    }
    let text = String::from_utf8_lossy(bytes);
    let mut section = String::new();
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if let Some(inner) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            section = inner.trim().to_string();
        } else if let Some((k, v)) = line.split_once('=')
            && masks
                .iter()
                .any(|m| m.section == section && m.key == k.trim())
        {
            // The value keeps its quotes, so a TOML or quoted ini file stays well-formed.
            let quote = match v.trim_start().chars().next() {
                Some(q @ ('"' | '\'')) => q.to_string(),
                _ => String::new(),
            };
            let eol = &line[line.trim_end_matches(['\r', '\n']).len()..];
            out.push_str(&format!("{k}= {quote}masked{quote}{eol}"));
            continue;
        }
        out.push_str(line);
    }
    out.into_bytes()
}

/// For an emulator that writes every setting: the bound keys its first start did not write.
pub(crate) fn unbound_keys(entry: &Entry, meta: &Meta, before: &Tree) -> Vec<String> {
    let mut out = Vec::new();
    let Some(cfg) = entry.config.as_ref() else {
        return out;
    };
    for (knob, k) in &cfg.knobs {
        let Knob::Bound(b) = k else { continue };
        let (Some(rel), Some(oracle)) = (meta.files.get(&b.file), meta.oracle.get(&b.file)) else {
            continue;
        };
        let Some(flat) = before
            .get(rel)
            .and_then(|bytes| oracle.read(&String::from_utf8_lossy(bytes)).ok())
        else {
            continue;
        };
        if !flat.contains_key(&(b.section.clone(), b.key.clone())) {
            out.push(format!(
                "{knob} binds [{}] {} in {rel}, which the first start did not write",
                b.section, b.key
            ));
        }
    }
    out
}

/// New fixtures get the sessions of another version of the same emulator and OS, and every
/// standard one that version did not have; existing fixtures get the standard sessions added
/// since they were made.
pub(crate) fn seed_sessions(dir: &Path) -> Result<(), String> {
    let sessions = dir.join("sessions");
    if sessions.is_dir() {
        return add_standard(&sessions);
    }
    std::fs::create_dir_all(&sessions).map_err(|e| e.to_string())?;
    let os = dir.file_name().expect("an os dir");
    let emu = dir
        .parent()
        .and_then(Path::parent)
        .expect("an emulator dir");
    let sibling = std::fs::read_dir(emu)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join(os).join("sessions"))
        .find(|p| p.is_dir() && p != &sessions);
    if let Some(from) = sibling {
        for e in std::fs::read_dir(&from)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let text = std::fs::read_to_string(e.path()).map_err(|e| e.to_string())?;
            let mut s: Session = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            s.expect.clear();
            let json = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())? + "\n";
            std::fs::write(sessions.join(e.file_name()), json).map_err(|e| e.to_string())?;
        }
    }
    add_standard(&sessions)
}

/// Writes each standard session `sessions` lacks; `HERMIR_BLESS=1` fills in what it expects.
pub(crate) fn add_standard(sessions: &Path) -> Result<(), String> {
    for (name, s) in standard_sessions() {
        let path = sessions.join(format!("{name}.json"));
        if path.exists() {
            continue;
        }
        let json = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())? + "\n";
        std::fs::write(path, json).map_err(|e| e.to_string())?;
    }
    Ok(())
}
