//! Pads as the consumer describes them. Every pad is an SDL game controller (the consumer's
//! virtual Xbox pad in practice), so the neutral layout is SDL's: the emulators that key on a
//! GUID get SDL's GUID computed from the USB identity, the ones that key on an index get the
//! pad's position. Writing them into an emulator is [`crate::config`]'s business.
use crate::model::PadRef;

impl PadRef {
    /// The wired Xbox 360 pad the Linux kernel's `xpad` table knows, at `index`.
    pub fn xbox360(index: u32) -> PadRef {
        PadRef {
            name: "Microsoft X-Box 360 pad".into(),
            bus: 3,
            vendor: 0x045e,
            product: 0x028e,
            version: 0x0110,
            index,
            evdev: None,
            guid: None,
            gamepad_name: None,
        }
    }

    /// SDL's joystick GUID, as its config strings spell it: bus, a CRC of the name, vendor,
    /// product, version, little-endian words. Eden zeroes the CRC; everyone else keeps it.
    /// The GUID SDL reported, when the pad carries it, is used as it is.
    pub fn sdl_guid(&self, name_crc: bool) -> String {
        if let Some(g) = &self.guid {
            let g = g.to_ascii_lowercase();
            return if name_crc || g.len() != 32 {
                g
            } else {
                format!("{}0000{}", &g[..4], &g[8..])
            };
        }
        let crc = if name_crc {
            crc16(self.name.as_bytes())
        } else {
            0
        };
        [
            self.bus,
            crc,
            self.vendor,
            0,
            self.product,
            0,
            self.version,
            0,
        ]
        .iter()
        .map(|w| format!("{:02x}{:02x}", w & 0xff, w >> 8))
        .collect()
    }

    /// What SDL calls the pad: the name SDL reported, when the pad carries it; else SDL's own
    /// name for the Xbox pads it knows, else the kernel's.
    pub fn sdl_name(&self) -> String {
        if let Some(n) = &self.gamepad_name {
            return n.clone();
        }
        match (self.vendor, self.product) {
            (0x045e, 0x028e) => "Xbox 360 Controller".into(),
            (0x045e, 0x02ea) => "Xbox One S Controller".into(),
            (0x045e, 0x0b00) => "Xbox One Elite 2 Controller".into(),
            _ => self.name.clone(),
        }
    }
}

/// A control of a game controller, by SDL's game-controller names: what an adapter that
/// binds raw numbers asks [`Raw::of`] for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Ctl {
    South,
    East,
    West,
    North,
    Back,
    Guide,
    Start,
    LeftStick,
    RightStick,
    LeftShoulder,
    RightShoulder,
    Up,
    Down,
    Left,
    Right,
    LeftX,
    LeftY,
    RightX,
    RightY,
    LeftTrigger,
    RightTrigger,
}

/// A hat direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// Where a control sits among a joystick's raw buttons, axes and hats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RawControl {
    Button(u32),
    Axis(u32),
    /// Hat 0, the only one a pad has.
    Hat(Dir),
}

/// How SDL numbers a pad's raw joystick controls. A pad SDL reads through evdev keeps the
/// kernel's order (the Xbox pads `xpad` and the host's uinput make: A B X Y LB RB Back Start
/// Guide LS RS, LT and RT on axes 2 and 5, the d-pad a hat). One SDL drives through HIDAPI
/// numbers buttons as SDL's game-controller enum and axes LX LY RX RY LT RT; SDL 2 sends its
/// d-pad as buttons 11–14, SDL 3 as hat 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Raw {
    Evdev,
    Hidapi { sdl3: bool },
}

