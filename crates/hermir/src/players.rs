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

    /// Whether the pad's raw buttons and axes are numbered as the Xbox 360 pad's (`xpad` on
    /// Linux): a Microsoft pad, or one the kernel names as an X-Box pad. The emulators that
    /// bind raw numbers are written for that layout.
    pub(crate) fn xbox_layout(&self) -> bool {
        self.vendor == 0x045e || self.name.contains("X-Box") || self.name.contains("Xbox")
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
}
