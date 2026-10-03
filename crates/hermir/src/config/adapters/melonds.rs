//! melonDS keys a raw SDL joystick by index: a button is its number, a hat direction is
//! `0x100 | hat << 4 | direction` (up 1, right 2, down 4, left 8). One DS, one player.
use super::{Bindings, Cx, Plan, Seating, beyond, join, layout_note, set};

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let Some(p) = players.iter().find(|p| p.seat == 1) else {
        return Ok(Bindings {
            edits: Vec::new(),
            note: beyond(players, 1, "a DS"),
        });
    };
    let mut edits = set(
        cx,
        "main",
        "Instance0",
        "JoystickID",
        &p.pad.index.to_string(),
    )?;
    let binds = [
        ("A", 1),
        ("B", 0),
        ("X", 3),
        ("Y", 2),
        ("L", 4),
        ("R", 5),
        ("Select", 6),
        ("Start", 7),
        ("Up", 0x101),
        ("Right", 0x102),
        ("Down", 0x104),
        ("Left", 0x108),
    ];
    for (k, v) in binds {
        edits.extend(set(cx, "main", "Instance0.Joystick", k, &v.to_string())?);
    }
    Ok(Bindings {
        edits,
        note: join(beyond(players, 1, "a DS"), layout_note("melonDS", s, 1)),
    })
}
