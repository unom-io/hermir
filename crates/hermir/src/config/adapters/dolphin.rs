//! Dolphin: a GameCube pad on the sticks and face buttons, a Wii Remote with Nunchuk, pointer
//! on the right stick and B on the right trigger, per seat. On Linux through its evdev
//! backend, which names a device `evdev/<id>/<kernel name>`, the id counting same-named
//! devices from 0, and a pad's controls by position (`Button 0`, `Axis 1-`); on Windows
//! through XInput, which names them (`Button A`, `Left Y+`).
use super::{Bindings, Cx, Plan, Seating, beyond, join, layout_note, set};
use crate::model::Os;

type Binds = &'static [(&'static str, &'static str)];

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let mut edits = Vec::new();
    for p in players.iter().filter(|p| p.seat <= 4) {
        let gc = format!("GCPad{}", p.seat);
        let wii = format!("Wiimote{}", p.seat);
        let (dev, gc_binds, wii_binds): (String, Binds, Binds) = match cx.os {
            Os::Windows => (
                format!("XInput/{}/Gamepad", p.pad.index),
                GC_XINPUT,
                WII_XINPUT,
            ),
            _ => (
                format!(
                    "evdev/{}/{}",
                    s.ordinal(&p.pad, |q| q.name.clone()),
                    p.pad.name
                ),
                GC_EVDEV,
                WII_EVDEV,
            ),
        };
        edits.extend(set(cx, "gcpad", &gc, "Device", &dev)?);
        for (k, v) in gc_binds {
            edits.extend(set(cx, "gcpad", &gc, k, v)?);
        }
        edits.extend(set(cx, "wiimote", &wii, "Source", "1")?);
        edits.extend(set(cx, "wiimote", &wii, "Device", &dev)?);
        edits.extend(set(cx, "wiimote", &wii, "Extension", "Nunchuk")?);
        for (k, v) in wii_binds {
            edits.extend(set(cx, "wiimote", &wii, k, v)?);
        }
    }
    Ok(Bindings {
        edits,
        note: if cx.os == Os::Windows {
            beyond(players, 4, "Dolphin")
        } else {
            join(
                join(beyond(players, 4, "Dolphin"), s.guessed("Dolphin", "name")),
                layout_note("Dolphin's evdev backend", s, 4),
            )
        },
    })
}

const GC_EVDEV: &[(&str, &str)] = &[
    ("Buttons/A", "`Button 0`"),
    ("Buttons/B", "`Button 2`"),
    ("Buttons/X", "`Button 1`"),
    ("Buttons/Y", "`Button 3`"),
    ("Buttons/Z", "`Button 5`"),
    ("Buttons/Start", "`Button 7`"),
    ("Main Stick/Up", "`Axis 1-`"),
    ("Main Stick/Down", "`Axis 1+`"),
    ("Main Stick/Left", "`Axis 0-`"),
    ("Main Stick/Right", "`Axis 0+`"),
    ("C-Stick/Up", "`Axis 4-`"),
    ("C-Stick/Down", "`Axis 4+`"),
    ("C-Stick/Left", "`Axis 3-`"),
    ("C-Stick/Right", "`Axis 3+`"),
    ("Triggers/L", "`Axis 2+`"),
    ("Triggers/R", "`Axis 5+`"),
    ("Triggers/L-Analog", "`Axis 2+`"),
    ("Triggers/R-Analog", "`Axis 5+`"),
    ("D-Pad/Up", "`Axis 7-`"),
    ("D-Pad/Down", "`Axis 7+`"),
    ("D-Pad/Left", "`Axis 6-`"),
    ("D-Pad/Right", "`Axis 6+`"),
    ("Rumble/Motor", "`Motor`"),
];

