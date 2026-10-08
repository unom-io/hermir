//! Azahar (Citra's lineage): one 3DS, so one player, in the first profile of `qt-config.ini`,
//! made the current one. Each control is `api:controller,…,engine:sdl,maptype:all`, which
//! reads SDL's game-controller numbers off every pad, so no GUID has to match the SDL Azahar
//! is built on. The motion device without a GUID is the first pad's gyro.
use super::{Bindings, Cx, Plan, Seating, beyond, set};
use crate::config::ini::qt_value;

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let note = beyond(players, 1, "a 3DS");
    if !players.iter().any(|p| p.seat == 1) {
        return Ok(Bindings {
            edits: Vec::new(),
            note,
        });
    }
    let any = "engine:sdl,maptype:all";
    let btn = |b: u32| qt_value(&format!("api:controller,button:{b},{any}"));
    let axis = |a: u32| {
        qt_value(&format!(
            "api:controller,axis:{a},direction:+,{any},threshold:0.500000"
        ))
    };
    let stick = |x: u32, y: u32| qt_value(&format!("api:controller,axis_x:{x},axis_y:{y},{any}"));
    // 3DS buttons by position: A east, B south, X north, Y west.
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
        ("motion_device", qt_value("engine:sdl")),
    ];
    let mut edits = set(cx, "main", "Controls", "profile", "0")?;
    edits.extend(set(cx, "main", "Controls", "profiles\\size", "1")?);
    for (key, value) in binds {
        edits.extend(set(
            cx,
            "main",
            "Controls",
            &format!("profiles\\1\\{key}"),
            &value,
        )?);
    }
    Ok(Bindings { edits, note })
}
