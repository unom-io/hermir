//! One golden test: a fixture's `before/` copied into a temp root, one session applied through
//! hermir's public API, and the checks of `docs/plan.md` §2.2:
//!
//! - **A** each knob reports the support the session expects;
//! - **B** what hermir wrote equals `hermir/<session>/`, byte for byte;
//! - **C** every file hermir wrote is well-formed, by an independent reader;
//! - **D** the keys that changed are the ones the catalog binds for the session's knobs;
//! - **E** where `after/<session>/` exists, hermir changed what the emulator changed, to the
//!   same values, and nothing else;
//! - **H** `get` reads back what each applied knob was set to;
//! - **G** a second apply changes nothing;
//! - **F** revert gives `before/` back byte for byte, and leaves nothing outstanding.
//!
//! `bless` rewrites the expectations (A's `expect`, B's files, a players session's `may_touch`)
//! instead of checking them; the PR diff is where they are reviewed.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use hermir::{
    Binding, Catalog, Entry, Exe, Hermir, Install, InstallKind, Knob, KnobValue, Options, Patch,
    StepOutcome, Support,
};

use crate::fixture::{Fixture, Session};
use crate::oracle::{Flat, Oracle};

/// Relative path → bytes, for every file under a directory.
pub type Tree = BTreeMap<String, Vec<u8>>;

/// `(file under the config root, section, key)`.
type Addr = (String, String, String);

