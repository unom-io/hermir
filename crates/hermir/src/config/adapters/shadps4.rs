//! shadPS4 seats SDL pads itself, four of them in the order they appear, and reads their gyro
//! and touchpad. What it needs told is to keep reading them while its window lacks focus,
//! which a streamed window can: `Input.background_controller_input` in `config.json`.
use super::{Bindings, Cx, Plan, Seating, beyond, set};

pub(super) fn players(cx: &Cx, s: &Seating) -> Plan {
    Ok(Bindings {
        edits: set(cx, "main", "Input", "background_controller_input", "true")?,
        note: beyond(&s.seats, 4, "shadPS4"),
    })
}
