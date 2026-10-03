//! `hermir install`, `update`, `remove`, `core install`: getting emulators.
use hermir::{Hermir, Installed, Result};
use serde::Serialize;

use super::Outcome;
use crate::render::{self, Stderr};

pub fn install(h: &Hermir, emulator: &str, json: bool) -> Result<Outcome> {
    let e = h.emulator(emulator)?;
    if !json {
        eprintln!("installing {}", e.entry().name);
    }
    let row = e.install(&Stderr)?;
    let mut human = format!("{}: {}", row.emulator, row.exe);
    if let Some(kept) = render::kept(&row) {
        human.push('\n');
        human.push_str(&kept);
    }
    Ok(Outcome::new(&row, human))
}

#[derive(Serialize)]
struct Updated {
    emulator: String,
    /// The new install; `null` when it was current.
    updated: Option<Installed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// One emulator, or every managed one. With `--all`, a failure does not stop the others; the
/// exit code is the first failure's.
pub fn update(h: &Hermir, emulator: Option<String>, all: bool) -> Result<Outcome> {
    let ids: Vec<String> = match emulator {
        Some(id) if !all => vec![id],
        _ => h
            .store()
            .installed()?
            .into_iter()
            .map(|r| r.emulator)
            .collect(),
    };
    let mut rows = Vec::new();
    let mut lines = Vec::new();
    let mut failed: Vec<(String, i32)> = Vec::new();
    for id in ids {
        let done = h.emulator(&id).and_then(|e| e.update(&Stderr));
        match done {
            Ok(Some(row)) => {
                lines.push(format!(
                    "{id}: updated to {}",
                    row.release
                        .clone()
                        .or_else(|| row.version.clone())
                        .unwrap_or_default()
                ));
                lines.extend(render::kept(&row));
                rows.push(Updated {
                    emulator: id,
                    updated: Some(row),
                    error: None,
                });
            }
            Ok(None) => {
                lines.push(format!("{id}: current"));
                rows.push(Updated {
                    emulator: id,
                    updated: None,
                    error: None,
                });
            }
            Err(e) if all => {
                lines.push(format!("{id}: FAILED {e}"));
                failed.push((id.clone(), e.exit_code()));
                rows.push(Updated {
                    emulator: id,
                    updated: None,
                    error: Some(e.to_string()),
                });
            }
            Err(e) => return Err(e),
        }
    }
    let out = Outcome::new(&rows, lines.join("\n"));
    Ok(match failed.first() {
        Some((_, exit)) => {
            let names: Vec<&str> = failed.iter().map(|(id, _)| id.as_str()).collect();
            out.failed(*exit, format!("not updated: {}", names.join(", ")))
        }
        None => out,
    })
}

pub fn remove(h: &Hermir, emulator: &str, purge: bool) -> Result<Outcome> {
    h.emulator(emulator)?.remove(purge)?;
    Ok(Outcome::new(
        &serde_json::json!({ "removed": emulator, "purged": purge }),
        format!("{emulator}: removed"),
    ))
}

pub fn core(h: &Hermir, core: &str) -> Result<Outcome> {
    let path = h.install_core(core, &Stderr)?;
    Ok(Outcome::new(
        &serde_json::json!({ "core": core, "path": path }),
        path.display().to_string(),
    ))
}
