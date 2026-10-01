//! Supermodel on its SDL game-controller input system, where `JOYn_BUTTON1..10` are A, B, X,
//! Y, LB, RB, Back, Start, LS, RS and the axes are named. Start on Start and coin on Back,
//! the cabinet's buttons on the face buttons, pedals on the triggers, wheel on the stick.
//! Two players.
use super::{Bindings, Cx, Plan, beyond, set};
use crate::model::Player;

pub(super) fn players(cx: &Cx, players: &[Player]) -> Plan {
    let mut edits = set(cx, "main", "Global", "InputSystem", "sdlgamepad")?;
    let q = |v: String| format!("\"{v}\"");
    for p in players.iter().filter(|p| p.seat <= 2) {
        let j = format!("JOY{}", p.pad.index + 1);
        let (s, k) = if p.seat == 1 { ("", "1") } else { ("2", "2") };
        let binds = [
            (format!("InputStart{k}"), q(format!("KEY_{k},{j}_BUTTON8"))),
            (
                format!("InputCoin{k}"),
                q(format!("KEY_{},{j}_BUTTON7", 4 + p.seat)),
            ),
            (
                format!("InputJoyUp{s}"),
                q(format!("KEY_UP,{j}_POV1_UP,{j}_YAXIS_NEG")),
            ),
            (
                format!("InputJoyDown{s}"),
                q(format!("KEY_DOWN,{j}_POV1_DOWN,{j}_YAXIS_POS")),
            ),
            (
                format!("InputJoyLeft{s}"),
                q(format!("KEY_LEFT,{j}_POV1_LEFT,{j}_XAXIS_NEG")),
            ),
            (
                format!("InputJoyRight{s}"),
                q(format!("KEY_RIGHT,{j}_POV1_RIGHT,{j}_XAXIS_POS")),
            ),
            (format!("InputPunch{s}"), q(format!("KEY_A,{j}_BUTTON1"))),
            (format!("InputKick{s}"), q(format!("KEY_S,{j}_BUTTON2"))),
            (format!("InputGuard{s}"), q(format!("KEY_D,{j}_BUTTON3"))),
            (format!("InputEscape{s}"), q(format!("KEY_F,{j}_BUTTON4"))),
            (
                format!("InputShortPass{s}"),
                q(format!("KEY_A,{j}_BUTTON1")),
            ),
            (format!("InputLongPass{s}"), q(format!("KEY_S,{j}_BUTTON2"))),
            (format!("InputShoot{s}"), q(format!("KEY_D,{j}_BUTTON3"))),
        ];
        for (k, v) in binds {
            edits.extend(set(cx, "main", "Global", &k, &v)?);
        }
        if p.seat == 1 {
            let one = [
                ("InputSteering", format!("{j}_XAXIS")),
                ("InputAccelerator", format!("KEY_UP,{j}_RZAXIS_POS")),
                ("InputBrake", format!("KEY_DOWN,{j}_ZAXIS_POS")),
                ("InputGearShiftUp", format!("KEY_Y,{j}_BUTTON6")),
                ("InputGearShiftDown", format!("KEY_H,{j}_BUTTON5")),
                ("InputVR1", format!("KEY_A,{j}_BUTTON1")),
                ("InputVR2", format!("KEY_S,{j}_BUTTON2")),
                ("InputVR3", format!("KEY_D,{j}_BUTTON3")),
                ("InputVR4", format!("KEY_F,{j}_BUTTON4")),
                ("InputViewChange", format!("KEY_A,{j}_BUTTON1")),
                ("InputHandBrake", format!("KEY_S,{j}_BUTTON2")),
                ("InputShift", format!("KEY_A,{j}_BUTTON1")),
                ("InputBeat", format!("KEY_A,{j}_BUTTON1")),
                ("InputCharge", format!("KEY_S,{j}_BUTTON2")),
                ("InputJump", format!("KEY_D,{j}_BUTTON3")),
                ("InputAnalogJoyLeft", format!("KEY_LEFT,{j}_XAXIS_NEG")),
                ("InputAnalogJoyRight", format!("KEY_RIGHT,{j}_XAXIS_POS")),
                ("InputAnalogJoyUp", format!("KEY_UP,{j}_YAXIS_NEG")),
                ("InputAnalogJoyDown", format!("KEY_DOWN,{j}_YAXIS_POS")),
                ("InputAnalogJoyTrigger", format!("KEY_A,{j}_BUTTON1")),
                ("InputAnalogJoyEvent", format!("KEY_S,{j}_BUTTON2")),
                ("InputGunX", format!("MOUSE_XAXIS,{j}_RXAXIS")),
                ("InputGunY", format!("MOUSE_YAXIS,{j}_RYAXIS")),
                (
                    "InputTrigger",
                    format!("KEY_A,{j}_BUTTON1,MOUSE_LEFT_BUTTON"),
                ),
                (
                    "InputOffscreen",
                    format!("KEY_S,{j}_BUTTON2,MOUSE_RIGHT_BUTTON"),
                ),
            ];
            for (k, v) in one {
                edits.extend(set(cx, "main", "Global", k, &q(v))?);
            }
        }
    }
    Ok(Bindings {
        edits,
        note: beyond(players, 2, "Supermodel"),
    })
}
