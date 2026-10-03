//! `hermir profile …`: a copy's own settings for a session.
use hermir::{Error, Hermir, Install, Result};

use super::Outcome;
use crate::render;

fn best(h: &Hermir, emulator: &str) -> Result<Install> {
    h.emulator(emulator)?
        .best()?
        .ok_or_else(|| Error::NotInstalled(emulator.into()))
}

pub fn list(h: &Hermir, emulator: &str) -> Result<Outcome> {
    let copy = best(h, emulator)?;
    let names = h.emulator(emulator)?.profiles(&copy)?;
    let human = if names.is_empty() {
        "no profiles".into()
    } else {
        names.join("\n")
    };
    Ok(Outcome::new(&names, human))
}

/// `create`, or with `reset` make it again from scratch.
pub fn create(h: &Hermir, emulator: &str, name: &str, fresh: bool, reset: bool) -> Result<Outcome> {
    let copy = best(h, emulator)?;
    let e = h.emulator(emulator)?;
    let p = e.profile(&copy, name)?;
    if !p.supported() {
        return Err(Error::Invalid(format!(
            "{emulator} has no profiles: `config apply --profile` writes in place, snapshotted"
        )));
    }
    let steps = if reset {
        p.reset(fresh)?
    } else {
        p.create(fresh)?
    };
    let human = std::iter::once(p.dir().display().to_string())
        .chain(steps.iter().map(render::step))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Outcome::new(
        &serde_json::json!({ "profile": name, "dir": p.dir(), "steps": steps }),
        human,
    ))
}

pub fn remove(h: &Hermir, emulator: &str, name: &str) -> Result<Outcome> {
    let copy = best(h, emulator)?;
    let p = h.emulator(emulator)?.profile(&copy, name)?;
    p.remove()?;
    Ok(Outcome::new(
        &serde_json::json!({ "removed": name, "dir": p.dir() }),
        format!("{emulator}: profile {name} removed"),
    ))
}

pub fn where_(h: &Hermir, emulator: &str, name: &str) -> Result<Outcome> {
    let copy = best(h, emulator)?;
    let p = h.emulator(emulator)?.profile(&copy, name)?;
    Ok(Outcome::new(
        &serde_json::json!({ "profile": name, "dir": p.dir(), "exists": p.exists(),
                             "supported": p.supported() }),
        p.dir().display().to_string(),
    ))
}
