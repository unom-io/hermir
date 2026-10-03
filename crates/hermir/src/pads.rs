//! The pads connected now, as SDL sees them (the `enumerate` feature): the list a consumer
//! seats players from, and passes whole as [`Patch::connected`](crate::Patch::connected) so
//! the emulators that number pads by name or GUID get the numbers they will see.
//!
//! SDL runs on a thread of its own, started on the first call and kept for the process: the
//! `sdl3` crate wants every SDL call on the thread that first initialized it, and SDL keeps
//! track of pads plugged in and out between calls. A host that initializes SDL itself through
//! the `sdl3` crate on another thread first gets an error here instead.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};

use crate::error::{Error, Result};
use crate::model::PadRef;

/// Work for the SDL thread, given SDL and its game-controller subsystem, or why SDL is not
/// there.
type Job = Box<dyn FnOnce(std::result::Result<(&sdl3::Sdl, &sdl3::GamepadSubsystem), &str>) + Send>;

/// Every game controller connected now, in SDL's order: each pad's `index` is its position,
/// its `guid` and `gamepad_name` what SDL reports, its `name` and `evdev` the kernel's on
/// Linux. Joysticks SDL has no game-controller mapping for are left out.
pub fn enumerate() -> Result<Vec<PadRef>> {
    on_sdl(|sdl| sdl.map_err(str::to_string).and_then(|(_, gp)| list(gp)))?.map_err(Error::Pads)
}

/// Runs `job` on the SDL thread and returns what it returns.
fn on_sdl<T: Send + 'static>(
    job: impl FnOnce(std::result::Result<(&sdl3::Sdl, &sdl3::GamepadSubsystem), &str>) -> T
    + Send
    + 'static,
) -> Result<T> {
    static SDL: OnceLock<Mutex<Option<Sender<Job>>>> = OnceLock::new();
    let gone = || Error::Pads("the SDL thread is gone".into());
    let jobs = SDL
        .get_or_init(|| Mutex::new(spawn()))
        .lock()
        .map_err(|_| gone())?
        .clone()
        .ok_or_else(|| Error::Pads("the SDL thread could not be started".into()))?;
    let (tx, rx) = channel();
    jobs.send(Box::new(move |sdl| {
        let _ = tx.send(job(sdl));
    }))
    .map_err(|_| gone())?;
    rx.recv().map_err(|_| gone())
}

/// The SDL thread: initializes SDL's game-controller subsystem once, then runs each job.
fn spawn() -> Option<Sender<Job>> {
    let (tx, rx) = channel::<Job>();
    std::thread::Builder::new()
        .name("hermir-sdl".into())
        .spawn(move || {
            // A process with no window still sees pads, and SDL leaves SIGINT and SIGTERM to
            // the process: its own handlers would turn Ctrl-C into an event nobody reads.
            sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
            sdl3::hint::set("SDL_NO_SIGNAL_HANDLERS", "1");
            let sdl = sdl3::init()
                .map_err(|e| e.to_string())
                .and_then(|sdl| Ok((sdl.gamepad().map_err(|e| e.to_string())?, sdl)));
            for job in rx {
                match &sdl {
                    Ok((gp, sdl)) => job(Ok((sdl, gp))),
                    Err(e) => job(Err(e)),
                }
            }
        })
        .ok()
        .map(|_| tx)
}

fn list(gp: &sdl3::GamepadSubsystem) -> std::result::Result<Vec<PadRef>, String> {
    // Pads plugged in since the last call.
    gp.update();
    let ids = gp.gamepads().map_err(|e| e.to_string())?;
    Ok(ids
        .into_iter()
        .zip(0u32..)
        .map(|(id, index)| {
            let guid = gp.guid_for_id(id).string().to_ascii_lowercase();
            let gamepad_name = gp.name_for_id(id).ok().filter(|n| !n.is_empty());
            let path = gp.path_for_id(id).ok().filter(|p| !p.is_empty());
            let kernel = path
                .as_deref()
                .and_then(|p| kernel(Path::new("/sys/class"), p));
            PadRef {
                name: kernel
                    .as_ref()
                    .map(|k| k.name.clone())
                    .or_else(|| gamepad_name.clone())
                    .unwrap_or_default(),
                bus: bus(&guid).unwrap_or(3),
                vendor: gp.vendor_for_id(id).unwrap_or(0),
                product: gp.product_for_id(id).unwrap_or(0),
                version: gp.product_version_for_id(id).unwrap_or(0),
                index,
                evdev: kernel.and_then(|k| k.evdev),
                guid: (guid.len() == 32).then_some(guid),
                gamepad_name,
            }
        })
        .collect())
}

/// The bus SDL's GUID starts with, a little-endian word.
fn bus(guid: &str) -> Option<u16> {
    let lo = u16::from_str_radix(guid.get(0..2)?, 16).ok()?;
    let hi = u16::from_str_radix(guid.get(2..4)?, 16).ok()?;
    Some(lo | hi << 8)
}

