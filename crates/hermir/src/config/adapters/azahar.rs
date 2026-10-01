//! Azahar (Citra's lineage): one 3DS, so one player, in profile 1 of `qt-config.ini`, each
//! control `api:controller,…,engine:sdl,guid:<guid>,port:<index>`.
use super::{Bindings, Cx, Plan, beyond, guid_note, join, set};
use crate::model::Player;

pub(super) fn players(cx: &Cx, players: &[Player]) -> Plan {
    let Some(p) = players.iter().find(|p| p.seat == 1) else {
        return Ok(Bindings {
            edits: Vec::new(),
            note: beyond(players, 1, "a 3DS"),
        });
    };
    let (g, i) = (p.pad.sdl_guid(true), p.pad.index);
    let btn = |b: u32| format!("\"api:controller,button:{b},engine:sdl,guid:{g},port:{i}\"");
    let axis = |a: u32| {
        format!(
            "\"api:controller,axis:{a},direction:+,engine:sdl,guid:{g},port:{i},threshold:0.500000\""
        )
    };
    let stick = |x: u32, y: u32| {
        format!("\"api:controller,axis_x:{x},axis_y:{y},engine:sdl,guid:{g},port:{i}\"")
    };
    let binds = [
        ("button_a", btn(1)),
        ("button_b", btn(0)),
        ("button_x", btn(3)),
        ("button_y", btn(2)),
        ("button_l", btn(9)),
        ("button_r", btn(10)),
        ("button_zl", axis(4)),
        ("button_zr", axis(5)),
        ("button_start", btn(6)),
        ("button_select", btn(4)),
        ("button_home", btn(5)),
        ("button_up", btn(11)),
        ("button_down", btn(12)),
        ("button_left", btn(13)),
        ("button_right", btn(14)),
        ("circle_pad", stick(0, 1)),
        ("c_stick", stick(2, 3)),
    ];
    let mut edits = Vec::new();
    for (key, value) in binds {
        edits.extend(set(
            cx,
            "main",
            "Controls",
            &format!("profiles\\1\\{key}"),
            &value,
        )?);
    }
    Ok(Bindings {
        edits,
        note: join(beyond(players, 1, "a 3DS"), guid_note(cx.os)),
    })
}
