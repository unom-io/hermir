//! The arguments clap leaves as text: pads and native keys.
use std::io::Read;

use hermir::{Error, Native, PadRef, Player, Result};

use crate::cli::PadArgs;

/// The players `--pad` or `--players` name; `None` when neither is given.
pub fn players(args: &PadArgs) -> Result<Option<Vec<Player>>> {
    if let Some(path) = &args.players {
        let text = if path.as_os_str() == "-" {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| Error::Invalid(format!("--players -: {e}")))?;
            s
        } else {
            std::fs::read_to_string(path)
                .map_err(|e| Error::Invalid(format!("--players {}: {e}", path.display())))?
        };
        let players = serde_json::from_str(&text)
            .map_err(|e| Error::Invalid(format!("--players: not a list of players: {e}")))?;
        return Ok(Some(players));
    }
    if args.pad.is_empty() {
        return Ok(None);
    }
    let mut out = Vec::new();
    for (spec, (seat, index)) in args.pad.iter().zip((1u8..).zip(0u32..)) {
        out.push(Player {
            seat,
            pad: pad(spec, index)?,
        });
    }
    Ok(Some(out))
}

/// `VID:PID[:NAME][@INDEX]` → a pad, at `index` unless the spec says another.
pub fn pad(spec: &str, index: u32) -> Result<PadRef> {
    let bad = |why: &str| Error::Invalid(format!("--pad {spec}: {why}"));
    let (body, index) = match spec.rsplit_once('@') {
        Some((body, i)) if !i.is_empty() && i.chars().all(|c| c.is_ascii_digit()) => (
            body,
            i.parse()
                .map_err(|_| bad("the index after @ is too large"))?,
        ),
        _ => (spec, index),
    };
    let mut parts = body.splitn(3, ':');
    let hex = |s: Option<&str>| {
        s.and_then(|s| u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
    };
    let (Some(vendor), Some(product)) = (hex(parts.next()), hex(parts.next())) else {
        return Err(bad("a pad is VID:PID[:NAME][@INDEX], hex ids"));
    };
    let name = parts.next().map(str::trim).filter(|n| !n.is_empty());
    let xbox = PadRef::xbox360(index);
    if (vendor, product) == (xbox.vendor, xbox.product) {
        return Ok(PadRef {
            name: name.map_or(xbox.name.clone(), String::from),
            ..xbox
        });
    }
    let Some(name) = name else {
        return Err(bad(
            "a pad other than the wired Xbox 360 one needs its name, as the kernel reports it \
             (VID:PID:NAME), or the whole pad through --players",
        ));
    };
    Ok(PadRef {
        name: name.into(),
        vendor,
        product,
        version: 0,
        ..xbox
    })
}

/// `section/key=value` → a native key for `file`; the last slash before `=` splits them.
pub fn native(file: &str, spec: &str) -> Result<Native> {
    let bad = || Error::Invalid(format!("--set {spec}: a native key is section/key=value"));
    let (left, value) = spec.split_once('=').ok_or_else(bad)?;
    let (section, key) = left.rsplit_once('/').unwrap_or(("", left));
    if key.trim().is_empty() {
        return Err(bad());
    }
    Ok(Native {
        file: file.into(),
        section: section.trim().into(),
        key: key.trim().into(),
        value: value.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_take_their_index_from_the_order_or_after_an_at() {
        let p = pad("045e:028e", 2).unwrap();
        assert_eq!((p.vendor, p.product, p.index), (0x045e, 0x028e, 2));
        assert_eq!(p, PadRef::xbox360(2));
        let p = pad("045e:028e:My Pad@5", 0).unwrap();
        assert_eq!((p.name.as_str(), p.index), ("My Pad", 5));
    }

    #[test]
    fn a_pad_that_is_not_the_xbox_one_carries_its_own_name_and_no_borrowed_version() {
        let p = pad("054c:0ce6:Sony Interactive Entertainment DualSense", 1).unwrap();
        assert_eq!(p.name, "Sony Interactive Entertainment DualSense");
        assert_eq!(
            (p.vendor, p.product, p.version, p.index),
            (0x054c, 0x0ce6, 0, 1)
        );
        assert!(pad("054c:0ce6", 0).is_err(), "no name, no guess");
        assert!(pad("nope", 0).is_err());
        let err = pad("045e", 0).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn native_keys_split_at_the_last_slash() {
        let n = native("main", "EmuCore/GS/upscale_multiplier=3").unwrap();
        assert_eq!(
            (n.section.as_str(), n.key.as_str(), n.value.as_str()),
            ("EmuCore/GS", "upscale_multiplier", "3")
        );
        let n = native("main", "video_fullscreen = \"true\"").unwrap();
        assert_eq!(
            (n.section.as_str(), n.key.as_str()),
            ("", "video_fullscreen")
        );
        assert!(native("main", "no-equals").is_err());
        assert!(native("main", "section/=1").is_err());
    }

    #[test]
    fn seats_follow_the_order_of_the_pads() {
        let args = PadArgs {
            pad: vec!["045e:028e".into(), "045e:028e@0".into()],
            players: None,
        };
        let p = players(&args).unwrap().unwrap();
        assert_eq!(
            p.iter().map(|p| (p.seat, p.pad.index)).collect::<Vec<_>>(),
            [(1, 0), (2, 0)]
        );
        assert!(
            players(&PadArgs {
                pad: vec![],
                players: None
            })
            .unwrap()
            .is_none()
        );
    }
}
