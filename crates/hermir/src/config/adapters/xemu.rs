//! xemu binds a port to SDL's GUID in `[input.bindings]`; the Xbox layout is fixed. Four ports.
//! Its window may lack focus while streamed, so it keeps reading pads in the background.
use super::{Bindings, Cx, Plan, Seating, beyond, guid_note, join, set};

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let mut edits = set(cx, "main", "input", "background_input_capture", "true")?;
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
        note: join(beyond(players, 4, "xemu"), guid_note(cx.os, s)),
    })
}
