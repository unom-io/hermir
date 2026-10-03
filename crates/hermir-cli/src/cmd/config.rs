//! `hermir config …`, `players`, `prepare`: a session's settings into every copy, and back.
use std::path::PathBuf;

use hermir::{
    Applied, Audio, Error, Hermir, Patch, PrepareStep, Prepared, Result, StepOutcome, Video,
};
use serde::Serialize;

use super::{Outcome, PerCopy, copies};
use crate::cli::{ApplyArgs, PadArgs};
use crate::{parse, render};

/// Exit codes for a step that failed: a config file (7), firmware placed or installed (5).
const CONFIG_FAILED: i32 = 7;
const PLACE_FAILED: i32 = 5;

pub fn apply(h: &Hermir, a: &ApplyArgs) -> Result<Outcome> {
    let video = Video {
        fullscreen: a.fullscreen,
        scale: a.scale,
        vsync: a.vsync,
        aspect: a.aspect,
    };
    let audio = Audio {
        device: a.audio_device.clone(),
        latency_ms: a.audio_latency,
    };
    let seats = parse::seats(&a.pads)?;
    let patch = Patch {
        audio: (!audio.is_empty()).then_some(audio),
        players: seats.players,
        connected: seats.connected,
        video: (!video.is_empty()).then_some(video),
        region: a.region,
        native: a
            .set
            .iter()
            .map(|s| parse::native(&a.file, s))
            .collect::<Result<_>>()?,
    };
    if patch.is_empty() {
        return Err(Error::Invalid(
            "nothing to apply; see `hermir config apply --help`".into(),
        ));
    }
    apply_patch(h, &a.emulator, &patch, a.profile.as_deref())
}

/// `players`: `config apply` with pads only, or `config revert`.
pub fn players(h: &Hermir, emulator: &str, revert: bool, pads: &PadArgs) -> Result<Outcome> {
    if revert {
        return self::revert(h, Some(emulator.to_string()), false, false);
    }
    let seats = parse::seats(pads)?;
    let players = seats.players.unwrap_or_else(|| {
        vec![hermir::Player {
            seat: 1,
            pad: hermir::PadRef::xbox360(0),
        }]
    });
    let patch = Patch {
        players: Some(players),
        connected: seats.connected,
        ..Default::default()
    };
    apply_patch(h, emulator, &patch, None)
}

