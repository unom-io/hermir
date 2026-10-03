//! `hermir pads`: the pads connected now, as SDL sees them, and `--pad auto`'s source.
use std::io::Write;

use hermir::{PadRef, Result};

use super::Outcome;

/// The pads connected now, in SDL's order.
#[cfg(feature = "enumerate")]
pub fn enumerate() -> Result<Vec<PadRef>> {
    hermir::pads::enumerate()
}

/// A hermir built without SDL cannot list pads: it says so.
#[cfg(not(feature = "enumerate"))]
pub fn enumerate() -> Result<Vec<PadRef>> {
    Err(hermir::Error::Invalid(
        "this hermir was built without pad enumeration (the `enumerate` feature); name the pads \
         with --pad VID:PID[:NAME][@INDEX]"
            .into(),
    ))
}

pub fn pads(json: bool, watch: bool) -> Result<Outcome> {
    let pads = enumerate()?;
    if !watch {
        return Ok(Outcome::new(&pads, human(&pads)));
    }
    let mut last = None;
    let mut stdout = std::io::stdout();
    loop {
        let pads = enumerate()?;
        if last.as_ref() != Some(&pads) {
            let text = if json {
                serde_json::to_string(&pads).unwrap_or_default()
            } else {
                format!("{}\n", human(&pads))
            };
            if writeln!(stdout, "{text}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                // Nobody is reading any more.
                return Ok(Outcome::new(&pads, String::new()));
            }
            last = Some(pads);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn human(pads: &[PadRef]) -> String {
    if pads.is_empty() {
        return "no pads connected".into();
    }
    pads.iter()
        .map(|p| {
            let mut line = format!(
                "{:<3}{:<28} {:04x}:{:04x}",
                p.index,
                p.sdl_name(),
                p.vendor,
                p.product
            );
            if p.name != p.sdl_name() {
                line.push_str(&format!("  {}", p.name));
            }
            if let Some(dev) = &p.evdev {
                line.push_str(&format!("  {}", dev.display()));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}
