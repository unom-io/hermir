//! Cemu loads `controllerProfiles/controller<n>.xml` for player n+1: one SDL controller,
//! keyed `<n>_<guid>`, n counting game controllers of the same GUID from 0 (its SDL provider's
//! `guid_counter`), on a GamePad for player 1 and Pro Controllers after. Mapping ids are
//! Cemu's per controller type; button ids are SDL's, plus Cemu's axis halves (38–49). A game
//! sees a controller for every file, so a seat without a pad has none.
use super::{Bindings, Cx, Edit, Plan, Seating, beyond, guid_note, join};
use crate::config::xml;

/// (GamePad mapping, SDL button or Cemu axis id). The GamePad's motion is the pad's gyro.
const GAMEPAD: [(u32, u32); 26] = [
    (1, 1),
    (2, 0),
    (3, 3),
    (4, 2),
    (5, 9),
    (6, 10),
    (7, 42),
    (8, 43),
    (9, 6),
    (10, 4),
    (11, 11),
    (12, 12),
    (13, 13),
    (14, 14),
    (15, 7),
    (16, 8),
    (17, 45),
    (18, 39),
    (19, 44),
    (20, 38),
    (21, 47),
    (22, 41),
    (23, 46),
    (24, 40),
    (26, 15),
    (27, 5),
];

/// (Pro Controller mapping, SDL button or Cemu axis id): Home sits before the d-pad here.
const PRO: [(u32, u32); 25] = [
    (1, 1),
    (2, 0),
    (3, 3),
    (4, 2),
    (5, 9),
    (6, 10),
    (7, 42),
    (8, 43),
    (9, 6),
    (10, 4),
    (11, 5),
    (12, 11),
    (13, 12),
    (14, 13),
    (15, 14),
    (16, 7),
    (17, 8),
    (18, 45),
    (19, 39),
    (20, 44),
    (21, 38),
    (22, 47),
    (23, 41),
    (24, 46),
    (25, 40),
];

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let file = |n: u8| {
        cx.root
            .join(format!("controllerProfiles/controller{n}.xml"))
    };
    let edits = (0..8u8)
        .map(|n| {
            let Some(p) = players.iter().find(|p| p.seat == n + 1) else {
                return Edit::Remove { file: file(n) };
            };
            let (kind, map, motion): (_, &[(u32, u32)], _) = if n == 0 {
                ("Wii U GamePad", &GAMEPAD, "true")
            } else {
                ("Wii U Pro Controller", &PRO, "false")
            };
            let entries: String = map
                .iter()
                .map(|(m, b)| {
                    format!(
                        "\t\t\t<entry>\n\t\t\t\t<mapping>{m}</mapping>\n\t\t\t\t<button>{b}</button>\n\t\t\t</entry>\n"
                    )
                })
                .collect();
            let content = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<emulated_controller>\n\t<type>{kind}</type>\n\t<controller>\n\t\t<api>SDLController</api>\n\t\t<uuid>{}_{}</uuid>\n\t\t<display_name>{}</display_name>\n\t\t<rumble>0</rumble>\n\t\t<motion>{motion}</motion>\n\t\t<axis>\n\t\t\t<deadzone>0.25</deadzone>\n\t\t\t<range>1</range>\n\t\t</axis>\n\t\t<rotation>\n\t\t\t<deadzone>0.25</deadzone>\n\t\t\t<range>1</range>\n\t\t</rotation>\n\t\t<trigger>\n\t\t\t<deadzone>0.25</deadzone>\n\t\t\t<range>1</range>\n\t\t</trigger>\n\t\t<mappings>\n{entries}\t\t</mappings>\n\t</controller>\n</emulated_controller>\n",
                s.ordinal(&p.pad, |q| q.sdl_guid(true)),
                p.pad.sdl_guid(true),
                xml::escape(&p.pad.sdl_name())
            );
            Edit::Whole {
                file: file(n),
                content,
            }
        })
        .collect();
    Ok(Bindings {
        edits,
        note: join(
            join(beyond(players, 8, "Cemu"), guid_note(cx.os, s)),
            s.guessed("Cemu", "GUID"),
        ),
    })
}