impl Raw {
    pub(crate) fn of(self, c: Ctl) -> RawControl {
        use Ctl::*;
        use RawControl::{Axis, Button, Hat};
        let hidapi = matches!(self, Raw::Hidapi { .. });
        match c {
            South => Button(0),
            East => Button(1),
            West => Button(2),
            North => Button(3),
            Back => Button(if hidapi { 4 } else { 6 }),
            Guide => Button(if hidapi { 5 } else { 8 }),
            Start => Button(if hidapi { 6 } else { 7 }),
            LeftStick => Button(if hidapi { 7 } else { 9 }),
            RightStick => Button(if hidapi { 8 } else { 10 }),
            LeftShoulder => Button(if hidapi { 9 } else { 4 }),
            RightShoulder => Button(if hidapi { 10 } else { 5 }),
            Up | Down | Left | Right if self == (Raw::Hidapi { sdl3: false }) => Button(
                11 + [Up, Down, Left, Right]
                    .iter()
                    .position(|d| *d == c)
                    .unwrap_or(0) as u32,
            ),
            Up => Hat(Dir::Up),
            Down => Hat(Dir::Down),
            Left => Hat(Dir::Left),
            Right => Hat(Dir::Right),
            LeftX => Axis(0),
            LeftY => Axis(1),
            RightX => Axis(if hidapi { 2 } else { 3 }),
            RightY => Axis(if hidapi { 3 } else { 4 }),
            LeftTrigger => Axis(if hidapi { 4 } else { 2 }),
            RightTrigger => Axis(5),
        }
    }
}

impl PadRef {
    /// Whether SDL drives the pad through HIDAPI: its GUID says so with `h` in byte 14.
    pub(crate) fn hidapi(&self) -> bool {
        self.guid
            .as_deref()
            .and_then(|g| g.get(28..30))
            .is_some_and(|b| b.eq_ignore_ascii_case("68"))
    }

    /// How an emulator built on SDL 3 (`sdl3`) or SDL 2 numbers this pad's raw controls.
    pub(crate) fn raw(&self, sdl3: bool) -> Raw {
        if self.hidapi() {
            Raw::Hidapi { sdl3 }
        } else {
            Raw::Evdev
        }
    }
}

/// CRC-16/ARC, what `SDL_crc16` computes over a joystick's name.
fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |crc, &b| {
        (0..8).fold(crc ^ u16::from(b), |c, _| {
            if c & 1 == 1 {
                (c >> 1) ^ 0xA001
            } else {
                c >> 1
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdl_guid_is_what_sdl_reports() {
        // Read back from SDL on a box with this pad plugged in.
        let pad = PadRef::xbox360(0);
        assert_eq!(pad.sdl_guid(true), "030081b85e0400008e02000010010000");
        assert_eq!(pad.sdl_guid(false), "030000005e0400008e02000010010000");
        assert_eq!(pad.sdl_name(), "Xbox 360 Controller");
    }

    #[test]
    fn a_guid_sdl_reported_is_taken_as_it_is_and_eden_still_gets_its_crc_zeroed() {
        // A DualSense through SDL's HIDAPI driver: the 'h' in byte 14 is nothing the USB
        // identity says.
        let pad = PadRef {
            guid: Some("0300F8D24C050000E60C000000016800".into()),
            gamepad_name: Some("DualSense Wireless Controller".into()),
            ..PadRef::xbox360(1)
        };
        assert_eq!(pad.sdl_guid(true), "0300f8d24c050000e60c000000016800");
        assert_eq!(pad.sdl_guid(false), "030000004c050000e60c000000016800");
        assert_eq!(pad.sdl_name(), "DualSense Wireless Controller");
    }

    #[test]
    fn raw_numbers_follow_the_driver_sdl_reads_the_pad_through() {
        use RawControl::{Axis, Button, Hat};
        let xbox = PadRef::xbox360(0);
        let ds = PadRef {
            guid: Some("030057564c050000e60c000000016800".into()),
            ..PadRef::xbox360(0)
        };
        assert_eq!(xbox.raw(true), Raw::Evdev);
        let (ev, h2, h3) = (xbox.raw(true), ds.raw(false), ds.raw(true));
        assert_eq!(ev.of(Ctl::Start), Button(7));
        assert_eq!(h3.of(Ctl::Start), Button(6));
        assert_eq!(ev.of(Ctl::LeftShoulder), Button(4));
        assert_eq!(h2.of(Ctl::LeftShoulder), Button(9));
        assert_eq!(ev.of(Ctl::RightX), Axis(3));
        assert_eq!(h3.of(Ctl::RightX), Axis(2));
        assert_eq!(ev.of(Ctl::LeftTrigger), Axis(2));
        assert_eq!(h3.of(Ctl::LeftTrigger), Axis(4));
        assert_eq!(ev.of(Ctl::Up), Hat(Dir::Up));
        assert_eq!(h3.of(Ctl::Left), Hat(Dir::Left));
        assert_eq!(h2.of(Ctl::Up), Button(11));
        assert_eq!(h2.of(Ctl::Right), Button(14));
    }
}
