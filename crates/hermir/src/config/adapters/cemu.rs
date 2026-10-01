//! Cemu loads `controllerProfiles/controller<n>.xml` for player n+1: one SDL controller,
//! keyed `<index>_<guid>`, on a GamePad for player 1 and Pro Controllers after. Mapping ids
//! are Cemu's Wii U buttons; button ids are SDL's, plus Cemu's axis halves (38–49).
use super::{Bindings, Cx, Edit, Plan, beyond, guid_note, join};
use crate::model::Player;

pub(super) fn players(cx: &Cx, players: &[Player]) -> Plan {
    let edits = players
        .iter()
        .filter(|p| p.seat <= 8)
        .map(|p| {
            let n = u32::from(p.seat) - 1;
            let kind = if p.seat == 1 {
                "Wii U GamePad"
            } else {
                "Wii U Pro Controller"
            };
            // (Cemu mapping, SDL button or Cemu axis id)
            let map: [(u32, u32); 26] = [
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
            let entries: String = map
                .iter()
                .map(|(m, b)| {
                    format!(
                        "\t\t\t<entry>\n\t\t\t\t<mapping>{m}</mapping>\n\t\t\t\t<button>{b}</button>\n\t\t\t</entry>\n"
                    )
                })
                .collect();
            let content = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<emulated_controller>\n\t<type>{kind}</type>\n\t<controller>\n\t\t<api>SDLController</api>\n\t\t<uuid>{}_{}</uuid>\n\t\t<display_name>{}</display_name>\n\t\t<rumble>0</rumble>\n\t\t<axis>\n\t\t\t<deadzone>0.25</deadzone>\n\t\t\t<range>1</range>\n\t\t</axis>\n\t\t<rotation>\n\t\t\t<deadzone>0.25</deadzone>\n\t\t\t<range>1</range>\n\t\t</rotation>\n\t\t<trigger>\n\t\t\t<deadzone>0.25</deadzone>\n\t\t\t<range>1</range>\n\t\t</trigger>\n\t\t<mappings>\n{entries}\t\t</mappings>\n\t</controller>\n</emulated_controller>\n",
                p.pad.index,
                p.pad.sdl_guid(true),
                p.pad.sdl_name()
            );
            Edit::Whole {
                file: cx
                    .root
                    .join(format!("controllerProfiles/controller{n}.xml")),
                content,
            }
        })
        .collect();
    Ok(Bindings {
        edits,
        note: join(beyond(players, 8, "Cemu"), guid_note(cx.os)),
    })
}
