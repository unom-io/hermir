//! Ryujinx: `input_config` in `Config.json`, a Pro Controller per seat on the SDL gamepad
//! backend, its motion the pad's gyro. A pad's id is `<n>-<GUID>`: SDL's GUID with the name
//! CRC zeroed, in .NET's byte order, n counting pads of that GUID from 0. Ryujinx rebuilds a
//! file it cannot load from its defaults, so only a `Config.json` it wrote is patched. A seat
//! without an entry is no player.
use serde_json::{Value, json};

use super::{Bindings, Cx, Edit, Plan, Seating, beyond, guid_note, join};
use crate::model::PadRef;

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    let players = &s.seats[..];
    let (path, _) = cx.file("main")?;
    if !path.is_file() {
        return Err("Ryujinx writes its Config.json on first start; start it once".into());
    }
    let entries: Vec<Value> = players
        .iter()
        .filter(|p| p.seat <= 8)
        .map(|p| {
            let n = s.ordinal(&p.pad, |q| q.sdl_guid(false));
            entry(
                p.seat,
                &format!("{n}-{}", dotnet(&p.pad)),
                &p.pad.sdl_name(),
            )
        })
        .collect();
    Ok(Bindings {
        edits: vec![Edit::JsonValue {
            file: path,
            key: "input_config".into(),
            value: Value::Array(entries),
        }],
        note: join(
            join(beyond(players, 8, "Ryujinx"), guid_note(cx.os, s)),
            s.guessed("Ryujinx", "GUID"),
        ),
    })
}

/// The pad's GUID, name CRC zeroed, as .NET's `Guid` spells those 16 bytes: the first three
/// groups little-endian.
fn dotnet(pad: &PadRef) -> String {
    let g = pad.sdl_guid(false);
    let b = |i: usize| g.get(i * 2..i * 2 + 2).unwrap_or("00");
    format!(
        "{}{}{}{}-{}{}-{}{}-{}{}-{}",
        b(3),
        b(2),
        b(1),
        b(0),
        b(5),
        b(4),
        b(7),
        b(6),
        b(8),
        b(9),
        g.get(20..32).unwrap_or("000000000000")
    )
}

/// One player's entry. Switch buttons by position: A east, B south, X north, Y west.
fn entry(seat: u8, id: &str, name: &str) -> Value {
    json!({
        "version": 1,
        "backend": "GamepadSDL2",
        "id": id,
        "name": name,
        "controller_type": "ProController",
        "player_index": format!("Player{seat}"),
        "deadzone_left": 0.1,
        "deadzone_right": 0.1,
        "range_left": 1.0,
        "range_right": 1.0,
        "trigger_threshold": 0.5,
        "left_joycon_stick": {
            "joystick": "Left", "stick_button": "LeftStick",
            "invert_stick_x": false, "invert_stick_y": false, "rotate90_cw": false
        },
        "right_joycon_stick": {
            "joystick": "Right", "stick_button": "RightStick",
            "invert_stick_x": false, "invert_stick_y": false, "rotate90_cw": false
        },
        "left_joycon": {
            "button_minus": "Minus", "button_l": "LeftShoulder", "button_zl": "LeftTrigger",
            "button_sl": "SingleLeftTrigger0", "button_sr": "SingleRightTrigger0",
            "dpad_up": "DpadUp", "dpad_down": "DpadDown",
            "dpad_left": "DpadLeft", "dpad_right": "DpadRight"
        },
        "right_joycon": {
            "button_plus": "Plus", "button_r": "RightShoulder", "button_zr": "RightTrigger",
            "button_sl": "SingleLeftTrigger1", "button_sr": "SingleRightTrigger1",
            "button_a": "B", "button_b": "A", "button_x": "Y", "button_y": "X"
        },
        "motion": {
            "motion_backend": "GamepadDriver", "sensitivity": 100,
            "gyro_deadzone": 1.0, "enable_motion": true
        },
        "rumble": { "strong_rumble": 1.0, "weak_rumble": 1.0, "enable_rumble": true },
        "led": { "enable_led": false, "turn_off_led": false, "use_rainbow": false, "led_color": 0 }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_is_the_guid_without_its_crc_in_dotnet_order() {
        let ds = PadRef {
            guid: Some("030057564c050000e60c000000016800".into()),
            ..PadRef::xbox360(0)
        };
        assert_eq!(dotnet(&ds), "00000003-054c-0000-e60c-000000016800");
        assert_eq!(
            dotnet(&PadRef::xbox360(0)),
            "00000003-045e-0000-8e02-000010010000"
        );
    }
}
