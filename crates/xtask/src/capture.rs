//! `cargo xtask capture`: an emulator's first start, as a fixture.
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
struct Recipe {
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
    /// Why the recipe is what it is.
    #[allow(dead_code)]
    notes: Option<String>,
}

struct Opts {
    host: bool,
    settle: Option<u32>,
    force: bool,
}

pub fn main(args: &[String]) -> Result<(), String> {
    let catalog = Catalog::embedded().map_err(|e| e.to_string())?;
    let mut opts = Opts {
        host: false,
        settle: None,
        force: false,
    };
    let mut ids = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--host" => opts.host = true,
            "--force" => opts.force = true,
            "--settle" => {
                let s = it.next().ok_or("--settle takes seconds")?;
                opts.settle = Some(
                    s.parse()
                        .map_err(|_| format!("--settle {s}: not seconds"))?,
                );
            }
            "--all" => ids.extend(
                catalog
                    .entries()
                    .iter()
                    .filter(|e| flatpak_id(e).is_some() && e.no_install.is_none())
                    .map(|e| e.id.clone()),
            ),
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            id => ids.push(id.to_string()),
        }
    }
    if ids.is_empty() {
        return Err(super::USAGE.into());
    }
    let mut failed = Vec::new();
    for id in &ids {
        let entry = catalog
            .get(id)
            .ok_or_else(|| format!("{id} is not in the catalog"))?;
        match capture(entry, &opts) {
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

/// The Flathub id hermir installs or detects the emulator as.
fn flatpak_id(e: &Entry) -> Option<&str> {
    match e.channels.get(Os::Linux) {
        Some(Channel::Flatpak { flatpak }) => Some(flatpak),
        _ => e.detect.get(Os::Linux).and_then(|d| d.flatpak.as_deref()),
    }
}

fn capture(entry: &Entry, opts: &Opts) -> Result<PathBuf, String> {
    let app = flatpak_id(entry).ok_or("no Flathub id in the catalog")?;
    if entry.config.as_ref().is_none_or(|c| c.files.is_empty()) {
        return Err("the catalog names no config files for it yet".into());
    }
    let root_tpl = entry
        .roots
        .flatpak
        .as_deref()
        .ok_or("no flatpak config root in the catalog")?;
    let recipe = recipe(&entry.id)?;
    let settle = opts.settle.or(recipe.settle).unwrap_or(90);
    let raw = super::root().join("target/capture").join(&entry.id);
    let _ = std::fs::remove_dir_all(&raw);
    std::fs::create_dir_all(&raw).map_err(|e| format!("{}: {e}", raw.display()))?;

    // What the first start should write, relative to the app's home: the capture stops once
    // all of it exists.
    let under_app = root_tpl
        .strip_prefix(&format!("~/.var/app/{app}/"))
        .ok_or("the flatpak config root is not under ~/.var/app/<id>")?;
    let wait_for: Vec<String> = entry
        .config
        .iter()
        .flat_map(|c| c.files.values())
        .filter_map(|f| f.path.get(Os::Linux))
        .filter(|rel| !rel.starts_with('{'))
        .map(|rel| format!("{under_app}/{rel}"))
        .collect();
    run_capture(app, settle, &raw, &recipe, &wait_for.join(" "), opts.host)?;

    let info = std::fs::read_to_string(raw.join("info")).map_err(|e| format!("info: {e}"))?;
    let field = |name: &str| {
        info.lines()
            .find_map(|l| l.trim().strip_prefix(name)?.strip_prefix(':'))
            .map(|v| v.trim().to_string())
    };
    let version = normalise_version(&field("Version").ok_or("flatpak info has no Version")?);
    let exit = std::fs::read_to_string(raw.join("exit")).unwrap_or_default();
    if !matches!(exit.trim(), "0" | "124" | "143") {
        eprintln!(
            "{}: exited {} before it was stopped; see {}",
            entry.id,
            exit.trim(),
            raw.join("log").display()
        );
    }

    let home = raw.join("home");
    let config_root = expand_home(root_tpl, &home);
    let app_home = home.join(".var/app").join(app);
    let files = entry.config.as_ref().map(|c| &c.files);
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
            by: "xtask capture".into(),
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
    let mut before = Tree::new();
    for (name, f) in files.into_iter().flatten() {
        let Some(rel) = f.path.get(Os::Linux) else {
            continue;
        };
        if rel.starts_with("{config}/") {
            return Err(format!(
                "{name} is under {{config}}/, which fixtures cannot hold yet"
            ));
        }
        meta.oracle
            .insert(name.clone(), Oracle::for_format(f.format, rel));
        match std::fs::read(config_root.join(rel)) {
            Ok(bytes) => {
                let masked = mask(&bytes, recipe.masks.iter().filter(|m| m.file == *name));
                before.insert(rel.to_string(), masked);
                meta.files.insert(name.clone(), rel.to_string());
            }
            Err(_) => meta.missing.push(name.clone()),
        }
    }
    if before.is_empty() && !meta.missing.is_empty() {
        return Err(format!(
            "the first start wrote none of {}; see {}",
            meta.missing.join(", "),
            raw.join("log").display()
        ));
    }
    if meta.full_writer {
        for w in unbound_keys(entry, &meta, &before) {
            eprintln!("{}: {w}", entry.id);
        }
    }

    let dir = super::root()
        .join("fixtures")
        .join(&entry.id)
        .join(&version)
        .join("linux");
    if dir.join("meta.json").is_file() {
        let old = tree(&dir.join("before"));
        if old == before {
            eprintln!("{}: before/ unchanged", entry.id);
        } else if !opts.force {
            return Err(format!(
                "{} exists and its before/ differs; --force replaces it",
                dir.display()
            ));
        }
        let _ = std::fs::remove_dir_all(dir.join("before"));
    }
    write_tree(&dir.join("before"), &before)?;
    let json = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())? + "\n";
    std::fs::write(dir.join("meta.json"), json).map_err(|e| e.to_string())?;
    seed_sessions(&dir)?;
    Ok(dir)
}

/// Runs `ci/capture/capture.sh`, in the capture image or on this machine.
fn run_capture(
    app: &str,
    settle: u32,
    out: &Path,
    recipe: &Recipe,
    wait_for: &str,
    host: bool,
) -> Result<(), String> {
    let args = &recipe.args;
    let close_after = recipe
        .close_after
        .map(|s| s.to_string())
        .unwrap_or_default();
    let script = super::root().join("ci/capture/capture.sh");
    let status = if host {
        Command::new("sh")
            .env("WAIT_FOR", wait_for)
            .env("CLOSE_AFTER", &close_after)
            .arg(&script)
            .arg(app)
            .arg(settle.to_string())
            .arg(out)
            .args(args)
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
        Command::new("docker")
            .args(["run", "--rm", "--privileged", "-v"])
            .arg(format!("{volume}:/fp"))
            .arg("-v")
            .arg(format!("{}:/out", out.display()))
            .arg("-e")
            .arg(format!("WAIT_FOR={wait_for}"))
            .arg("-e")
            .arg(format!("CLOSE_AFTER={close_after}"))
            .args(extra.split_whitespace())
            .arg(IMAGE)
            .arg(app)
            .arg(settle.to_string())
            .arg("/out")
            .args(args)
            .status()
    };
    let status = status.map_err(|e| format!("capture: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("capture script failed ({status})"))
    }
}

fn recipe(id: &str) -> Result<Recipe, String> {
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

fn expand_home(tpl: &str, home: &Path) -> PathBuf {
    match tpl.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(tpl),
    }
}

/// `key = value` lines of `[section]` with the value replaced by `masked`.
fn mask<'a>(bytes: &[u8], masks: impl Iterator<Item = &'a Mask>) -> Vec<u8> {
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
fn unbound_keys(entry: &Entry, meta: &Meta, before: &Tree) -> Vec<String> {
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

/// New fixtures get the sessions of another version of the same emulator and OS, or the
/// standard ones.
fn seed_sessions(dir: &Path) -> Result<(), String> {
    let sessions = dir.join("sessions");
    if sessions.is_dir() {
        return Ok(());
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
        return Ok(());
    }
    for (name, s) in standard_sessions() {
        let json = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())? + "\n";
        std::fs::write(sessions.join(format!("{name}.json")), json).map_err(|e| e.to_string())?;
    }
    Ok(())
}
