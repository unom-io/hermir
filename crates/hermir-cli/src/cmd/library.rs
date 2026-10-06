//! `hermir catalog registry`, `saves list|export|import`, `content`, `firmware`, `adopt`,
//! `core list`: what a game library asks of hermir.
use std::path::{Path, PathBuf};

use hermir::{Error, Hermir, Install, Result, SaveKind};

use super::Outcome;

pub fn registry(h: &Hermir) -> Outcome {
    let r = h.registry();
    let human = format!(
        "{} platforms, {} emulators",
        r.platforms.len(),
        r.emulators.len()
    );
    Outcome::new(&r, human)
}

fn best(h: &Hermir, emulator: &str) -> Result<Install> {
    h.emulator(emulator)?
        .best()?
        .ok_or_else(|| Error::NotInstalled(emulator.into()))
}

fn kind(s: &str) -> Result<SaveKind> {
    match s {
        "save" => Ok(SaveKind::Save),
        "memcard" => Ok(SaveKind::Memcard),
        "state" => Ok(SaveKind::State),
        _ => Err(Error::Invalid(format!("{s}: save, memcard or state"))),
    }
}

pub fn units(h: &Hermir, emulator: &str, platform: &str, game: Option<&Path>) -> Result<Outcome> {
    let copy = best(h, emulator)?;
    let units = h.emulator(emulator)?.units(&copy, platform, game)?;
    let human = if units.is_empty() {
        "no saves yet".into()
    } else {
        units
            .iter()
            .map(|u| {
                format!(
                    "{:<8} {}{}  {} B",
                    format!("{:?}", u.kind).to_lowercase(),
                    u.name,
                    if u.shared { " (shared)" } else { "" },
                    u.size
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Outcome::new(&units, human))
}

pub fn export(
    h: &Hermir,
    emulator: &str,
    platform: &str,
    game: Option<&Path>,
    k: &str,
    name: &str,
    out: &Path,
) -> Result<Outcome> {
    let copy = best(h, emulator)?;
    let done = h
        .emulator(emulator)?
        .export_unit(&copy, platform, game, kind(k)?, name, out)?;
    let human = format!("{}  {}  md5 {}", done.file.display(), done.size, done.md5);
    Ok(Outcome::new(&done, human))
}

#[allow(clippy::too_many_arguments)]
pub fn import(
    h: &Hermir,
    emulator: &str,
    platform: &str,
    game: Option<&Path>,
    k: &str,
    name: &str,
    file: &Path,
    others: &[String],
) -> Result<Outcome> {
    let copy = best(h, emulator)?;
    let step = h.emulator(emulator)?.import_unit(
        &copy,
        platform,
        game,
        &[kind(k)?],
        name,
        file,
        others,
    )?;
    let human = format!(
        "{:?} {}{}",
        step.outcome,
        step.target.display(),
        step.note
            .as_deref()
            .map(|n| format!(" — {n}"))
            .unwrap_or_default()
    );
    Ok(Outcome::new(&step, human))
}

pub fn content(
    h: &Hermir,
    emulator: &str,
    platform: &str,
    k: &str,
    files: &[PathBuf],
) -> Result<Outcome> {
    let copy = best(h, emulator)?;
    let steps = h
        .emulator(emulator)?
        .install_content(&copy, platform, k, files)?;
    let human = steps
        .iter()
        .map(|s| {
            format!(
                "{:?} {} → {}{}",
                s.outcome,
                s.source.display(),
                s.target.display(),
                s.note
                    .as_deref()
                    .map(|n| format!(" — {n}"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Outcome::new(&steps, human))
}

pub fn firmware(h: &Hermir, emulator: &str, platform: &str) -> Result<Outcome> {
    let copy = best(h, emulator)?;
    let status = h.emulator(emulator)?.firmware_status(&copy, platform)?;
    let human = match &status {
        None => format!("{emulator} needs no firmware for {platform}"),
        Some(s) => {
            let mut lines = vec![format!(
                "{}: {}",
                platform,
                if s.ok { "ready" } else { "missing" }
            )];
            for f in &s.found {
                lines.push(format!(
                    "  {}  {}",
                    f.path.display(),
                    match f.known {
                        Some(true) => "known good dump",
                        Some(false) => "not a known dump",
                        None => "",
                    }
                ));
            }
            lines.join("\n")
        }
    };
    Ok(Outcome::new(&status, human))
}

pub fn adopt(h: &Hermir, emulator: &str, exe: &Path, forget: bool) -> Result<Outcome> {
    let copy = h.emulator(emulator)?.adopt(exe, !forget)?;
    let human = match &copy {
        Some(i) => format!("{} adopted: {}", emulator, i.exe),
        None => format!("{} forgotten", exe.display()),
    };
    Ok(Outcome::new(&copy, human))
}

pub fn cores(h: &Hermir) -> Result<Outcome> {
    let e = h.emulator("retroarch")?;
    let copy = e
        .best()?
        .ok_or_else(|| Error::NotInstalled("retroarch".into()))?;
    let cores = e.cores(&copy);
    Ok(Outcome::new(&cores, cores.join("\n")))
}
