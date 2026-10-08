//! PCSX2 and DuckStation share one input core: `SDL-<index>/<control>` per emulated button
//! in `[Pad<n>]`, with the face buttons named differently in each. Pads 1 and 2 are the ports,
//! 3–5 port 1's multitap and 6–8 port 2's, so a third player turns port 1's on. A pad with a
//! type is plugged in whether or not a device is behind it: a seat without one has none.
use super::{Bindings, Cx, Plan, beyond, set};
use crate::model::Player;

pub(super) struct Names {
    pub south: &'static str,
    pub east: &'static str,
    pub west: &'static str,
    pub north: &'static str,
}

/// Where each emulator turns a port's multitap on.
pub(super) enum Multitap {
    /// `[Pad] MultitapPort1/2 = true|false`.
    Pcsx2,
    /// `[ControllerPorts] MultitapMode = Disabled|Port1Only|BothPorts`.
    DuckStation,
}

pub(super) fn players(
    cx: &Cx,
    kind: &str,
    names: &Names,
    tap: Multitap,
    players: &[Player],
) -> Plan {
    let mut edits = set(cx, "main", "InputSources", "SDL", "true")?;
    let top = players
        .iter()
        .map(|p| p.seat)
        .filter(|s| *s <= 8)
        .max()
        .unwrap_or(0);
    match tap {
        Multitap::Pcsx2 => {
            let on = |b: bool| if b { "true" } else { "false" };
            edits.extend(set(cx, "main", "Pad", "MultitapPort1", on(top >= 3))?);
            edits.extend(set(cx, "main", "Pad", "MultitapPort2", on(top >= 6))?);
        }
        Multitap::DuckStation => {
            let mode = match top {
                0..=2 => "Disabled",
                3..=5 => "Port1Only",
                _ => "BothPorts",
            };
            edits.extend(set(cx, "main", "ControllerPorts", "MultitapMode", mode)?);
        }
    }
    for seat in 1..=8u8 {
        let sec = format!("Pad{seat}");
        let Some(p) = players.iter().find(|p| p.seat == seat) else {
            edits.extend(set(cx, "main", &sec, "Type", "None")?);
            continue;
        };
        let d = format!("SDL-{}", p.pad.index);
        let binds = [
            ("Type", kind.to_string()),
            ("Up", format!("{d}/DPadUp")),
            ("Right", format!("{d}/DPadRight")),
            ("Down", format!("{d}/DPadDown")),
            ("Left", format!("{d}/DPadLeft")),
            ("Triangle", format!("{d}/{}", names.north)),
            ("Circle", format!("{d}/{}", names.east)),
            ("Cross", format!("{d}/{}", names.south)),
            ("Square", format!("{d}/{}", names.west)),
            ("Select", format!("{d}/Back")),
            ("Start", format!("{d}/Start")),
            ("L1", format!("{d}/LeftShoulder")),
            ("L2", format!("{d}/+LeftTrigger")),
            ("R1", format!("{d}/RightShoulder")),
            ("R2", format!("{d}/+RightTrigger")),
            ("L3", format!("{d}/LeftStick")),
            ("R3", format!("{d}/RightStick")),
            ("Analog", format!("{d}/Guide")),
            ("LUp", format!("{d}/-LeftY")),
            ("LRight", format!("{d}/+LeftX")),
            ("LDown", format!("{d}/+LeftY")),
            ("LLeft", format!("{d}/-LeftX")),
            ("RUp", format!("{d}/-RightY")),
            ("RRight", format!("{d}/+RightX")),
            ("RDown", format!("{d}/+RightY")),
            ("RLeft", format!("{d}/-RightX")),
            ("LargeMotor", format!("{d}/LargeMotor")),
            ("SmallMotor", format!("{d}/SmallMotor")),
        ];
        for (k, v) in binds {
            edits.extend(set(cx, "main", &sec, k, &v)?);
        }
    }
    Ok(Bindings {
        edits,
        note: beyond(players, 8, "a multitap"),
    })
}
