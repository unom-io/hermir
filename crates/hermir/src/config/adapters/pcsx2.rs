//! PCSX2: a DualShock 2 per seat in `inis/PCSX2.ini`, through the shared SDL input core.
use super::pad_ini::{self, Names};
use super::{Cx, Plan};
use crate::model::Player;

const NAMES: Names = Names {
    south: "FaceSouth",
    east: "FaceEast",
    west: "FaceWest",
    north: "FaceNorth",
};

pub(super) fn players(cx: &Cx, players: &[Player]) -> Plan {
    pad_ini::players(cx, "DualShock2", &NAMES, players)
}
