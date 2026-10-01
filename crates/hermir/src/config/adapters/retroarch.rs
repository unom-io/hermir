//! RetroArch: on Linux through its udev joypad driver, whose numbers are the pad's raw order,
//! with the same binds its own profile for this pad would carry, so a missing autoconfig
//! changes nothing. On Windows through XInput, whose pads its built-in profiles bind; only
//! the driver and each player's slot are written.
use super::{Bindings, Cx, Plan, beyond, set};
use crate::model::{Os, Player};

pub(super) fn players(cx: &Cx, players: &[Player]) -> Plan {
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
    edits.extend(set(cx, "main", "", "input_joypad_driver", &q("udev"))?);
    edits.extend(set(cx, "main", "", "input_menu_toggle_btn", &q("8"))?);
    for p in players.iter().filter(|p| p.seat <= 16) {
        let n = p.seat;
        let binds = [
            ("joypad_index", p.pad.index.to_string()),
            ("b_btn", "0".into()),
            ("a_btn", "1".into()),
            ("y_btn", "2".into()),
            ("x_btn", "3".into()),
            ("l_btn", "4".into()),
            ("r_btn", "5".into()),
            ("select_btn", "6".into()),
            ("start_btn", "7".into()),
            ("l3_btn", "9".into()),
            ("r3_btn", "10".into()),
            ("l2_axis", "+2".into()),
            ("r2_axis", "+5".into()),
            ("l_x_plus_axis", "+0".into()),
            ("l_x_minus_axis", "-0".into()),
            ("l_y_plus_axis", "+1".into()),
            ("l_y_minus_axis", "-1".into()),
            ("r_x_plus_axis", "+3".into()),
            ("r_x_minus_axis", "-3".into()),
            ("r_y_plus_axis", "+4".into()),
            ("r_y_minus_axis", "-4".into()),
            ("up_btn", "h0up".into()),
            ("down_btn", "h0down".into()),
            ("left_btn", "h0left".into()),
            ("right_btn", "h0right".into()),
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