fn apply_patch(
    h: &Hermir,
    emulator: &str,
    patch: &Patch,
    profile: Option<&str>,
) -> Result<Outcome> {
    let e = h.emulator(emulator)?;
    let copies = copies(h, emulator)?;
    let done: Vec<Applied> = copies
        .iter()
        .map(|i| match profile {
            Some(name) => e.profile(i, name)?.apply(patch),
            None => e.apply(i, patch),
        })
        .collect::<Result<_>>()?;
    let rows: Vec<PerCopy<'_, Applied>> = copies
        .iter()
        .zip(&done)
        .map(|(install, result)| PerCopy { install, result })
        .collect();
    let human = copies
        .iter()
        .zip(&done)
        .flat_map(|(i, a)| {
            std::iter::once(i.exe.to_string())
                .chain(a.note.iter().map(|n| format!("  {n}")))
                .chain(a.knobs.iter().map(render::knob))
                .chain(a.steps.iter().map(render::step))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let out = Outcome::new(&rows, human);
    Ok(if done.iter().any(Applied::failed) {
        out.failed(CONFIG_FAILED, "a config file could not be written")
    } else {
        out
    })
}

#[derive(Serialize)]
struct Reverted {
    emulator: String,
    steps: Vec<PrepareStep>,
}

pub fn revert(h: &Hermir, emulator: Option<String>, all: bool, force: bool) -> Result<Outcome> {
    let done: Vec<Reverted> = match emulator {
        Some(id) if !all => vec![Reverted {
            steps: h.emulator(&id)?.revert(force)?,
            emulator: id,
        }],
        _ => h
            .revert_all(force)?
            .into_iter()
            .map(|(emulator, steps)| Reverted { emulator, steps })
            .collect(),
    };
    let human = if done.iter().all(|r| r.steps.is_empty()) {
        "nothing outstanding".into()
    } else {
        done.iter()
            .flat_map(|r| {
                std::iter::once(r.emulator.clone()).chain(r.steps.iter().map(render::step))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let failed = done
        .iter()
        .flat_map(|r| &r.steps)
        .any(|s| matches!(s.outcome, StepOutcome::Failed | StepOutcome::Conflict));
    let out = Outcome::new(&done, human);
    Ok(if failed {
        out.failed(
            CONFIG_FAILED,
            "a file was not restored; its snapshot is kept for the next revert",
        )
    } else {
        out
    })
}

pub fn support(h: &Hermir, emulator: Option<&str>) -> Result<Outcome> {
    let matrix: Vec<(String, Vec<hermir::KnobChange>)> = match emulator {
        Some(id) => vec![(id.to_string(), h.emulator(id)?.support())],
        None => h.support(),
    };
    let rows: Vec<serde_json::Value> = matrix
        .iter()
        .map(|(id, knobs)| serde_json::json!({ "emulator": id, "knobs": knobs }))
        .collect();
    let human = if let ([(id, knobs)], Some(_)) = (matrix.as_slice(), emulator) {
        std::iter::once(id.clone())
            .chain(knobs.iter().map(|k| {
                format!(
                    "  {} {:<17}{}",
                    render::support_mark(k.support),
                    k.knob,
                    k.note
                        .as_deref()
                        .map(|n| format!(" {n}"))
                        .unwrap_or_default()
                )
            }))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        let names: Vec<&str> = matrix
            .first()
            .map(|(_, k)| k.iter().map(|k| k.knob.as_str()).collect())
            .unwrap_or_default();
        let mut lines = vec![format!(
            "{:<15} {}",
            "",
            names
                .iter()
                .map(|n| format!("{:<11}", n.trim_start_matches("video.")))
                .collect::<String>()
        )];
        for (id, knobs) in &matrix {
            lines.push(format!(
                "{:<15} {}",
                id,
                knobs
                    .iter()
                    .map(|k| format!("{:<11}", render::support_mark(k.support)))
                    .collect::<String>()
            ));
        }
        lines.push(
            "+ applied   ~ partial   - unsupported; `config support <emulator>` says why".into(),
        );
        lines.join("\n")
    };
    Ok(Outcome::new(&rows, human))
}

pub fn prepare(
    h: &Hermir,
    emulator: &str,
    platform: Option<&str>,
    firmware: &[PathBuf],
) -> Result<Outcome> {
    let e = h.emulator(emulator)?;
    let copies = copies(h, emulator)?;
    let done: Vec<Prepared> = copies
        .iter()
        .map(|i| e.prepare(i, platform, firmware))
        .collect::<Result<_>>()?;
    let rows: Vec<PerCopy<'_, Prepared>> = copies
        .iter()
        .zip(&done)
        .map(|(install, result)| PerCopy { install, result })
        .collect();
    let human = copies
        .iter()
        .zip(&done)
        .flat_map(|(i, p)| {
            std::iter::once(i.exe.to_string()).chain(p.steps.iter().map(render::step))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let failed = done
        .iter()
        .flat_map(|p| &p.steps)
        .any(|s| s.outcome == StepOutcome::Failed);
    let out = Outcome::new(&rows, human);
    Ok(if failed {
        out.failed(PLACE_FAILED, "a step failed")
    } else {
        out
    })
}

#[derive(Serialize)]
struct Native<'a> {
    file: &'a str,
    section: &'a str,
    key: &'a str,
    value: Option<String>,
}

/// What every copy holds: all knobs, one knob, or one native key.
pub fn get(
    h: &Hermir,
    emulator: &str,
    what: Option<&str>,
    file: &str,
    profile: Option<&str>,
) -> Result<Outcome> {
    let e = h.emulator(emulator)?;
    let copies = copies(h, emulator)?;
    let native = what.filter(|w| !hermir::KNOBS.contains(w));
    if let Some(spec) = native {
        let (section, key) = spec.rsplit_once('/').unwrap_or(("", spec));
        let mut rows = Vec::new();
        let mut lines = Vec::new();
        for i in &copies {
            let value = e.get_native(i, file, section, key)?;
            lines.push(format!(
                "{}\n  {spec} = {}",
                i.exe,
                value.as_deref().unwrap_or("(not set)")
            ));
            rows.push(serde_json::json!({
                "install": i,
                "native": Native { file, section, key, value },
            }));
        }
        return Ok(Outcome::new(&rows, lines.join("\n")));
    }
    let mut rows = Vec::new();
    let mut lines = Vec::new();
    for i in &copies {
        let read = match profile {
            Some(name) => e.profile(i, name)?.get()?,
            None => e.get(i)?,
        };
        let knobs: Vec<hermir::KnobValue> = read
            .into_iter()
            .filter(|k| what.is_none_or(|w| k.knob == w))
            .collect();
        lines.push(i.exe.to_string());
        for k in &knobs {
            lines.push(format!(
                "  {:<17}{}{}",
                k.knob,
                k.value.as_deref().or(k.literal.as_deref()).unwrap_or("-"),
                k.note
                    .as_deref()
                    .map(|n| format!(" — {n}"))
                    .unwrap_or_default()
            ));
        }
        rows.push(serde_json::json!({ "install": i, "knobs": knobs }));
    }
    Ok(Outcome::new(&rows, lines.join("\n")))
}
