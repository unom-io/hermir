//! mGBA binds a raw SDL 2 joystick: `key<Name>` is a button number, `hat0<Dir>` the GBA key a
//! hat direction presses, in `[<platform>.input.SDLB]`. A profile named after the pad, its
//! own or built in, is loaded over that, so the same binds go there too. One player; its gyro
//! and tilt, for the carts that have them, need nothing.
use super::{Bindings, Cx, Plan, Seating, beyond, set};
use crate::players::{Ctl, Dir, RawControl};

/// GBA keys as mGBA numbers them.
const UP: u32 = 6;
const RIGHT: u32 = 4;
const DOWN: u32 = 7;
const LEFT: u32 = 5;

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let note = beyond(players, 1, "mGBA");
    let Some(p) = players.iter().find(|p| p.seat == 1) else {
        return Ok(Bindings {
            edits: Vec::new(),
            note,
        });
    };
    let raw = p.pad.raw(false);
    let mut binds: Vec<(String, u32)> = Vec::new();
    // GBA buttons by position: A east, B south.
    for (key, c) in [
        ("A", Ctl::East),
        ("B", Ctl::South),
        ("L", Ctl::LeftShoulder),
        ("R", Ctl::RightShoulder),
        ("Start", Ctl::Start),
        ("Select", Ctl::Back),
        ("Up", Ctl::Up),
        ("Down", Ctl::Down),
        ("Left", Ctl::Left),
        ("Right", Ctl::Right),
    ] {
        match raw.of(c) {
            RawControl::Button(b) => binds.push((format!("key{key}"), b)),
            RawControl::Hat(d) => {
                let (dir, gba) = match d {
                    Dir::Up => ("Up", UP),
                    Dir::Right => ("Right", RIGHT),
                    Dir::Down => ("Down", DOWN),
                    Dir::Left => ("Left", LEFT),
                };
                binds.push((format!("hat0{dir}"), gba));
            }
            RawControl::Axis(_) => {}
        }
    }
    let mut edits = Vec::new();
    for platform in ["gba", "gb"] {
        for section in [
            format!("{platform}.input.SDLB"),
            format!("{platform}.input-profile.{}", p.pad.sdl_name()),
        ] {
            for (k, v) in &binds {
                edits.extend(set(cx, "main", &section, k, &v.to_string())?);
            }
        }
    }
    Ok(Bindings { edits, note })
}
