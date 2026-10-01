//! PCSX2 and DuckStation share one input core: `SDL-<index>/<control>` per emulated button
//! in `[Pad<n>]`, with the face buttons named differently in each.
use super::{Bindings, Cx, Plan, set};
use crate::model::Player;

pub(super) struct Names {
    pub south: &'static str,
    pub east: &'static str,
    pub west: &'static str,
    pub north: &'static str,
}

pub(super) fn players(cx: &Cx, kind: &str, names: &Names, players: &[Player]) -> Plan {
    let mut edits = set(cx, "main", "InputSources", "SDL", "true")?;
    for p in players {
        let sec = format!("Pad{}", p.seat);
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
    Ok(Bindings { edits, note: None })
}
