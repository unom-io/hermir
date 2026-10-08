//! Dolphin: a GameCube pad on the sticks and face buttons, a Wii Remote with Nunchuk, pointer
//! on the right stick, B on the right trigger and the pad's gyro and accelerometer as its
//! motion, per seat. Through its SDL backend, which names a device `SDL/<id>/<SDL name>`, the
//! id counting same-named pads from 0, and a pad's controls by SDL's game-controller names,
//! so every pad kind binds alike. An Xbox pad on Windows goes through XInput; the face
//! buttons are spelled its way (`Button A` south), which SDL's backend matches by position.
//! A seat is plugged in by `SIDevice<n>` and the remote's `Source`, so every seat without a
//! pad is unplugged.
use super::{Bindings, Cx, Plan, Seating, beyond, join, set};
use crate::model::{Os, PadRef};

type Binds = &'static [(&'static str, &'static str)];

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let mut edits = Vec::new();
    for seat in 1..=4u8 {
        let (gc, wii) = (format!("GCPad{seat}"), format!("Wiimote{seat}"));
        let si = format!("SIDevice{}", seat - 1);
        let Some(p) = players.iter().find(|p| p.seat == seat) else {
            edits.extend(set(cx, "main", "Core", &si, "0")?);
            edits.extend(set(cx, "wiimote", &wii, "Source", "0")?);
            continue;
        };
        let dev = device(cx.os, s, &p.pad);
        // 6: a standard controller.
        edits.extend(set(cx, "main", "Core", &si, "6")?);
        edits.extend(set(cx, "gcpad", &gc, "Device", &dev)?);
        for (k, v) in GC {
            edits.extend(set(cx, "gcpad", &gc, k, v)?);
        }
        edits.extend(set(cx, "wiimote", &wii, "Source", "1")?);
        edits.extend(set(cx, "wiimote", &wii, "Device", &dev)?);
        edits.extend(set(cx, "wiimote", &wii, "Extension", "Nunchuk")?);
        for (k, v) in WII {
            edits.extend(set(cx, "wiimote", &wii, k, v)?);
        }
    }
    Ok(Bindings {
        edits,
        note: join(beyond(players, 4, "Dolphin"), s.guessed("Dolphin", "name")),
    })
}

/// The pad as Dolphin names it: XInput's slot for an Xbox pad on Windows, else SDL's.
fn device(os: Os, s: &Seating, pad: &PadRef) -> String {
    if os == Os::Windows && !pad.hidapi() {
        return format!("XInput/{}/Gamepad", pad.index);
    }
    format!(
        "SDL/{}/{}",
        s.ordinal(pad, PadRef::sdl_name),
        pad.sdl_name()
    )
}

const GC: Binds = &[
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

const WII: Binds = &[
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
    // A pad without the sensors leaves these idle; XInput has none.
    ("IMUAccelerometer/Up", "`Accel Up`"),
    ("IMUAccelerometer/Down", "`Accel Down`"),
    ("IMUAccelerometer/Left", "`Accel Left`"),
    ("IMUAccelerometer/Right", "`Accel Right`"),
    ("IMUAccelerometer/Forward", "`Accel Forward`"),
    ("IMUAccelerometer/Backward", "`Accel Backward`"),
    ("IMUGyroscope/Pitch Up", "`Gyro Pitch Up`"),
    ("IMUGyroscope/Pitch Down", "`Gyro Pitch Down`"),
    ("IMUGyroscope/Roll Left", "`Gyro Roll Left`"),
    ("IMUGyroscope/Roll Right", "`Gyro Roll Right`"),
    ("IMUGyroscope/Yaw Left", "`Gyro Yaw Left`"),
    ("IMUGyroscope/Yaw Right", "`Gyro Yaw Right`"),
];
