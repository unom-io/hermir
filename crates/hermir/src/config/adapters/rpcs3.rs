//! RPCS3's global input config, whole: its SDL handler names a device `<SDL name> <n>` with n
//! counting same-named pads from 1 in SDL's order, and takes SDL's control names. Seven
//! players.
use super::{Bindings, Cx, Edit, Plan, Seating, beyond, join};
use crate::config::yaml;
use crate::model::PadRef;

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let (path, _) = cx.file("input")?;
    let binds = [
        ("Left Stick Left", "LS X-"),
        ("Left Stick Down", "LS Y-"),
        ("Left Stick Right", "LS X+"),
        ("Left Stick Up", "LS Y+"),
        ("Right Stick Left", "RS X-"),
        ("Right Stick Down", "RS Y-"),
        ("Right Stick Right", "RS X+"),
        ("Right Stick Up", "RS Y+"),
        ("Start", "Start"),
        ("Select", "Back"),
        ("PS Button", "Guide"),
        ("Square", "West"),
        ("Cross", "South"),
        ("Circle", "East"),
        ("Triangle", "North"),
        ("Left", "Left"),
        ("Down", "Down"),
        ("Right", "Right"),
        ("Up", "Up"),
        ("R1", "RB"),
        ("R2", "RT"),
        ("R3", "RS"),
        ("L1", "LB"),
        ("L2", "LT"),
        ("L3", "LS"),
    ];
    let mut content = String::new();
    for seat in 1..=7u8 {
        match players.iter().find(|p| p.seat == seat) {
            Some(p) => {
                let n = s.ordinal(&p.pad, PadRef::sdl_name) + 1;
                let device = yaml::quote(&format!("{} {n}", p.pad.sdl_name()));
                content.push_str(&format!(
                    "Player {seat} Input:\n  Handler: SDL\n  Device: {device}\n  Config:\n"
                ));
                for (k, v) in binds {
                    content.push_str(&format!("    {k}: {v}\n"));
                }
                content.push_str("  Buddy Device: \"Null\"\n");
            }
            None => content.push_str(&format!(
                "Player {seat} Input:\n  Handler: Null\n  Device: \"Null\"\n  Buddy Device: \"Null\"\n"
            )),
        }
    }
    Ok(Bindings {
        edits: vec![Edit::Whole {
            file: path,
            content,
        }],
        note: join(beyond(players, 7, "RPCS3"), s.guessed("RPCS3", "name")),
    })
}
