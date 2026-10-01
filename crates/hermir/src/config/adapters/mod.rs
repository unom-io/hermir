//! Player bindings are where the formats stop being flat: each emulator keys a pad its own
//! way and lays buttons out its own way, so each has a function from pads to edits here, one
//! file per emulator. The catalog's `config.players` says which emulator uses which adapter;
//! an emulator that maps SDL pads itself, or has no pads, is data there and never reaches
//! this module.
//!
//! SDL game-controller numbering: buttons A 0, B 1, X 2, Y 3, Back 4, Guide 5, Start 6,
//! LS 7, RS 8, LB 9, RB 10, D-pad up 11 down 12 left 13 right 14; axes LX 0, LY 1, RX 2,
//! RY 3, LT 4, RT 5. Raw evdev/joystick order of the same pad: buttons A 0, B 1, X 2, Y 3,
//! LB 4, RB 5, Back 6, Start 7, Guide 8, LS 9, RS 10; axes X 0, Y 1, LT 2, RX 3, RY 4, RT 5,
//! hat X 6, hat Y 7. Nintendo layouts are mapped by position, so a face button keeps its place.
mod azahar;
mod cemu;
mod dolphin;
mod duckstation;
mod eden;
mod melonds;
mod pad_ini;
mod pcsx2;
mod retroarch;
mod rpcs3;
mod supermodel;
mod xemu;

use super::{Cx, Edit};
use crate::model::{Entry, KnobChange, Os, Player, PlayersSupport, Support};

/// The adapters that exist, as `config.players.adapter` names them.
pub const ADAPTERS: &[&str] = &[
    "azahar",
    "cemu",
    "dolphin",
    "duckstation",
    "eden",
    "melonds",
    "pcsx2",
    "retroarch",
    "rpcs3",
    "supermodel",
    "xemu",
];

pub fn exists(adapter: &str) -> bool {
    ADAPTERS.contains(&adapter)
}

/// What an adapter plans: the edits, and a caveat when the result is only partly what was
/// asked (a seat beyond the emulator's ports, an identity that is a best effort on this OS).
pub(crate) struct Bindings {
    pub edits: Vec<Edit>,
    pub note: Option<String>,
}

/// `Err` is why nothing can be written, in a phrase a UI shows.
pub(crate) type Plan = Result<Bindings, String>;

/// Plans `players` for `entry` on this copy: a `KnobChange` named `players` and the edits.
pub(crate) fn plan(entry: &Entry, cx: &Cx, players: &[Player]) -> (KnobChange, Vec<Edit>) {
    let change = |support: Support, note: Option<String>, file| KnobChange {
        knob: "players".into(),
        support,
        note,
        file,
    };
    let Some(how) = entry.config.as_ref().and_then(|c| c.players.as_ref()) else {
        return (
            change(
                Support::Unsupported,
                Some(format!(
                    "player bindings are not described for {} yet",
                    entry.id
                )),
                None,
            ),
            Vec::new(),
        );
    };
    let adapter = match how {
        PlayersSupport::Automatic { automatic } => {
            return (
                change(Support::Partial, Some(automatic.clone()), None),
                Vec::new(),
            );
        }
        PlayersSupport::Unsupported { unsupported } => {
            return (
                change(Support::Unsupported, Some(unsupported.clone()), None),
                Vec::new(),
            );
        }
        PlayersSupport::Adapter { adapter } => adapter.as_str(),
    };
    let planned = match adapter {
        "azahar" => azahar::players(cx, players),
        "cemu" => cemu::players(cx, players),
        "dolphin" => dolphin::players(cx, players),
        "duckstation" => duckstation::players(cx, players),
        "eden" => eden::players(cx, players),
        "melonds" => melonds::players(cx, players),
        "pcsx2" => pcsx2::players(cx, players),
        "retroarch" => retroarch::players(cx, players),
        "rpcs3" => rpcs3::players(cx, players),
        "supermodel" => supermodel::players(cx, players),
        "xemu" => xemu::players(cx, players),
        other => Err(format!("no adapter named {other}")),
    };
    match planned {
        Ok(b) => {
            let file = b.edits.first().map(|e| e.file().to_path_buf());
            let support = if b.note.is_some() {
                Support::Partial
            } else {
                Support::Applied
            };
            (change(support, b.note, file), b.edits)
        }
        Err(why) => (change(Support::Unsupported, Some(why), None), Vec::new()),
    }
}

/// `key` in `section` of the catalog's `file`, spelled for its format.
fn set(cx: &Cx, file: &str, section: &str, key: &str, value: &str) -> Result<Vec<Edit>, String> {
    let (path, f) = cx.file(file)?;
    Ok(Edit::set(&path, f, section, key, value))
}

/// Seats past `ports`, as a note, or nothing.
fn beyond(players: &[Player], ports: u8, name: &str) -> Option<String> {
    let over: Vec<String> = players
        .iter()
        .filter(|p| p.seat > ports)
        .map(|p| p.seat.to_string())
        .collect();
    (!over.is_empty()).then(|| {
        format!(
            "{name} has {ports} ports; seat {} left out",
            over.join(", ")
        )
    })
}

/// The caveat for emulators that key a pad by SDL's GUID, which on Windows depends on the
/// driver the pad comes through (XInput, HIDAPI, DirectInput), not on its USB identity alone.
fn guid_note(os: Os) -> Option<String> {
    (os == Os::Windows).then(|| {
        "on Windows SDL derives a pad's GUID from the driver it comes through; the USB-derived \
         GUID is a best effort"
            .into()
    })
}

fn join(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, None) => a,
        (None, b) => b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    #[test]
    fn every_adapter_is_named_by_exactly_one_entry_and_every_named_adapter_exists() {
        let c = Catalog::embedded().unwrap();
        let named: Vec<&str> = c
            .entries()
            .iter()
            .filter_map(|e| e.config.as_ref()?.players.as_ref())
            .filter_map(|p| match p {
                PlayersSupport::Adapter { adapter } => Some(adapter.as_str()),
                _ => None,
            })
            .collect();
        for a in ADAPTERS {
            assert_eq!(
                named.iter().filter(|n| n == &a).count(),
                1,
                "adapter {a} must be named by one catalog entry"
            );
        }
        for n in &named {
            assert!(exists(n), "{n} is named by the catalog but has no code");
        }
    }
}
