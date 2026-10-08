//! Flycast drives Dreamcast port n with the pad at SDL index n and maps its controls itself;
//! what a port holds is `[input] device<n>`, a controller (0) or nothing (10), and only port A
//! holds one unless told. Four ports.
use super::{Bindings, Cx, Plan, Seating, beyond, set};

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let mut edits = Vec::new();
    for port in 1..=4u8 {
        let device = if players.iter().any(|p| p.seat == port) {
            "0"
        } else {
            "10"
        };
        edits.extend(set(cx, "main", "input", &format!("device{port}"), device)?);
    }
    Ok(Bindings {
        edits,
        note: beyond(players, 4, "Flycast"),
    })
}
