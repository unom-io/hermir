//! RetroArch: on Linux through its `sdl2` joypad driver, which numbers a pad's controls by
//! SDL's game-controller enum whatever the pad, and reads its gyro for the cores that take
//! one; each seat's slot is the pad's SDL index. On Windows through XInput, whose pads its
//! built-in profiles bind; only the driver and each player's slot are written.
use super::{Bindings, Cx, Plan, Seating, beyond, set};
use crate::model::Os;

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let q = |s: &str| format!("\"{s}\"");
    let mut edits = Vec::new();
    if cx.os == Os::Windows {
        edits.extend(set(cx, "main", "", "input_joypad_driver", &q("xinput"))?);
        for p in players.iter().filter(|p| p.seat <= 16) {
            edits.extend(set(
                cx,
                "main",
                "",
                &format!("input_player{}_joypad_index", p.seat),
                &q(&p.pad.index.to_string()),
            )?);
        }
        return Ok(Bindings {
            edits,
            note: beyond(players, 16, "RetroArch"),
        });
    }
    edits.extend(set(cx, "main", "", "input_joypad_driver", &q("sdl2"))?);
    // Guide opens the menu.
    edits.extend(set(cx, "main", "", "input_menu_toggle_btn", &q("5"))?);
    for p in players.iter().filter(|p| p.seat <= 16) {
        let n = p.seat;
        // RetroPad B is the south button, A east, Y west, X north.
        let binds = [
            ("joypad_index", p.pad.index.to_string()),
            ("b_btn", "0".into()),
            ("a_btn", "1".into()),
            ("y_btn", "2".into()),
            ("x_btn", "3".into()),
            ("select_btn", "4".into()),
            ("start_btn", "6".into()),
            ("l3_btn", "7".into()),
            ("r3_btn", "8".into()),
            ("l_btn", "9".into()),
            ("r_btn", "10".into()),
            ("up_btn", "11".into()),
            ("down_btn", "12".into()),
            ("left_btn", "13".into()),
            ("right_btn", "14".into()),
            ("l2_axis", "+4".into()),
            ("r2_axis", "+5".into()),
            ("l_x_plus_axis", "+0".into()),
            ("l_x_minus_axis", "-0".into()),
            ("l_y_plus_axis", "+1".into()),
            ("l_y_minus_axis", "-1".into()),
            ("r_x_plus_axis", "+2".into()),
            ("r_x_minus_axis", "-2".into()),
            ("r_y_plus_axis", "+3".into()),
            ("r_y_minus_axis", "-3".into()),
        ];
        for (k, v) in binds {
            edits.extend(set(
                cx,
                "main",
                "",
                &format!("input_player{n}_{k}"),
                &q(&v),
            )?);
        }
    }
    Ok(Bindings {
        edits,
        note: beyond(players, 16, "RetroArch"),
    })
}