/// Runs `session` of `fx`; `Err` lists every check that failed.
pub fn run(fx: &Fixture, session: &str, bless: bool) -> Result<(), String> {
    let catalog = Catalog::embedded().map_err(|e| e.to_string())?;
    let entry = catalog
        .get(&fx.meta.emulator)
        .ok_or_else(|| format!("{} is not in the catalog", fx.meta.emulator))?;
    let mut s = fx.session(session)?;
    let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let root = tmp.path().join("root");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let before = tree(&fx.dir.join("before"));
    write_tree(&root, &before)?;

    let h = Hermir::open(Options {
        prefix: Some(tmp.path().join("prefix")),
        os: Some(fx.meta.os),
        ..Default::default()
    })
    .map_err(|e| e.to_string())?;
    let emu = h.emulator(&fx.meta.emulator).map_err(|e| e.to_string())?;
    let exe = match (fx.meta.kind, &fx.meta.source.flatpak) {
        (InstallKind::Flatpak, Some(id)) => Exe::FlatpakRun(id.clone()),
        _ => Exe::Path(root.join("emulator")),
    };
    let install = Install {
        emulator: fx.meta.emulator.clone(),
        kind: fx.meta.kind,
        exe,
        version: Some(fx.meta.version.clone()),
        config_root: Some(root.clone()),
    };

    let mut failures = Vec::new();
    let applied = emu.apply(&install, &s.patch).map_err(|e| e.to_string())?;
    for st in applied
        .steps
        .iter()
        .filter(|st| st.outcome == StepOutcome::Failed)
    {
        failures.push(format!(
            "apply: {} failed: {}",
            st.target.display(),
            st.note.as_deref().unwrap_or("")
        ));
    }

    // A
    let reported: BTreeMap<_, _> = applied
        .knobs
        .iter()
        .map(|k| (k.knob.clone(), k.support))
        .collect();
    if bless {
        s.expect = reported;
    } else if s.expect != reported {
        failures.push(format!(
            "A: apply reported {reported:?}, the session expects {:?}",
            s.expect
        ));
    }

    let first = tree(&root);
    let changed: Tree = first
        .iter()
        .filter(|(k, v)| before.get(*k) != Some(*v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // B
    let golden_dir = fx.dir.join("hermir").join(session);
    if bless {
        let _ = std::fs::remove_dir_all(&golden_dir);
        if !changed.is_empty() {
            write_tree(&golden_dir, &changed)?;
        }
    } else {
        let golden = tree(&golden_dir);
        if golden != changed {
            failures.push(format!("B: {}", describe(&golden, &changed)));
        }
    }

    // C and D
    let allowed = addressed(entry, fx, &s.patch);
    let mut extras: Vec<(String, Option<String>, String)> = Vec::new();
    for (rel, bytes) in &changed {
        let Some(oracle) = oracle_for(fx, entry, rel) else {
            extras.push((rel.clone(), None, "the whole file".into()));
            continue;
        };
        let after_text = String::from_utf8_lossy(bytes);
        let before_text = before.get(rel).map(|b| String::from_utf8_lossy(b));
        let flat_after = match oracle.read(&after_text) {
            Ok(f) => f,
            Err(e) => {
                let was = before_text.as_deref().map(|t| oracle.read(t));
                if !matches!(&was, Some(Err(w)) if *w == e) {
                    failures.push(format!("C: {rel} is not well-formed after apply: {e}"));
                }
                continue;
            }
        };
        let flat_before = before_text
            .as_deref()
            .and_then(|t| oracle.read(t).ok())
            .unwrap_or_default();
        for (section, key) in differing(&flat_before, &flat_after) {
            if !allowed.contains(&(rel.clone(), section.clone(), key.clone())) {
                extras.push((rel.clone(), Some(section), key));
            }
        }
    }
    // `<file>` covers the whole file; `<file>:<section>` that section and everything under it.
    let covered = |rel: &str, section: Option<&str>, mt: &[String]| {
        mt.iter().any(|m| match m.split_once(':') {
            None => m == rel,
            Some((f, s)) => {
                f == rel
                    && section.is_some_and(|sec| {
                        sec == s || sec.strip_prefix(s).is_some_and(|r| r.starts_with('/'))
                    })
            }
        })
    };
    match &s.may_touch {
        None if bless && s.patch.players.is_some() => {
            // A file the adapter created is its own; elsewhere, the top-level sections it wrote.
            let mut mt: BTreeSet<String> = BTreeSet::new();
            for (rel, section, _) in &extras {
                mt.insert(match section {
                    Some(sec) if before.contains_key(rel) => {
                        let top = sec.split('/').next().unwrap_or(sec);
                        format!("{rel}:{}", top.split('#').next().unwrap_or(top))
                    }
                    _ => rel.clone(),
                });
            }
            s.may_touch = Some(mt.into_iter().collect());
        }
        mt => {
            let mt = mt.as_deref().unwrap_or_default();
            let stray: Vec<String> = extras
                .iter()
                .filter(|(rel, sec, _)| !covered(rel, sec.as_deref(), mt))
                .map(|(rel, sec, key)| match sec {
                    Some(sec) => format!("{rel} [{sec}] {key}"),
                    None => format!("{rel} ({key})"),
                })
                .collect();
            if !stray.is_empty() {
                failures.push(format!(
                    "D: apply changed what the session did not ask for: {}",
                    stray.join(", ")
                ));
            }
        }
    }

    // H
    failures.extend(read_back(
        entry,
        &s,
        &emu.get(&install).map_err(|e| e.to_string())?,
    ));

    // E
    let after_dir = fx.dir.join("after").join(session);
    if after_dir.is_dir() {
        failures.extend(against_emulator(
            fx,
            entry,
            &before,
            &tree(&after_dir),
            &first,
        ));
    }

    // G
    let again = emu.apply(&install, &s.patch).map_err(|e| e.to_string())?;
    if again
        .steps
        .iter()
        .any(|st| st.outcome == StepOutcome::Failed)
    {
        failures.push("G: the second apply failed".into());
    }
    let second = tree(&root);
    if second != first {
        failures.push(format!(
            "G: a second apply changed files: {}",
            describe(&first, &second)
        ));
    }

    // F
    for st in emu.revert(false).map_err(|e| e.to_string())? {
        if st.outcome == StepOutcome::Failed {
            failures.push(format!(
                "F: revert of {} failed: {}",
                st.target.display(),
                st.note.as_deref().unwrap_or("")
            ));
        }
    }
    let reverted = tree(&root);
    if reverted != before {
        failures.push(format!(
            "F: revert did not restore before/: {}",
            describe(&before, &reverted)
        ));
    }
    if !emu.revert(false).map_err(|e| e.to_string())?.is_empty() {
        failures.push("F: a snapshot is still outstanding after revert".into());
    }

    if bless {
        let json = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())? + "\n";
        std::fs::write(fx.session_path(session), json).map_err(|e| e.to_string())?;
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

/// Check E: what the emulator wrote in `after` against what hermir wrote in `hermir`.
fn against_emulator(
    fx: &Fixture,
    entry: &Entry,
    before: &Tree,
    after: &Tree,
    hermir: &Tree,
) -> Vec<String> {
    let mut failures = Vec::new();
    let files: BTreeSet<&String> = after
        .keys()
        .chain(hermir.keys().filter(|k| before.get(*k) != hermir.get(*k)))
        .collect();
    for rel in files {
        let Some(oracle) = oracle_for(fx, entry, rel) else {
            continue;
        };
        let read = |t: Option<&Vec<u8>>| {
            t.and_then(|b| oracle.read(&String::from_utf8_lossy(b)).ok())
                .unwrap_or_default()
        };
        let b = read(before.get(rel));
        let a = after.get(rel).map_or_else(|| b.clone(), |x| read(Some(x)));
        let h = hermir.get(rel).map_or_else(|| b.clone(), |x| read(Some(x)));
        let ignored = |(section, key): &(String, String)| {
            fx.meta
                .ignore_after
                .iter()
                .any(|i| *i == format!("{section}/{key}"))
        };
        let emu: BTreeSet<_> = differing(&b, &a)
            .into_iter()
            .filter(|k| !ignored(k))
            .collect();
        for k in &emu {
            if h.get(k) != a.get(k) {
                failures.push(format!(
                    "E: {rel} [{}] {}: the emulator wrote {:?}, hermir {:?}",
                    k.0,
                    k.1,
                    a.get(k),
                    h.get(k)
                ));
            }
        }
        for k in differing(&b, &h) {
            if !emu.contains(&k) && !ignored(&k) {
                failures.push(format!(
                    "E: {rel} [{}] {}: hermir changed it, the emulator did not",
                    k.0, k.1
                ));
            }
        }
    }
    failures
}

/// Check H: every knob `apply` reported written reads back as what the session asked for, or
/// as a value the emulator spells the same way (PPSSPP writes 16:9 and auto alike).
fn read_back(entry: &Entry, s: &Session, got: &[KnobValue]) -> Vec<String> {
    let mut asked: Vec<(&str, String)> = Vec::new();
    if let Some(v) = &s.patch.video {
        if let Some(f) = v.fullscreen {
            asked.push(("video.fullscreen", f.to_string()));
        }
        if let Some(n) = v.scale {
            asked.push(("video.scale", n.to_string()));
        }
        if let Some(f) = v.vsync {
            asked.push(("video.vsync", f.to_string()));
        }
        if let Some(a) = v.aspect {
            asked.push(("video.aspect", a.as_str().to_string()));
        }
    }
    if let Some(r) = s.patch.region {
        asked.push(("region", r.as_str().to_string()));
    }
    let mut failures = Vec::new();
    for (knob, want) in asked {
        if !matches!(
            s.expect.get(knob),
            Some(Support::Applied | Support::Partial)
        ) {
            continue;
        }
        let Some(Knob::Bound(b)) = entry.config.as_ref().and_then(|c| c.knobs.get(knob)) else {
            continue;
        };
        let read = got.iter().find(|v| v.knob == knob);
        let value = read.and_then(|v| v.value.as_deref());
        let same = value.is_some_and(|v| v == want || spell(b, v) == spell(b, &want));
        if !same {
            failures.push(format!(
                "H: {knob} was set to {want}, get reads {:?} ({:?})",
                value,
                read.and_then(|v| v.literal.as_deref().or(v.note.as_deref()))
            ));
        }
    }
    failures
}

/// How the binding spells a neutral value.
fn spell(b: &Binding, neutral: &str) -> Option<String> {
    if let Some([t, f]) = &b.bool {
        return match neutral {
            "true" => Some(t.clone()),
            "false" => Some(f.clone()),
            _ => None,
        };
    }
    if let Some(values) = &b.values {
        return values.get(neutral).cloned();
    }
    b.scale
        .as_ref()
        .and_then(|sc| sc.render(neutral.parse().ok()?))
}

/// Keys whose value differs between `a` and `b`, or that only one has.
fn differing(a: &Flat, b: &Flat) -> Vec<(String, String)> {
    let keys: BTreeSet<&(String, String)> = a.keys().chain(b.keys()).collect();
    keys.into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .cloned()
        .collect()
}

/// Every key the catalog says the session's knobs and native keys write.
fn addressed(entry: &Entry, fx: &Fixture, patch: &Patch) -> BTreeSet<Addr> {
    let mut out = BTreeSet::new();
    let Some(cfg) = entry.config.as_ref() else {
        return out;
    };
    let mut push = |file: &str, section: &str, key: &str| {
        let Some(f) = cfg.files.get(file) else {
            return;
        };
        let Some(rel) = f.path.get(fx.meta.os) else {
            return;
        };
        let section = match (f.format, f.root.as_deref()) {
            (hermir::Format::Xml, Some(root)) if section.is_empty() => root.to_string(),
            (hermir::Format::Xml, Some(root)) => format!("{root}/{section}"),
            _ => section.to_string(),
        };
        out.insert((rel.to_string(), section.clone(), key.to_string()));
        if f.format == hermir::Format::Qt {
            out.insert((rel.to_string(), section, format!("{key}\\default")));
        }
    };
    let mut knobs = Vec::new();
    if let Some(v) = &patch.video {
        for (name, set) in [
            ("video.fullscreen", v.fullscreen.is_some()),
            ("video.scale", v.scale.is_some()),
            ("video.vsync", v.vsync.is_some()),
            ("video.aspect", v.aspect.is_some()),
        ] {
            if set {
                knobs.push(name);
            }
        }
    }
    if patch.region.is_some() {
        knobs.push("region");
    }
    for name in knobs {
        if let Some(Knob::Bound(b)) = cfg.knobs.get(name) {
            push(&b.file, &b.section, &b.key);
            for a in &b.also {
                push(
                    a.file.as_deref().unwrap_or(&b.file),
                    a.section.as_deref().unwrap_or(&b.section),
                    &a.key,
                );
            }
        }
    }
    for n in &patch.native {
        push(&n.file, &n.section, &n.key);
    }
    out
}

/// How to read `rel`: by the catalog file it is, else by its extension.
fn oracle_for(fx: &Fixture, entry: &Entry, rel: &str) -> Option<Oracle> {
    if let Some((name, _)) = fx.meta.files.iter().find(|(_, p)| *p == rel)
        && let Some(o) = fx.meta.oracle.get(name)
    {
        return Some(*o);
    }
    let cfg = entry.config.as_ref()?;
    cfg.files
        .values()
        .find(|f| f.path.get(fx.meta.os) == Some(rel))
        .map(|f| Oracle::for_format(f.format, rel))
        .or_else(|| Oracle::for_path(rel))
}

/// Every file under `dir`, by `/`-separated relative path. Links are not followed.
pub fn tree(dir: &Path) -> Tree {
    fn walk(root: &Path, dir: &Path, out: &mut Tree) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                walk(root, &path, out);
            } else if ft.is_file()
                && let Ok(bytes) = std::fs::read(&path)
            {
                let rel = path
                    .strip_prefix(root)
                    .expect("under root")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(rel, bytes);
            }
        }
    }
    let mut out = Tree::new();
    walk(dir, dir, &mut out);
    out
}

pub fn write_tree(dir: &Path, files: &Tree) -> Result<(), String> {
    for (rel, bytes) in files {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}

/// What differs between two trees, briefly: missing and extra files, and the first differing
/// line of each file in both.
fn describe(want: &Tree, got: &Tree) -> String {
    let mut out = Vec::new();
    for rel in want.keys().filter(|k| !got.contains_key(*k)) {
        out.push(format!("{rel} missing"));
    }
    for rel in got.keys().filter(|k| !want.contains_key(*k)) {
        out.push(format!("{rel} unexpected"));
    }
    for (rel, w) in want {
        if let Some(g) = got.get(rel)
            && g != w
        {
            let (w, g) = (String::from_utf8_lossy(w), String::from_utf8_lossy(g));
            let line = w
                .lines()
                .zip(g.lines())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| w.lines().count().min(g.lines().count()));
            out.push(format!(
                "{rel} differs at line {}: want {:?}, got {:?}",
                line + 1,
                w.lines().nth(line).unwrap_or("<end>"),
                g.lines().nth(line).unwrap_or("<end>")
            ));
        }
    }
    out.join("; ")
}
