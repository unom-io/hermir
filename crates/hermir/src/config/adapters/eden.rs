//! Eden (yuzu's lineage): a Pro Controller per player in `qt-config.ini`, each control
//! `engine:sdl,guid:<guid>,port:<n>,…` with SDL's game-controller numbers, n counting pads of
//! the same GUID from 0. Eden zeroes the name CRC in the GUID.
use super::{Bindings, Cx, Plan, Seating, beyond, guid_note, join, set};
use crate::config::ini::qt_value;

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let mut edits = Vec::new();
    for p in players.iter().filter(|p| p.seat <= 8) {
        let n = u32::from(p.seat) - 1;
        let g = p.pad.sdl_guid(false);
        let i = s.ordinal(&p.pad, |q| q.sdl_guid(false));
        let btn = |b: u32| qt_value(&format!("engine:sdl,guid:{g},port:{i},button:{b}"));
        let axis = |a: u32| {
            qt_value(&format!(
                "engine:sdl,guid:{g},port:{i},axis:{a},threshold:0.500000,invert:+"
            ))
        };
        let stick = |x: u32, y: u32| {
            qt_value(&format!(
                "engine:sdl,guid:{g},port:{i},axis_x:{x},axis_y:{y},offset_x:-0.000000,\
                 offset_y:-0.000000,invert_x:+,invert_y:+,deadzone:0.150000,range:0.950000,\
                 threshold:0.500000"
            ))
        };
        let binds = [
            ("button_a", btn(1)),
            ("button_b", btn(0)),
            ("button_x", btn(3)),
            ("button_y", btn(2)),
            ("button_lstick", btn(7)),
            ("button_rstick", btn(8)),
            ("button_l", btn(9)),
            ("button_r", btn(10)),
            ("button_zl", axis(4)),
            ("button_zr", axis(5)),
            ("button_plus", btn(6)),
            ("button_minus", btn(4)),
            ("button_dleft", btn(13)),
            ("button_dup", btn(11)),
            ("button_dright", btn(14)),
            ("button_ddown", btn(12)),
            ("button_slleft", btn(9)),
            ("button_srleft", btn(10)),
            ("button_slright", btn(9)),
            ("button_srright", btn(10)),
            ("button_home", btn(5)),
            ("button_screenshot", btn(15)),
            ("lstick", stick(0, 1)),
            ("rstick", stick(2, 3)),
        ];
        edits.extend(set(
            cx,
            "main",
            "Controls",
            &format!("player_{n}_type"),
            "0",
        )?);
        edits.extend(set(
            cx,
            "main",
            "Controls",
            &format!("player_{n}_connected"),
            "true",
        )?);
        for (key, value) in binds {
            edits.extend(set(
                cx,
                "main",
                "Controls",
                &format!("player_{n}_{key}"),
                &value,
            )?);
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
