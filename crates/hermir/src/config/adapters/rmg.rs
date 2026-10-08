//! RMG (Rosalie's Mupen GUI): an N64 pad per seat in `mupen64plus.cfg`, section
//! `Rosalie's Mupen GUI - Input Plugin Profile <i>`, on device type Automatic (2), which takes
//! the first free SDL gamepad. Each control is an `InputType`/`Data`/`ExtraData` list: a
//! gamepad button (0) or axis (1) by SDL's game-controller enum, an axis's half 0 for minus
//! and 1 for plus. A profile without its section, or one left plugged in, is a fourth player.
use super::{Bindings, Cx, Plan, Seating, beyond, set};

/// (N64 control, input type, SDL button or axis, axis half).
const MAP: &[(&str, u8, u8, u8)] = &[
    ("A", 0, 0, 0),
    ("B", 0, 2, 0),
    ("Start", 0, 6, 0),
    ("DpadUp", 0, 11, 0),
    ("DpadDown", 0, 12, 0),
    ("DpadLeft", 0, 13, 0),
    ("DpadRight", 0, 14, 0),
    ("CButtonUp", 1, 3, 0),
    ("CButtonDown", 1, 3, 1),
    ("CButtonLeft", 1, 2, 0),
    ("CButtonRight", 1, 2, 1),
    ("LeftTrigger", 0, 9, 0),
    ("RightTrigger", 0, 10, 0),
    ("ZTrigger", 1, 4, 1),
    ("AnalogStickUp", 1, 1, 0),
    ("AnalogStickDown", 1, 1, 1),
    ("AnalogStickLeft", 1, 0, 0),
    ("AnalogStickRight", 1, 0, 1),
];

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let q = |v: &str| format!("\"{v}\"");
    let mut edits = Vec::new();
    for i in 0..4u8 {
        let sec = format!("Rosalie's Mupen GUI - Input Plugin Profile {i}");
        let mut key = |k: &str, v: &str| -> Result<(), String> {
            edits.extend(set(cx, "main", &sec, k, v)?);
            Ok(())
        };
        if !players.iter().any(|p| p.seat == i + 1) {
            key("PluggedIn", "False")?;
            continue;
        }
        key("PluggedIn", "True")?;
        key("DeviceType", "2")?;
        // A named user profile would replace this section.
        key("UseProfile", &q(""))?;
        key("Deadzone", "9")?;
        key("Sensitivity", "100")?;
        key("Pak", "0")?;
        for (name, ty, data, half) in MAP {
            key(&format!("{name}_InputType"), &q(&ty.to_string()))?;
            key(&format!("{name}_Name"), &q(name))?;
            key(&format!("{name}_Data"), &q(&data.to_string()))?;
            key(&format!("{name}_ExtraData"), &q(&half.to_string()))?;
        }
    }
    Ok(Bindings {
        edits,
        note: beyond(players, 4, "RMG"),
    })
}
