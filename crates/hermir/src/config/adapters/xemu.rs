//! xemu binds a port to SDL's GUID in `[input.bindings]`; the Xbox layout is fixed. Four ports.
use super::{Bindings, Cx, Plan, beyond, guid_note, join, set};
use crate::model::Player;

pub(super) fn players(cx: &Cx, players: &[Player]) -> Plan {
    let mut edits = Vec::new();
    for p in players.iter().filter(|p| p.seat <= 4) {
        edits.extend(set(
            cx,
            "main",
            "input.bindings",
            &format!("port{}", p.seat),
            &format!("\"{}\"", p.pad.sdl_guid(true)),
        )?);
    }
    Ok(Bindings {
        edits,
        note: join(beyond(players, 4, "xemu"), guid_note(cx.os)),
    })
}
