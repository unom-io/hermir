//! PCSX2: a DualShock 2 per seat in `inis/PCSX2.ini`, through the shared SDL input core.
use super::pad_ini::{self, Multitap, Names};
use super::{Cx, Plan, Seating};

const NAMES: Names = Names {
    south: "FaceSouth",
    east: "FaceEast",
    west: "FaceWest",
    north: "FaceNorth",
};

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    pad_ini::players(cx, "DualShock2", &NAMES, Multitap::Pcsx2, players)
}
