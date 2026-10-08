//! melonDS keys a raw SDL 2 joystick by index: a button is its number, a hat direction is
//! `0x100 | hat << 4 | direction` (up 1, right 2, down 4, left 8), laid out as SDL numbers
//! the pad's raw controls. One DS, one player; its gyro, when the pad has one, needs nothing.
use super::{Bindings, Cx, Plan, Seating, beyond, set};
use crate::players::{Ctl, Dir, RawControl};

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let note = beyond(players, 1, "a DS");
    let Some(p) = players.iter().find(|p| p.seat == 1) else {
        return Ok(Bindings {
            edits: Vec::new(),
            note,
        });
    };
    let raw = p.pad.raw(false);
    let code = |c: Ctl| match raw.of(c) {
        RawControl::Button(b) => b,
        RawControl::Hat(d) => {
            0x100
                | match d {
                    Dir::Up => 1,
                    Dir::Right => 2,
                    Dir::Down => 4,
                    Dir::Left => 8,
                }
        }
        // Only the triggers are axes, and none is bound to them.
        RawControl::Axis(a) => 0x10000 | a,
    };
    let mut edits = set(
        cx,
        "main",
        "Instance0",
        "JoystickID",
        &p.pad.index.to_string(),
    )?;
    // DS buttons by position: A east, B south, X north, Y west.
    let binds = [
        ("A", code(Ctl::East)),
        ("B", code(Ctl::South)),
        ("X", code(Ctl::North)),
        ("Y", code(Ctl::West)),
        ("L", code(Ctl::LeftShoulder)),
        ("R", code(Ctl::RightShoulder)),
        ("Select", code(Ctl::Back)),
        ("Start", code(Ctl::Start)),
        ("Up", code(Ctl::Up)),
        ("Right", code(Ctl::Right)),
        ("Down", code(Ctl::Down)),
        ("Left", code(Ctl::Left)),
    ];
    for (k, v) in binds {
        edits.extend(set(cx, "main", "Instance0.Joystick", k, &v.to_string())?);
    }
    Ok(Bindings { edits, note })
}
