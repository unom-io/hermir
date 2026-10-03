//! DuckStation: an analog controller per seat in `settings.ini`, through the shared SDL
//! input core it has in common with PCSX2.
use super::pad_ini::{self, Names};
use super::{Cx, Plan, Seating};

const NAMES: Names = Names {
    south: "A",
    east: "B",
    west: "X",
    north: "Y",
};

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    pad_ini::players(cx, "AnalogController", &NAMES, players)
}
