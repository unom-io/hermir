//! Eden (yuzu's lineage): a Pro Controller per player in `qt-config.ini`, each control
//! `engine:sdl,guid:<guid>,port:<n>,…` with SDL 3's raw joystick numbers, n counting pads of
//! the same GUID from 0. Eden zeroes the name CRC in the GUID. A player is plugged in by its
//! `connected` flag, not by a pad, so every seat without one is unplugged.
use super::{Bindings, Cx, Plan, Seating, beyond, guid_note, join, set};
use crate::config::ini::qt_value;
use crate::players::{Ctl, Dir, RawControl};

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let mut edits = Vec::new();
    let mut key = |n: u8, k: &str, v: &str| -> Result<(), String> {
        edits.extend(set(cx, "main", "Controls", &format!("player_{n}_{k}"), v)?);
        Ok(())
    };
    for n in 0..8u8 {
        let Some(p) = players.iter().find(|p| p.seat == n + 1) else {
            key(n, "connected", "false")?;
            continue;
        };
        let g = p.pad.sdl_guid(false);
        let i = s.ordinal(&p.pad, |q| q.sdl_guid(false));
        let raw = p.pad.raw(true);
        let pad = format!("engine:sdl,guid:{g},port:{i}");
        let ctl = |c: Ctl| {
            qt_value(&match raw.of(c) {
                RawControl::Button(b) => format!("{pad},button:{b}"),
                RawControl::Axis(a) => format!("{pad},axis:{a},threshold:0.500000,invert:+"),
                RawControl::Hat(d) => {
                    let d = match d {
                        Dir::Up => "up",
                        Dir::Down => "down",
                        Dir::Left => "left",
                        Dir::Right => "right",
                    };
                    format!("{pad},hat:0,direction:{d}")
                }
            })
        };
        let axis = |c: Ctl| match raw.of(c) {
            RawControl::Axis(a) => a,
            _ => 0,
        };
        let stick = |x: Ctl, y: Ctl| {
            qt_value(&format!(
                "{pad},axis_x:{},axis_y:{},offset_x:-0.000000,offset_y:-0.000000,invert_x:+,\
                 invert_y:+,deadzone:0.150000,range:0.950000,threshold:0.500000",
                axis(x),
                axis(y)
            ))
        };
        // Switch buttons by position: A east, B south, X north, Y west.
        let binds = [
            ("button_a", ctl(Ctl::East)),
            ("button_b", ctl(Ctl::South)),
            ("button_x", ctl(Ctl::North)),
            ("button_y", ctl(Ctl::West)),
            ("button_lstick", ctl(Ctl::LeftStick)),
            ("button_rstick", ctl(Ctl::RightStick)),
            ("button_l", ctl(Ctl::LeftShoulder)),
            ("button_r", ctl(Ctl::RightShoulder)),
            ("button_zl", ctl(Ctl::LeftTrigger)),
            ("button_zr", ctl(Ctl::RightTrigger)),
            ("button_plus", ctl(Ctl::Start)),
            ("button_minus", ctl(Ctl::Back)),
            ("button_dleft", ctl(Ctl::Left)),
            ("button_dup", ctl(Ctl::Up)),
            ("button_dright", ctl(Ctl::Right)),
            ("button_ddown", ctl(Ctl::Down)),
            ("button_slleft", ctl(Ctl::LeftShoulder)),
            ("button_srleft", ctl(Ctl::RightShoulder)),
            ("button_slright", ctl(Ctl::LeftShoulder)),
            ("button_srright", ctl(Ctl::RightShoulder)),
            ("button_home", ctl(Ctl::Guide)),
            ("lstick", stick(Ctl::LeftX, Ctl::LeftY)),
            ("rstick", stick(Ctl::RightX, Ctl::RightY)),
            // A pad without a gyro gives Eden nothing here; one with gives both halves motion.
            (
                "motionleft",
                qt_value(&format!("engine:sdl,motion:0,port:{i},guid:{g}")),
            ),
            (
                "motionright",
                qt_value(&format!("engine:sdl,motion:0,port:{i},guid:{g}")),
            ),
        ];
        key(n, "type", "0")?;
        key(n, "connected", "true")?;
        for (k, v) in binds {
            key(n, k, &v)?;
        }
    }
    Ok(Bindings {
        edits,
        note: join(
            join(beyond(players, 8, "Eden"), guid_note(cx.os, s)),
            s.guessed("Eden", "GUID"),
        ),
    })
}