const WII_EVDEV: &[(&str, &str)] = &[
    ("Buttons/A", "`Button 0`"),
    ("Buttons/B", "`Axis 5+`"),
    ("Buttons/1", "`Button 2`"),
    ("Buttons/2", "`Button 3`"),
    ("Buttons/-", "`Button 6`"),
    ("Buttons/+", "`Button 7`"),
    ("Buttons/Home", "`Button 8`"),
    ("IR/Up", "`Axis 4-`"),
    ("IR/Down", "`Axis 4+`"),
    ("IR/Left", "`Axis 3-`"),
    ("IR/Right", "`Axis 3+`"),
    ("Shake/X", "`Button 1`"),
    ("Shake/Y", "`Button 1`"),
    ("Shake/Z", "`Button 1`"),
    ("Nunchuk/Buttons/C", "`Button 4`"),
    ("Nunchuk/Buttons/Z", "`Axis 2+`"),
    ("Nunchuk/Stick/Up", "`Axis 1-`"),
    ("Nunchuk/Stick/Down", "`Axis 1+`"),
    ("Nunchuk/Stick/Left", "`Axis 0-`"),
    ("Nunchuk/Stick/Right", "`Axis 0+`"),
    ("Nunchuk/Shake/X", "`Button 10`"),
    ("Nunchuk/Shake/Y", "`Button 10`"),
    ("Nunchuk/Shake/Z", "`Button 10`"),
    ("D-Pad/Up", "`Axis 7-`"),
    ("D-Pad/Down", "`Axis 7+`"),
    ("D-Pad/Left", "`Axis 6-`"),
    ("D-Pad/Right", "`Axis 6+`"),
    ("Rumble/Motor", "`Motor`"),
];

const GC_XINPUT: &[(&str, &str)] = &[
    ("Buttons/A", "`Button A`"),
    ("Buttons/B", "`Button X`"),
    ("Buttons/X", "`Button B`"),
    ("Buttons/Y", "`Button Y`"),
    ("Buttons/Z", "`Shoulder R`"),
    ("Buttons/Start", "`Start`"),
    ("Main Stick/Up", "`Left Y+`"),
    ("Main Stick/Down", "`Left Y-`"),
    ("Main Stick/Left", "`Left X-`"),
    ("Main Stick/Right", "`Left X+`"),
    ("C-Stick/Up", "`Right Y+`"),
    ("C-Stick/Down", "`Right Y-`"),
    ("C-Stick/Left", "`Right X-`"),
    ("C-Stick/Right", "`Right X+`"),
    ("Triggers/L", "`Trigger L`"),
    ("Triggers/R", "`Trigger R`"),
    ("Triggers/L-Analog", "`Trigger L`"),
    ("Triggers/R-Analog", "`Trigger R`"),
    ("D-Pad/Up", "`Pad N`"),
    ("D-Pad/Down", "`Pad S`"),
    ("D-Pad/Left", "`Pad W`"),
    ("D-Pad/Right", "`Pad E`"),
    ("Rumble/Motor", "`Motor L`|`Motor R`"),
];

const WII_XINPUT: &[(&str, &str)] = &[
    ("Buttons/A", "`Button A`"),
    ("Buttons/B", "`Trigger R`"),
    ("Buttons/1", "`Button X`"),
    ("Buttons/2", "`Button Y`"),
    ("Buttons/-", "`Back`"),
    ("Buttons/+", "`Start`"),
    ("Buttons/Home", "`Guide`"),
    ("IR/Up", "`Right Y+`"),
    ("IR/Down", "`Right Y-`"),
    ("IR/Left", "`Right X-`"),
    ("IR/Right", "`Right X+`"),
    ("Shake/X", "`Button B`"),
    ("Shake/Y", "`Button B`"),
    ("Shake/Z", "`Button B`"),
    ("Nunchuk/Buttons/C", "`Shoulder L`"),
    ("Nunchuk/Buttons/Z", "`Trigger L`"),
    ("Nunchuk/Stick/Up", "`Left Y+`"),
    ("Nunchuk/Stick/Down", "`Left Y-`"),
    ("Nunchuk/Stick/Left", "`Left X-`"),
    ("Nunchuk/Stick/Right", "`Left X+`"),
    ("Nunchuk/Shake/X", "`Thumb R`"),
    ("Nunchuk/Shake/Y", "`Thumb R`"),
    ("Nunchuk/Shake/Z", "`Thumb R`"),
    ("D-Pad/Up", "`Pad N`"),
    ("D-Pad/Down", "`Pad S`"),
    ("D-Pad/Left", "`Pad W`"),
    ("D-Pad/Right", "`Pad E`"),
    ("Rumble/Motor", "`Motor L`|`Motor R`"),
];
