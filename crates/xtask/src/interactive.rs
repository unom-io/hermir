//! `cargo xtask capture <emu> --interactive --session <s>`: what the emulator itself writes
//! when a person sets the session's settings in its UI, as `after/<session>/`. The golden
//! tests then hold hermir to it (check E). It runs on this desktop, in a home of its own: the
//! person's own settings are never read or touched.
use std::io::Write as _;

use hermir::Entry;
use hermir_golden::check::{self, tree, write_tree};
use hermir_golden::fixture::{Fixture, Session};

use crate::capture::{self, Run};

pub fn capture(entry: &Entry, session: &str, run: Run) -> Result<(), String> {
    // The fixture for the current release: made headless first if it does not exist.
    let first = capture::first_start(
        entry,
        Run {
            interactive: false,
            host: true,
            ..run
        },
    )?;
    let dir = capture::write_fixture(entry, &first, false)?;
    let fx = Fixture::load(&dir)?;
    let s = fx
        .session(session)
        .map_err(|e| format!("{e}; the fixture's sessions are {}", fx.sessions.join(", ")))?;
    let recipe = capture::recipe(&entry.id)?;
    let steps = recipe
        .checklists
        .get(session)
        .cloned()
        .unwrap_or_else(|| checklist(&s));
    println!(
        "{} {} will open with its own fresh settings. In its UI:",
        entry.name, fx.meta.version
    );
    for (i, step) in steps.iter().enumerate() {
        println!("  {}. {step}", i + 1);
    }
    println!("then quit it from its menu (not by closing the terminal).");
    print!("Press Enter to start it… ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);

    let before = tree(&dir.join("before"));
    let after = capture::start(entry, run, Some(&before))?;
    let changed: hermir_golden::check::Tree = after
        .files
        .iter()
        .filter(|(rel, bytes)| before.get(*rel) != Some(*bytes))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if changed.is_empty() {
        return Err("it wrote nothing new: were the settings saved before it quit?".into());
    }
    let out = dir.join("after").join(session);
    let _ = std::fs::remove_dir_all(&out);
    write_tree(&out, &changed)?;
    println!("wrote {}:", out.display());
    for rel in changed.keys() {
        println!("  {rel}");
    }
    // Hold hermir to it now: check E compares what each wrote, key by key.
    match check::run(&Fixture::load(&dir)?, session, false) {
        Ok(()) => println!("hermir writes what {} wrote: check E passes", entry.name),
        Err(e) => println!(
            "hermir differs from {} (fix the catalog, then rerun):\n{e}",
            entry.name
        ),
    }
    Ok(())
}

/// What to set, from the session itself, when the recipe has no menu paths for it.
fn checklist(s: &Session) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(v) = &s.patch.video {
        if let Some(f) = v.fullscreen {
            out.push(format!(
                "start in fullscreen: {}",
                if f { "on" } else { "off" }
            ));
        }
        if let Some(n) = v.scale {
            out.push(format!(
                "internal resolution: {n}× the console's (native when 1)"
            ));
        }
        if let Some(f) = v.vsync {
            out.push(format!("vsync: {}", if f { "on" } else { "off" }));
        }
        if let Some(a) = v.aspect {
            out.push(format!("aspect ratio: {}", a.as_str()));
        }
    }
    if let Some(r) = s.patch.region {
        out.push(format!("console region: {}", r.as_str()));
    }
    for p in s.patch.players.iter().flatten() {
        out.push(format!(
            "player {}: bind every button of the pad {} ({:04x}:{:04x})",
            p.seat, p.pad.name, p.pad.vendor, p.pad.product
        ));
    }
    for n in &s.patch.native {
        out.push(format!("set [{}] {} to {}", n.section, n.key, n.value));
    }
    out
}