/// What the kernel calls a pad, and its evdev node.
#[derive(Debug, PartialEq)]
struct Kernel {
    name: String,
    evdev: Option<PathBuf>,
}

/// The kernel's name and evdev node for the device SDL opened at `path`: `/dev/input/eventN`
/// for a pad SDL reads through evdev, `/dev/hidrawN` for one it drives through HIDAPI.
/// `class` is `/sys/class`, a parameter for the tests.
fn kernel(class: &Path, path: &str) -> Option<Kernel> {
    let node = Path::new(path).file_name()?.to_str()?;
    let read_name = |dir: &Path| {
        std::fs::read_to_string(dir.join("name"))
            .ok()
            .map(|n| n.trim_end().to_string())
            .filter(|n| !n.is_empty())
    };
    if node.starts_with("event") {
        return Some(Kernel {
            name: read_name(&class.join("input").join(node).join("device"))?,
            evdev: Some(PathBuf::from(path)),
        });
    }
    if node.starts_with("hidraw") {
        // The input device the same HID device made: hidrawN/device/input/inputM, with its
        // eventK beside its name.
        let inputs = class.join("hidraw").join(node).join("device").join("input");
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&inputs)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        dirs.sort();
        let input = dirs.into_iter().find(|d| read_name(d).is_some())?;
        let evdev = std::fs::read_dir(&input)
            .ok()?
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with("event"))
            .min()
            .map(|e| PathBuf::from("/dev/input").join(e));
        return Some(Kernel {
            name: read_name(&input)?,
            evdev,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pad_plugged_in_is_listed_with_what_sdl_says_about_it() {
        // CI has no pads: SDL starts and lists nothing, and a second call reuses its thread.
        assert_eq!(enumerate().unwrap(), Vec::new());
        // A virtual game controller, plugged in and out on the SDL thread.
        let pads = on_sdl(|sdl| {
            let (sdl, gp) = sdl.unwrap();
            use sdl3::gamepad::{Axis, Button};
            let desc = sdl3::joystick::VirtualJoystickDescription::new()
                .name("hermir test pad")
                .joystick_type(sdl3::joystick::JoystickType::Gamepad)
                .with_buttons([Button::South, Button::East, Button::West, Button::North])
                .with_buttons([Button::Back, Button::Guide, Button::Start])
                .with_axes([Axis::LeftX, Axis::LeftY, Axis::RightX, Axis::RightY]);
            let plugged = sdl
                .joystick()
                .unwrap()
                .attach_virtual_joystick(desc)
                .unwrap();
            let pads = list(gp);
            drop(plugged);
            (pads, list(gp))
        })
        .unwrap();
        let (with, without) = (pads.0.unwrap(), pads.1.unwrap());
        assert_eq!(with.len(), 1, "{with:?}");
        let pad = &with[0];
        assert_eq!(pad.index, 0);
        assert_eq!(pad.gamepad_name.as_deref(), Some("hermir test pad"));
        assert_eq!(pad.name, "hermir test pad", "no kernel device: SDL's name");
        let guid = pad.guid.as_deref().unwrap();
        assert_eq!(guid.len(), 32);
        assert_eq!(pad.sdl_guid(true), guid);
        assert_eq!(without, Vec::new());
    }

    #[test]
    fn the_kernel_name_comes_from_sysfs_for_evdev_and_hidraw_pads() {
        let tmp = tempfile::tempdir().unwrap();
        let class = tmp.path();
        let ev = class.join("input/event7/device");
        std::fs::create_dir_all(&ev).unwrap();
        std::fs::write(ev.join("name"), "Microsoft X-Box 360 pad\n").unwrap();
        assert_eq!(
            kernel(class, "/dev/input/event7"),
            Some(Kernel {
                name: "Microsoft X-Box 360 pad".into(),
                evdev: Some("/dev/input/event7".into()),
            })
        );
        let input = class.join("hidraw/hidraw3/device/input/input42");
        std::fs::create_dir_all(input.join("event21")).unwrap();
        std::fs::create_dir_all(input.join("event20")).unwrap();
        std::fs::write(
            input.join("name"),
            "Sony Interactive Entertainment DualSense Wireless Controller\n",
        )
        .unwrap();
        assert_eq!(
            kernel(class, "/dev/hidraw3"),
            Some(Kernel {
                name: "Sony Interactive Entertainment DualSense Wireless Controller".into(),
                evdev: Some("/dev/input/event20".into()),
            })
        );
        assert_eq!(kernel(class, "/dev/input/event9"), None);
        assert_eq!(kernel(class, "\\\\?\\HID#VID_045E"), None);
        assert_eq!(bus("03008fe54c0500"), Some(3));
        assert_eq!(bus("0500"), Some(5));
    }
}
