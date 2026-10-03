//! `cargo xtask drift`: whether each emulator's current release still takes what hermir
//! writes. A release whose files are what the newest fixture holds is unchanged; a new one is
//! captured as a fixture (sessions copied, expectations blessed for review), checked, and
//! started on the files hermir patched. The two newest versions per emulator are kept.
use std::fmt::Write as _;
use std::path::PathBuf;

use hermir::{Catalog, Entry, Hermir, Options, Os};
use hermir_golden::check::{self, Home, Tree, tree, write_tree};
use hermir_golden::fixture::{Fixture, discover};

use crate::capture::{self, Run};

enum Outcome {
    Unchanged(String),
    New {
        version: String,
        dir: PathBuf,
        problems: Vec<String>,
    },
}

pub fn main(args: &[String]) -> Result<(), String> {
    let catalog = Catalog::embedded().map_err(|e| e.to_string())?;
    let mut run = Run::default();
    let mut ids = Vec::new();
    for a in args {
        match a.as_str() {
            "--host" => run.host = true,
            "--all" => ids.extend(capture::capturable(&catalog)),
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            id => ids.push(id.to_string()),
        }
    }
    if ids.is_empty() {
        return Err(super::USAGE.into());
    }
    let mut report = String::from("| emulator | release | result |\n|---|---|---|\n");
    let mut failed = 0;
    for id in &ids {
        let entry = catalog
            .get(id)
            .ok_or_else(|| format!("{id} is not in the catalog"))?;
        let row = match drift(entry, run) {
            Ok(Outcome::Unchanged(v)) => format!("| {id} | {v} | unchanged |"),
            Ok(Outcome::New {
                version,
                dir,
                problems,
            }) if problems.is_empty() => format!(
                "| {id} | {version} | new fixture, every check passed: `{}` |",
                dir.strip_prefix(super::root()).unwrap_or(&dir).display()
            ),
            Ok(Outcome::New {
                version, problems, ..
            }) => {
                failed += 1;
                format!(
                    "| {id} | {version} | **{} problem(s)**: {} |",
                    problems.len(),
                    problems.join("; ").replace('|', "\\|").replace('\n', " ")
                )
            }
            Err(e) => {
                failed += 1;
                format!("| {id} | ? | **not captured**: {} |", e.replace('|', "\\|"))
            }
        };
        eprintln!("{row}");
        let _ = writeln!(report, "{row}");
    }
    let out = super::root().join("target/drift");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    std::fs::write(out.join("summary.md"), &report).map_err(|e| e.to_string())?;
    if failed == 0 {
        Ok(())
    } else {
        Err(format!(
            "{failed} emulator(s) drifted; target/drift/summary.md says how"
        ))
    }
}

/// The newest fixture of an emulator on Linux, by capture date.
fn newest(id: &str) -> Option<Fixture> {
    let mut all: Vec<Fixture> = discover(&super::root().join("fixtures"))
        .ok()?
        .into_iter()
        .filter(|f| f.meta.emulator == id && f.meta.os == Os::Linux)
        .collect();
    all.sort_by(|a, b| a.meta.captured.on.cmp(&b.meta.captured.on));
    all.pop()
}

fn drift(entry: &Entry, run: Run) -> Result<Outcome, String> {
    let s = capture::first_start(entry, run)?;
    if let Some(fx) = newest(&entry.id)
        && fx.meta.version == s.version
        && tree(&fx.dir.join("before")) == s.files
    {
        return Ok(Outcome::Unchanged(s.version));
    }
    let mut problems = Vec::new();
    if !s.alive() {
        problems.push(format!("its first start exited {}", s.exit.trim()));
    }
    if s.meta.full_writer {
        problems.extend(capture::unbound_keys(entry, &s.meta, &s.files));
    }
    let dir = capture::write_fixture(entry, &s, true)?;
    let fx = Fixture::load(&dir)?;
    for session in &fx.sessions {
        if let Err(e) = check::run(&fx, session, true) {
            problems.push(format!("{session}: {e}"));
        }
    }
    problems.extend(smoke(entry, &fx, run)?);
    prune(&entry.id, 2)?;
    Ok(Outcome::New {
        version: s.version,
        dir,
        problems,
    })
}

/// The emulator started on the files hermir wrote for `video-all`: it must still start, and a
/// file it rewrites must keep what hermir set.
fn smoke(entry: &Entry, fx: &Fixture, run: Run) -> Result<Vec<String>, String> {
    let Ok(session) = fx.session("video-all") else {
        return Ok(Vec::new());
    };
    let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let open = |root: &std::path::Path| -> Result<(Hermir, PathBuf), String> {
        let h = Hermir::open(Options {
            prefix: Some(root.join("prefix")),
            os: Some(Os::Linux),
            env: Some(Box::new(Home(root.join("home"), Os::Linux))),
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
        Ok((h, root.join("root")))
    };
    let (h, root) = open(&tmp.path().join("patched"))?;
    write_tree(&root, &tree(&fx.dir.join("before")))?;
    let install = check::install(fx, &root);
    let emu = h.emulator(&entry.id).map_err(|e| e.to_string())?;
    emu.apply(&install, &session.patch)
        .map_err(|e| e.to_string())?;
    let patched: Tree = tree(&root);
    let wrote = emu.get(&install).map_err(|e| e.to_string())?;

    let started = capture::start(
        entry,
        Run {
            settle: Some(25),
            ..run
        },
        Some(&patched),
    )?;
    let mut problems = Vec::new();
    if !started.alive() {
        problems.push(format!(
            "it exited {} on the files hermir wrote; see {}",
            started.exit.trim(),
            started.raw.join("log").display()
        ));
    }
    if started
        .files
        .iter()
        .any(|(rel, b)| patched.get(rel) != Some(b))
    {
        // It rewrote a file on load: what hermir set must have survived.
        let (h2, root2) = open(&tmp.path().join("rewritten"))?;
        write_tree(&root2, &started.files)?;
        let kept = h2
            .emulator(&entry.id)
            .and_then(|e| e.get(&check::install(fx, &root2)))
            .map_err(|e| e.to_string())?;
        // Fullscreen is not checked: under Xvfb there is no window manager to grant it, and an
        // emulator that saves its window state (mGBA) writes back the windowed one it got.
        for w in wrote.iter().filter(|w| w.knob != "video.fullscreen") {
            let k = kept.iter().find(|k| k.knob == w.knob);
            if w.value.is_some() && k.and_then(|k| k.value.as_ref()) != w.value.as_ref() {
                let file = w
                    .file
                    .as_ref()
                    .map(|f| f.strip_prefix(&root).unwrap_or(f).display().to_string())
                    .unwrap_or_default();
                problems.push(format!(
                    "it rewrote {file} on load and {} went from {:?} to {:?}",
                    w.knob,
                    w.value,
                    k.and_then(|k| k.value.clone())
                ));
            }
        }
    }
    Ok(problems)
}

/// Keeps the `keep` newest versions of an emulator's Linux fixtures.
fn prune(id: &str, keep: usize) -> Result<(), String> {
    let mut all: Vec<Fixture> = discover(&super::root().join("fixtures"))?
        .into_iter()
        .filter(|f| f.meta.emulator == id && f.meta.os == Os::Linux)
        .collect();
    all.sort_by(|a, b| b.meta.captured.on.cmp(&a.meta.captured.on));
    for old in all.into_iter().skip(keep) {
        let version_dir = old.dir.parent().unwrap_or(&old.dir);
        std::fs::remove_dir_all(&old.dir).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir(version_dir);
    }
    Ok(())
}
