//! ares: a virtual gamepad per seat in `settings.bml`, `VirtualPad<n>`, which each system's
//! ports read. On its SDL driver an input is `<GUID>/<slot>/<group>/<input>`, the slot counting
//! pads of that GUID from 0, with SDL 3's raw numbers (groups: axis 0, hat 1, button 3; a hat's
//! inputs are its X then Y), an axis or hat half ending `/Lo` or `/Hi`. Each input holds three
//! bindings split by `;`. Five virtual pads.
use super::{Bindings, Cx, Plan, Seating, beyond, guid_note, join, set};
use crate::players::{Ctl, Dir, RawControl};

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let mut edits = set(cx, "main", "Input", "Driver", "SDL")?;
    for p in players.iter().filter(|p| p.seat <= 5) {
        let pad = format!(
            "{}/{}",
            p.pad.sdl_guid(true),
            s.ordinal(&p.pad, |q| q.sdl_guid(true))
        );
        let raw = p.pad.raw(true);
        // `hi` picks an axis's half; a hat direction carries its own.
        let at = |c: Ctl, hi: bool| {
            let half = if hi { "Hi" } else { "Lo" };
            let one = match raw.of(c) {
                RawControl::Button(b) => format!("{pad}/3/{b}"),
                RawControl::Axis(a) => format!("{pad}/0/{a}/{half}"),
                RawControl::Hat(d) => match d {
                    Dir::Up => format!("{pad}/1/1/Lo"),
                    Dir::Down => format!("{pad}/1/1/Hi"),
                    Dir::Left => format!("{pad}/1/0/Lo"),
                    Dir::Right => format!("{pad}/1/0/Hi"),
                },
            };
            format!("{one};;")
        };
        let binds = [
            ("Pad.Up", at(Ctl::Up, false)),
            ("Pad.Down", at(Ctl::Down, false)),
            ("Pad.Left", at(Ctl::Left, false)),
            ("Pad.Right", at(Ctl::Right, false)),
            ("Select", at(Ctl::Back, false)),
            ("Start", at(Ctl::Start, false)),
            ("A..South", at(Ctl::South, false)),
            ("B..East", at(Ctl::East, false)),
            ("X..West", at(Ctl::West, false)),
            ("Y..North", at(Ctl::North, false)),
            ("L-Bumper", at(Ctl::LeftShoulder, false)),
            ("R-Bumper", at(Ctl::RightShoulder, false)),
            ("L-Trigger", at(Ctl::LeftTrigger, true)),
            ("R-Trigger", at(Ctl::RightTrigger, true)),
            ("L-Stick..Click", at(Ctl::LeftStick, false)),
            ("R-Stick..Click", at(Ctl::RightStick, false)),
            ("L-Up", at(Ctl::LeftY, false)),
            ("L-Down", at(Ctl::LeftY, true)),
            ("L-Left", at(Ctl::LeftX, false)),
            ("L-Right", at(Ctl::LeftX, true)),
            ("R-Up", at(Ctl::RightY, false)),
            ("R-Down", at(Ctl::RightY, true)),
            ("R-Left", at(Ctl::RightX, false)),
            ("R-Right", at(Ctl::RightX, true)),
        ];
        let section = format!("VirtualPad{}", p.seat);
        for (k, v) in binds {
            edits.extend(set(cx, "main", &section, k, &v)?);
        }
    }
    Ok(Bindings {
        edits,
        note: join(
            join(beyond(players, 5, "ares"), guid_note(cx.os, s)),
            s.guessed("ares", "GUID"),
        ),
    })
}
