//! `hermir status`, `detect`, `where`, `doctor`: what is on this machine.
use std::path::Path;

use hermir::{Error, Hermir, Os, Result};

use super::Outcome;
use crate::render;

pub fn status(h: &Hermir, emulator: Option<String>, check: bool) -> Result<Outcome> {
    let rows = h.status(emulator.as_deref(), check)?;
    let human = rows
        .iter()
        .filter(|s| s.managed.is_some() || !s.detected.is_empty() || emulator.is_some())
        .map(|s| {
            let mut line = format!("{:<15} {}", s.emulator, s.name);
            if let Some(m) = &s.managed {
                line.push_str(&format!(
                    "\n  managed  {}{}",
                    m.exe,
                    m.version
                        .as_ref()
                        .map(|v| format!("  v{v}"))
                        .unwrap_or_default()
                ));
                if let Some(u) = &s.update {
                    line.push_str(&format!("  → update: {u}"));
                }
                if let Some(e) = &s.update_error {
                    line.push_str(&format!("\n  could not check for an update: {e}"));
                }
            }
            for d in &s.detected {
                line.push_str(&format!("\n  {:<8} {}", render::kind(d.kind), d.exe));
            }
            if s.managed.is_none() && s.detected.is_empty() {
                line.push_str(if s.offered {
                    "\n  not installed"
                } else {
                    "\n  not offered here"
                });
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Outcome::new(&rows, human))
}

pub fn detect(h: &Hermir) -> Outcome {
    let found = h.detect();
    let human = if found.is_empty() {
        "nothing detected".into()
    } else {
        found
            .iter()
            .map(|i| format!("{:<15} {:<9} {}", i.emulator, render::kind(i.kind), i.exe))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Outcome::new(&found, human)
}

pub fn where_(h: &Hermir, emulator: &str) -> Result<Outcome> {
    let i = h
        .emulator(emulator)?
        .best()?
        .ok_or_else(|| Error::NotInstalled(emulator.into()))?;
    let human = format!(
        "{}\nconfig: {}",
        i.exe,
        i.config_root
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    Ok(Outcome::new(&i, human))
}

pub fn doctor(h: &Hermir) -> Result<Outcome> {
    let os = h.os();
    let prefix = h.store().root().to_path_buf();
    let writable = writable(&prefix);
    let flatpak = os != Os::Linux || which("flatpak");
    let report = serde_json::json!({
        "os": os, "prefix": prefix, "prefix_writable": writable,
        "flatpak": if os == Os::Linux { Some(flatpak) } else { None },
        "catalog_entries": h.catalog().entries().len(),
        "offered_here": h.catalog().offered_on(os).count(),
        "managed": h.store().installed()?.len(),
        "detected": h.detect().len(),
        "github_token": std::env::var("GITHUB_TOKEN").is_ok_and(|t| !t.is_empty()),
    });
    let mut s = format!(
        "os: {os}\nprefix: {} ({})",
        prefix.display(),
        if writable { "writable" } else { "NOT writable" }
    );
    if os == Os::Linux {
        s.push_str(if flatpak {
            "\nflatpak: found"
        } else {
            "\nflatpak: missing — the Linux channel needs it"
        });
    }
    s.push_str(&format!(
        "\ncatalog: {} entries, {} offered here\nmanaged: {}, detected: {}",
        report["catalog_entries"], report["offered_here"], report["managed"], report["detected"]
    ));
    let out = Outcome::new(&report, s);
    Ok(if writable {
        out
    } else {
        out.failed(1, format!("{} cannot be written", prefix.display()))
    })
}

/// Whether hermir could write `prefix`: the prefix itself when it exists, else the nearest
/// folder above it that does. Nothing is left behind.
fn writable(prefix: &Path) -> bool {
    let Some(dir) = prefix.ancestors().find(|p| p.is_dir()) else {
        return false;
    };
    let probe = dir.join(format!(".hermir-doctor-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}
