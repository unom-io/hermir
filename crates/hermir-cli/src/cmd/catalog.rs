//! `hermir catalog …`
use std::path::Path;

use hermir::{Catalog, Hermir, Result};

use super::Outcome;

pub fn validate(dir: &Path) -> Result<Outcome> {
    let c = Catalog::from_dir(dir)?;
    let (entries, platforms) = (c.entries().len(), c.platforms().len());
    Ok(Outcome::new(
        &serde_json::json!({ "entries": entries, "platforms": platforms }),
        format!("ok: {entries} entries, {platforms} platforms"),
    ))
}

pub fn list(h: &Hermir) -> Outcome {
    let os = h.os();
    let rows: Vec<serde_json::Value> = h
        .catalog()
        .entries()
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id, "name": e.name, "platforms": e.platforms,
                "channel": e.channels.get(os).map(|c| c.kind()),
                "no_install": e.no_install,
            })
        })
        .collect();
    let human = h
        .catalog()
        .entries()
        .iter()
        .map(|e| {
            let channel =
                e.channels
                    .get(os)
                    .map(|c| c.kind())
                    .unwrap_or(if e.no_install.is_some() {
                        "no-install"
                    } else {
                        "-"
                    });
            format!(
                "{:<15} {:<22} {:<10} {}",
                e.id,
                e.name,
                channel,
                e.platforms.join(",")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Outcome::new(&rows, human)
}

pub fn show(h: &Hermir, emulator: &str) -> Result<Outcome> {
    let e = h.emulator(emulator)?.entry().clone();
    let text = serde_json::to_string_pretty(&e).unwrap_or_default();
    Ok(Outcome::new(&e, text))
}

/// Every entry asked for, resolved or with why not. A failure does not stop the others; the
/// exit code is the first failure's.
pub fn resolve(h: &Hermir, emulator: Option<String>, all: bool) -> Result<Outcome> {
    let ids: Vec<String> = match emulator {
        Some(id) if !all => vec![id],
        _ => h
            .catalog()
            .offered_on(h.os())
            .map(|e| e.id.clone())
            .collect(),
    };
    let mut rows = Vec::new();
    let mut lines = Vec::new();
    let mut failed: Vec<(String, i32)> = Vec::new();
    for id in ids {
        match h.emulator(&id)?.resolve() {
            Ok(r) => {
                lines.push(format!(
                    "{:<15} {:<8} {:<28} {}",
                    r.emulator,
                    r.channel,
                    r.release,
                    r.file_name.clone().unwrap_or_default()
                ));
                rows.push(serde_json::to_value(r).unwrap_or_default());
            }
            Err(e) => {
                lines.push(format!("{id:<15} FAILED   {e}"));
                rows.push(serde_json::json!({ "emulator": id, "error": e.to_string() }));
                failed.push((id, e.exit_code()));
            }
        }
    }
    let out = Outcome::new(&rows, lines.join("\n"));
    Ok(match failed.first() {
        Some((_, exit)) => {
            let names: Vec<&str> = failed.iter().map(|(id, _)| id.as_str()).collect();
            out.failed(*exit, format!("did not resolve: {}", names.join(", ")))
        }
        None => out,
    })
}
