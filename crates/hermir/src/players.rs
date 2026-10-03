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
        }
    }

    /// SDL's joystick GUID, as its config strings spell it: bus, a CRC of the name, vendor,
    /// product, version, little-endian words. Eden zeroes the CRC; everyone else keeps it.
    pub fn sdl_guid(&self, name_crc: bool) -> String {
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

    /// What SDL calls the pad: its own name for the Xbox pads it knows, else the kernel's.
    pub fn sdl_name(&self) -> String {
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
}
