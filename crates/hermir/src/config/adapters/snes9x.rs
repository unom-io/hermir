//! Snes9x's GTK build: a pad per seat in `snes9x.conf`, `[Joypad <seat - 1>]`, each control
//! `Joystick <n> Button <b>` or `Joystick <n> Axis <a> <+|-> <threshold>%` on a raw SDL 2
//! joystick, n its number from 1. A hat is two axes after the real ones: up is `+` on
//! `axes + 2h`, left `-` on the one after. A third player needs port 2's multitap, five at
//! most. The Windows build reads pads through winmm, which nothing here describes.
use super::{Bindings, Cx, Plan, Seating, beyond, set};
use crate::model::Os;
use crate::players::{Ctl, Dir, RawControl};

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    if cx.os == Os::Windows {
        return Err("Snes9x on Windows reads pads through winmm, not SDL".into());
    }
    let players = &s.seats[..];
    let top = players
        .iter()
        .map(|p| p.seat)
        .filter(|s| *s <= 5)
        .max()
        .unwrap_or(0);
    let port = if top >= 3 { "multitap" } else { "joypad" };
    let mut edits = set(cx, "main", "Input", "ControllerPort1", port)?;
    for p in players.iter().filter(|p| p.seat <= 5) {
        let raw = p.pad.raw(false);
        // SDL 2 counts six axes on both layouts; the hat axes come after them.
        let axes = 6;
        let joy = format!("Joystick {}", p.pad.index + 1);
        let at = |c: Ctl| match raw.of(c) {
            RawControl::Button(b) => format!("{joy} Button {b}"),
            RawControl::Axis(a) => format!("{joy} Axis {a} + 50%"),
            RawControl::Hat(d) => {
                let (a, sign) = match d {
                    Dir::Up => (axes, '+'),
                    Dir::Down => (axes, '-'),
                    Dir::Left => (axes + 1, '-'),
                    Dir::Right => (axes + 1, '+'),
                };
                format!("{joy} Axis {a} {sign} 50%")
            }
        };
        // SNES buttons by position: B south, A east, Y west, X north.
        let binds = [
            ("A", at(Ctl::East)),
            ("B", at(Ctl::South)),
            ("X", at(Ctl::North)),
            ("Y", at(Ctl::West)),
            ("L", at(Ctl::LeftShoulder)),
            ("R", at(Ctl::RightShoulder)),
            ("Select", at(Ctl::Back)),
            ("Start", at(Ctl::Start)),
            ("Up", at(Ctl::Up)),
            ("Down", at(Ctl::Down)),
            ("Left", at(Ctl::Left)),
            ("Right", at(Ctl::Right)),
        ];
        let section = format!("Joypad {}", p.seat - 1);
        for (k, v) in binds {
            edits.extend(set(cx, "main", &section, k, &v)?);
        }
    }
    Ok(Bindings {
        edits,
        note: beyond(players, 5, "Snes9x with a multitap"),
    })
}
