//! The consumer's pads written into an emulator's bindings, seat by seat, and taken out
//! again. Every file is snapshotted before its first edit, so `revert` gives the player's own
//! settings back byte for byte. An adapter is a function from pads to edits; the snapshot,
//! the write and the report are shared.
//!
//! Every pad here is an SDL game controller (the consumer's virtual Xbox pad in practice), so
//! the neutral layout is SDL's: the emulators that key on a GUID get SDL's GUID computed from
//! the USB identity, the ones that key on an index get the pad's position.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{Install, PrepareStep, Prepared, StepOutcome};
use crate::prepare::ini_set;

/// A pad as the consumer sees it: its USB identity and the order it appeared in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PadRef {
    /// The kernel's device name (`Microsoft X-Box 360 pad`), what evdev shows.
    pub name: String,
    /// USB (3) unless the pad says otherwise; part of SDL's GUID.
    #[serde(default = "usb")]
    pub bus: u16,
    pub vendor: u16,
    pub product: u16,
    #[serde(default)]
    pub version: u16,
    /// Position among the pads at launch, 0-based. SDL and evdev number devices that way.
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evdev: Option<PathBuf>,
}

fn usb() -> u16 {
    3
}

/// One seat: player `seat` (1-based) plays on `pad`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Player {
    pub seat: u8,
    pub pad: PadRef,
}

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

/// One change to one file.
#[derive(Clone, Debug, PartialEq)]
enum Edit {
    /// `key` in `[section]`; an empty section is the top of the file.
    Ini {
        file: PathBuf,
        section: String,
        key: String,
        value: String,
    },
    /// The whole file, ours.
    Whole { file: PathBuf, content: String },
}

impl Edit {
    fn file(&self) -> &Path {
        match self {
            Edit::Ini { file, .. } | Edit::Whole { file, .. } => file,
        }
    }
}

enum Plan {
    /// The emulator picks up SDL pads on its own, in the order they appear.
    Automatic,
    Unsupported(&'static str),
    Edits(Vec<Edit>),
}

/// Writes `players` into the copy's bindings; every file touched is snapshotted first under
/// `snapshots/<emulator>/` unless it already is. One step per file.
pub fn apply(entry_id: &str, install: &Install, players: &[Player], snapshots: &Path) -> Prepared {
    let mut steps = Vec::new();
    let Some(root) = install.config_root.as_deref() else {
        steps.push(step(
            "players",
            Path::new(""),
            StepOutcome::Failed,
            Some("the catalog does not know where this copy keeps its config".into()),
        ));
        return done(entry_id, steps);
    };
    let edits = match plan(entry_id, root, install, players) {
        Plan::Automatic => {
            steps.push(step(
                "players",
                root,
                StepOutcome::Present,
                Some("the emulator maps SDL pads itself".into()),
            ));
            return done(entry_id, steps);
        }
        Plan::Unsupported(why) => {
            steps.push(step(
                "players",
                root,
                StepOutcome::Skipped,
                Some(why.into()),
            ));
            return done(entry_id, steps);
        }
        Plan::Edits(edits) => edits,
    };
    let mut snap = match Snapshot::open(snapshots, entry_id) {
        Ok(s) => s,
        Err(e) => {
            steps.push(step("players", root, StepOutcome::Failed, Some(e)));
            return done(entry_id, steps);
        }
    };
    let mut files: Vec<PathBuf> = Vec::new();
    for e in &edits {
        if !files.iter().any(|f| f == e.file()) {
            files.push(e.file().to_path_buf());
        }
    }
    for file in files {
        if let Err(e) = snap.keep(&file) {
            steps.push(step("players", &file, StepOutcome::Failed, Some(e)));
            continue;
        }
        let before = match std::fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                steps.push(step(
                    "players",
                    &file,
                    StepOutcome::Failed,
                    Some(e.to_string()),
                ));
                continue;
            }
        };
        let mut text = before.clone();
        for e in edits.iter().filter(|e| e.file() == file) {
            match e {
                Edit::Ini {
                    section,
                    key,
                    value,
                    ..
                } => {
                    if let Some(next) = ini_set(&text, section, key, value) {
                        text = next;
                    }
                }
                Edit::Whole { content, .. } => text = content.clone(),
            }
        }
        if text == before {
            steps.push(step("players", &file, StepOutcome::Present, None));
            continue;
        }
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| crate::prepare::write_atomic(&file, text.as_bytes()));
        steps.push(match written {
            Ok(()) => step("players", &file, StepOutcome::Applied, None),
            Err(e) => step("players", &file, StepOutcome::Failed, Some(e.to_string())),
        });
    }
    done(entry_id, steps)
}

/// Puts back every file `apply` snapshotted for `entry_id`, then forgets the snapshot. One
/// step per file; none when nothing was outstanding.
pub fn revert(snapshots: &Path, entry_id: &str) -> Vec<PrepareStep> {
    let snap = match Snapshot::open(snapshots, entry_id) {
        Ok(s) => s,
        Err(e) => return vec![step("revert", snapshots, StepOutcome::Failed, Some(e))],
    };
    let mut steps = Vec::new();
    for (file, kept) in &snap.manifest {
        let file = Path::new(file);
        let restored = match kept {
            Some(name) => std::fs::read(snap.dir.join("files").join(name))
                .and_then(|bytes| crate::prepare::write_atomic(file, &bytes)),
            None => match std::fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        };
        steps.push(match restored {
            Ok(()) => step("revert", file, StepOutcome::Applied, None),
            Err(e) => step("revert", file, StepOutcome::Failed, Some(e.to_string())),
        });
    }
    if steps.iter().all(|s| s.outcome != StepOutcome::Failed) {
        let _ = std::fs::remove_dir_all(&snap.dir);
    }
    steps
}

/// Emulators with a snapshot outstanding.
pub fn outstanding(snapshots: &Path) -> Vec<String> {
    std::fs::read_dir(snapshots)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().join("manifest.json").is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// `<snapshots>/<emulator>/manifest.json`: file → the copy of it under `files/`, or `None`
/// for a file that did not exist. The first snapshot of a file is the one that stays: a
/// second `apply` writes over hermir's own text, not the player's.
struct Snapshot {
    dir: PathBuf,
    manifest: BTreeMap<String, Option<String>>,
}

impl Snapshot {
    fn open(snapshots: &Path, entry_id: &str) -> std::result::Result<Snapshot, String> {
        let dir = snapshots.join(entry_id);
        let path = dir.join("manifest.json");
        let manifest = match std::fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        Ok(Snapshot { dir, manifest })
    }

    fn keep(&mut self, file: &Path) -> std::result::Result<(), String> {
        let key = file.to_string_lossy().into_owned();
        if self.manifest.contains_key(&key) {
            return Ok(());
        }
        let files = self.dir.join("files");
        std::fs::create_dir_all(&files).map_err(|e| format!("{}: {e}", files.display()))?;
        let kept = match std::fs::read(file) {
            Ok(bytes) => {
                let name = format!("{}.bak", self.manifest.len());
                let at = files.join(&name);
                std::fs::write(&at, bytes).map_err(|e| format!("{}: {e}", at.display()))?;
                Some(name)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", file.display())),
        };
        self.manifest.insert(key, kept);
        let path = self.dir.join("manifest.json");
        let json = serde_json::to_vec_pretty(&self.manifest).map_err(|e| e.to_string())?;
        crate::prepare::write_atomic(&path, &json).map_err(|e| format!("{}: {e}", path.display()))
    }
}

fn step(kind: &str, target: &Path, outcome: StepOutcome, note: Option<String>) -> PrepareStep {
    PrepareStep {
        kind: kind.into(),
        target: target.to_path_buf(),
        outcome,
        note,
    }
}

fn done(entry_id: &str, steps: Vec<PrepareStep>) -> Prepared {
    Prepared {
        emulator: entry_id.into(),
        steps,
    }
}

// ───────────────────────────────── adapters ─────────────────────────────────
//
// SDL game-controller numbering: buttons A 0, B 1, X 2, Y 3, Back 4, Guide 5, Start 6,
// LS 7, RS 8, LB 9, RB 10, D-pad up 11 down 12 left 13 right 14; axes LX 0, LY 1, RX 2,
// RY 3, LT 4, RT 5. Raw evdev/joystick order of the same pad: buttons A 0, B 1, X 2, Y 3,
// LB 4, RB 5, Back 6, Start 7, Guide 8, LS 9, RS 10; axes X 0, Y 1, LT 2, RX 3, RY 4, RT 5,
// hat X 6, hat Y 7. Nintendo layouts are mapped by position, so a face button keeps its place.

fn plan(id: &str, root: &Path, install: &Install, players: &[Player]) -> Plan {
    match id {
        "eden" => Plan::Edits(eden(&eden_settings(root).join("qt-config.ini"), players)),
        "azahar" => Plan::Edits(azahar(&root.join("qt-config.ini"), players)),
        "dolphin" => Plan::Edits(dolphin(root, players)),
        "melonds" => Plan::Edits(melonds(&root.join("melonDS.toml"), players)),
        "cemu" => Plan::Edits(cemu(root, players)),
        "pcsx2" => Plan::Edits(pad_ini(
            &root.join("inis/PCSX2.ini"),
            "DualShock2",
            &PCSX2_NAMES,
            players,
        )),
        "duckstation" => Plan::Edits(pad_ini(
            &root.join("settings.ini"),
            "AnalogController",
            &DUCKSTATION_NAMES,
            players,
        )),
        "rpcs3" => Plan::Edits(rpcs3(
            &root.join("input_configs/global/Default.yml"),
            players,
        )),
        "retroarch" => Plan::Edits(retroarch(&root.join("retroarch.cfg"), players)),
        "supermodel" => Plan::Edits(supermodel(&root.join("Config/Supermodel.ini"), players)),
        "xemu" => Plan::Edits(xemu(&root.join("xemu.toml"), players)),
        "ppsspp" | "flycast" | "xenia-canary" | "shadps4" | "vita3k" | "mgba" => Plan::Automatic,
        "dosbox-staging" | "scummvm" => Plan::Unsupported("keyboard and mouse games"),
        _ => {
            let _ = install;
            Plan::Unsupported("no player bindings for this emulator")
        }
    }
}

fn ini(file: &Path, section: &str, key: impl Into<String>, value: impl Into<String>) -> Edit {
    Edit::Ini {
        file: file.to_path_buf(),
        section: section.into(),
        key: key.into(),
        value: value.into(),
    }
}

/// Eden's settings sit beside its data: `<prefix>/config/eden` for a Flatpak or
/// `~/.config/eden`, while the catalog root is the data folder (keys, firmware).
fn eden_settings(root: &Path) -> PathBuf {
    let up = |n: usize| root.ancestors().nth(n).map(Path::to_path_buf);
    if root
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|d| d == "data")
    {
        return up(2).unwrap_or_default().join("config/eden");
    }
    if up(2).and_then(|p| p.file_name().map(|n| n == "share")) == Some(true) {
        return up(3).unwrap_or_default().join(".config/eden");
    }
    root.join("config")
}

fn eden(cfg: &Path, players: &[Player]) -> Vec<Edit> {
    let mut out = Vec::new();
    for p in players {
        let n = u32::from(p.seat) - 1;
        let (g, i) = (p.pad.sdl_guid(false), p.pad.index);
        let btn = |b: u32| format!("\"engine:sdl,guid:{g},port:{i},button:{b}\"");
        let axis = |a: u32| {
            format!("\"engine:sdl,guid:{g},port:{i},axis:{a},threshold:0.500000,invert:+\"")
        };
        let stick = |x: u32, y: u32| {
            format!(
                "\"engine:sdl,guid:{g},port:{i},axis_x:{x},axis_y:{y},offset_x:-0.000000,\
                 offset_y:-0.000000,invert_x:+,invert_y:+,deadzone:0.150000,range:0.950000,\
                 threshold:0.500000\""
            )
        };
        let binds = [
            ("button_a", btn(1)),
            ("button_b", btn(0)),
            ("button_x", btn(3)),
            ("button_y", btn(2)),
            ("button_lstick", btn(7)),
            ("button_rstick", btn(8)),
            ("button_l", btn(9)),
            ("button_r", btn(10)),
            ("button_zl", axis(4)),
            ("button_zr", axis(5)),
            ("button_plus", btn(6)),
            ("button_minus", btn(4)),
            ("button_dleft", btn(13)),
            ("button_dup", btn(11)),
            ("button_dright", btn(14)),
            ("button_ddown", btn(12)),
            ("button_slleft", btn(9)),
            ("button_srleft", btn(10)),
            ("button_slright", btn(9)),
            ("button_srright", btn(10)),
            ("button_home", btn(5)),
            ("button_screenshot", btn(15)),
            ("lstick", stick(0, 1)),
            ("rstick", stick(2, 3)),
        ];
        out.push(ini(cfg, "Controls", format!("player_{n}_type"), "0"));
        out.push(ini(
            cfg,
            "Controls",
            format!("player_{n}_connected"),
            "true",
        ));
        for (key, value) in binds {
            out.push(ini(cfg, "Controls", format!("player_{n}_{key}"), value));
            out.push(ini(
                cfg,
                "Controls",
                format!("player_{n}_{key}\\default"),
                "false",
            ));
        }
    }
    out
}

fn azahar(cfg: &Path, players: &[Player]) -> Vec<Edit> {
    let Some(p) = players.first() else {
        return Vec::new();
    };
    let (g, i) = (p.pad.sdl_guid(true), p.pad.index);
    let btn = |b: u32| format!("\"api:controller,button:{b},engine:sdl,guid:{g},port:{i}\"");
    let axis = |a: u32| {
        format!(
            "\"api:controller,axis:{a},direction:+,engine:sdl,guid:{g},port:{i},threshold:0.500000\""
        )
    };
    let stick = |x: u32, y: u32| {
        format!("\"api:controller,axis_x:{x},axis_y:{y},engine:sdl,guid:{g},port:{i}\"")
    };
    let binds = [
        ("button_a", btn(1)),
        ("button_b", btn(0)),
        ("button_x", btn(3)),
        ("button_y", btn(2)),
        ("button_l", btn(9)),
        ("button_r", btn(10)),
        ("button_zl", axis(4)),
        ("button_zr", axis(5)),
        ("button_start", btn(6)),
        ("button_select", btn(4)),
        ("button_home", btn(5)),
        ("button_up", btn(11)),
        ("button_down", btn(12)),
        ("button_left", btn(13)),
        ("button_right", btn(14)),
        ("circle_pad", stick(0, 1)),
        ("c_stick", stick(2, 3)),
    ];
    let mut out = Vec::new();
    for (key, value) in binds {
        out.push(ini(cfg, "Controls", format!("profiles\\1\\{key}"), value));
        out.push(ini(
            cfg,
            "Controls",
            format!("profiles\\1\\{key}\\default"),
            "false",
        ));
    }
    out
}

/// Dolphin through its evdev backend, which names a pad's controls by position: `Button 0`,
/// `Axis 1-`. A GameCube pad on the sticks and face buttons; a Wii Remote with Nunchuk,
/// pointer on the right stick, B on the right trigger.
fn dolphin(root: &Path, players: &[Player]) -> Vec<Edit> {
    let gc = root.join("GCPadNew.ini");
    let wii = root.join("WiimoteNew.ini");
    let mut out = Vec::new();
    for p in players {
        let dev = format!("evdev/{}/{}", p.pad.index, p.pad.name);
        let g = format!("GCPad{}", p.seat);
        let w = format!("Wiimote{}", p.seat);
        let hat = [
            ("D-Pad/Up", "`Axis 7-`"),
            ("D-Pad/Down", "`Axis 7+`"),
            ("D-Pad/Left", "`Axis 6-`"),
            ("D-Pad/Right", "`Axis 6+`"),
        ];
        let gc_binds = [
            ("Device", dev.as_str()),
            ("Buttons/A", "`Button 0`"),
            ("Buttons/B", "`Button 2`"),
            ("Buttons/X", "`Button 1`"),
            ("Buttons/Y", "`Button 3`"),
            ("Buttons/Z", "`Button 5`"),
            ("Buttons/Start", "`Button 7`"),
            ("Main Stick/Up", "`Axis 1-`"),
            ("Main Stick/Down", "`Axis 1+`"),
            ("Main Stick/Left", "`Axis 0-`"),
            ("Main Stick/Right", "`Axis 0+`"),
            ("C-Stick/Up", "`Axis 4-`"),
            ("C-Stick/Down", "`Axis 4+`"),
            ("C-Stick/Left", "`Axis 3-`"),
            ("C-Stick/Right", "`Axis 3+`"),
            ("Triggers/L", "`Axis 2+`"),
            ("Triggers/R", "`Axis 5+`"),
            ("Triggers/L-Analog", "`Axis 2+`"),
            ("Triggers/R-Analog", "`Axis 5+`"),
            ("Rumble/Motor", "`Motor`"),
        ];
        for (k, v) in gc_binds.iter().chain(hat.iter()) {
            out.push(ini(&gc, &g, *k, *v));
        }
        let wii_binds = [
            ("Source", "1"),
            ("Device", dev.as_str()),
            ("Extension", "Nunchuk"),
            ("Buttons/A", "`Button 0`"),
            ("Buttons/B", "`Axis 5+`"),
            ("Buttons/1", "`Button 2`"),
            ("Buttons/2", "`Button 3`"),
            ("Buttons/-", "`Button 6`"),
            ("Buttons/+", "`Button 7`"),
            ("Buttons/Home", "`Button 8`"),
            ("IR/Up", "`Axis 4-`"),
            ("IR/Down", "`Axis 4+`"),
            ("IR/Left", "`Axis 3-`"),
            ("IR/Right", "`Axis 3+`"),
            ("Shake/X", "`Button 1`"),
            ("Shake/Y", "`Button 1`"),
            ("Shake/Z", "`Button 1`"),
            ("Nunchuk/Buttons/C", "`Button 4`"),
            ("Nunchuk/Buttons/Z", "`Axis 2+`"),
            ("Nunchuk/Stick/Up", "`Axis 1-`"),
            ("Nunchuk/Stick/Down", "`Axis 1+`"),
            ("Nunchuk/Stick/Left", "`Axis 0-`"),
            ("Nunchuk/Stick/Right", "`Axis 0+`"),
            ("Nunchuk/Shake/X", "`Button 10`"),
            ("Nunchuk/Shake/Y", "`Button 10`"),
            ("Nunchuk/Shake/Z", "`Button 10`"),
            ("Rumble/Motor", "`Motor`"),
        ];
        for (k, v) in wii_binds.iter().chain(hat.iter()) {
            out.push(ini(&wii, &w, *k, *v));
        }
    }
    out
}

/// melonDS keys a raw SDL joystick: a button is its number, a hat direction is
/// `0x100 | hat << 4 | direction` (up 1, right 2, down 4, left 8). One player.
fn melonds(cfg: &Path, players: &[Player]) -> Vec<Edit> {
    let Some(p) = players.first() else {
        return Vec::new();
    };
    let mut out = vec![ini(cfg, "Instance0", "JoystickID", p.pad.index.to_string())];
    let binds = [
        ("A", 1),
        ("B", 0),
        ("X", 3),
        ("Y", 2),
        ("L", 4),
        ("R", 5),
        ("Select", 6),
        ("Start", 7),
        ("Up", 0x101),
        ("Right", 0x102),
        ("Down", 0x104),
        ("Left", 0x108),
    ];
    for (k, v) in binds {
        out.push(ini(cfg, "Instance0.Joystick", k, v.to_string()));
    }
    out
}

/// Cemu loads `controllerProfiles/controller<n>.xml` for player n+1: one SDL controller,
/// keyed `<index>_<guid>`, on a GamePad for player 1 and Pro Controllers after. Mapping ids
/// are Cemu's Wii U buttons; button ids are SDL's, plus Cemu's axis halves (38–49).
fn cemu(root: &Path, players: &[Player]) -> Vec<Edit> {
    players
        .iter()
        .map(|p| {
            let n = u32::from(p.seat) - 1;
            let kind = if p.seat == 1 {
                "Wii U GamePad"
            } else {
                "Wii U Pro Controller"
            };
            // (Cemu mapping, SDL button or Cemu axis id)
            let map: [(u32, u32); 26] = [
                (1, 1),
                (2, 0),
                (3, 3),
                (4, 2),
                (5, 9),
                (6, 10),
                (7, 42),
                (8, 43),
                (9, 6),
                (10, 4),
                (11, 11),
                (12, 12),
                (13, 13),
                (14, 14),
                (15, 7),
                (16, 8),
                (17, 45),
                (18, 39),
                (19, 44),
                (20, 38),
                (21, 47),
                (22, 41),
                (23, 46),
                (24, 40),
                (26, 15),
                (27, 5),
            ];
            let entries: String = map
                .iter()
                .map(|(m, b)| {
                    format!(
                        "\t\t\t<entry>\n\t\t\t\t<mapping>{m}</mapping>\n\t\t\t\t<button>{b}</button>\n\t\t\t</entry>\n"
                    )
                })
                .collect();
            let content = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<emulated_controller>\n\t<type>{kind}</type>\n\t<controller>\n\t\t<api>SDLController</api>\n\t\t<uuid>{}_{}</uuid>\n\t\t<display_name>{}</display_name>\n\t\t<rumble>0</rumble>\n\t\t<axis>\n\t\t\t<deadzone>0.25</deadzone>\n\t\t\t<range>1</range>\n\t\t</axis>\n\t\t<rotation>\n\t\t\t<deadzone>0.25</deadzone>\n\t\t\t<range>1</range>\n\t\t</rotation>\n\t\t<trigger>\n\t\t\t<deadzone>0.25</deadzone>\n\t\t\t<range>1</range>\n\t\t</trigger>\n\t\t<mappings>\n{entries}\t\t</mappings>\n\t</controller>\n</emulated_controller>\n",
                p.pad.index,
                p.pad.sdl_guid(true),
                p.pad.sdl_name()
            );
            Edit::Whole {
                file: root.join(format!("controllerProfiles/controller{n}.xml")),
                content,
            }
        })
        .collect()
}

/// PCSX2 and DuckStation share one input core: `SDL-<index>/<control>` per emulated button,
/// with the face buttons named differently in each.
struct PadNames {
    south: &'static str,
    east: &'static str,
    west: &'static str,
    north: &'static str,
}

const PCSX2_NAMES: PadNames = PadNames {
    south: "FaceSouth",
    east: "FaceEast",
    west: "FaceWest",
    north: "FaceNorth",
};

const DUCKSTATION_NAMES: PadNames = PadNames {
    south: "A",
    east: "B",
    west: "X",
    north: "Y",
};

fn pad_ini(cfg: &Path, kind: &str, names: &PadNames, players: &[Player]) -> Vec<Edit> {
    let mut out = vec![ini(cfg, "InputSources", "SDL", "true")];
    for p in players {
        let sec = format!("Pad{}", p.seat);
        let d = format!("SDL-{}", p.pad.index);
        let binds = [
            ("Type", kind.to_string()),
            ("Up", format!("{d}/DPadUp")),
            ("Right", format!("{d}/DPadRight")),
            ("Down", format!("{d}/DPadDown")),
            ("Left", format!("{d}/DPadLeft")),
            ("Triangle", format!("{d}/{}", names.north)),
            ("Circle", format!("{d}/{}", names.east)),
            ("Cross", format!("{d}/{}", names.south)),
            ("Square", format!("{d}/{}", names.west)),
            ("Select", format!("{d}/Back")),
            ("Start", format!("{d}/Start")),
            ("L1", format!("{d}/LeftShoulder")),
            ("L2", format!("{d}/+LeftTrigger")),
            ("R1", format!("{d}/RightShoulder")),
            ("R2", format!("{d}/+RightTrigger")),
            ("L3", format!("{d}/LeftStick")),
            ("R3", format!("{d}/RightStick")),
            ("Analog", format!("{d}/Guide")),
            ("LUp", format!("{d}/-LeftY")),
            ("LRight", format!("{d}/+LeftX")),
            ("LDown", format!("{d}/+LeftY")),
            ("LLeft", format!("{d}/-LeftX")),
            ("RUp", format!("{d}/-RightY")),
            ("RRight", format!("{d}/+RightX")),
            ("RDown", format!("{d}/+RightY")),
            ("RLeft", format!("{d}/-RightX")),
            ("LargeMotor", format!("{d}/LargeMotor")),
            ("SmallMotor", format!("{d}/SmallMotor")),
        ];
        for (k, v) in binds {
            out.push(ini(cfg, &sec, k, v));
        }
    }
    out
}

/// RPCS3's global input config, whole: its SDL handler names a device `<SDL name> <n>` with n
/// counting same-named pads from 1, and takes SDL's control names.
fn rpcs3(cfg: &Path, players: &[Player]) -> Vec<Edit> {
    let binds = [
        ("Left Stick Left", "LS X-"),
        ("Left Stick Down", "LS Y-"),
        ("Left Stick Right", "LS X+"),
        ("Left Stick Up", "LS Y+"),
        ("Right Stick Left", "RS X-"),
        ("Right Stick Down", "RS Y-"),
        ("Right Stick Right", "RS X+"),
        ("Right Stick Up", "RS Y+"),
        ("Start", "Start"),
        ("Select", "Back"),
        ("PS Button", "Guide"),
        ("Square", "West"),
        ("Cross", "South"),
        ("Circle", "East"),
        ("Triangle", "North"),
        ("Left", "Left"),
        ("Down", "Down"),
        ("Right", "Right"),
        ("Up", "Up"),
        ("R1", "RB"),
        ("R2", "RT"),
        ("R3", "RS"),
        ("L1", "LB"),
        ("L2", "LT"),
        ("L3", "LS"),
    ];
    let mut content = String::new();
    for seat in 1..=7u8 {
        match players.iter().find(|p| p.seat == seat) {
            Some(p) => {
                content.push_str(&format!(
                    "Player {seat} Input:\n  Handler: SDL\n  Device: \"{} {}\"\n  Config:\n",
                    p.pad.sdl_name(),
                    p.pad.index + 1
                ));
                for (k, v) in binds {
                    content.push_str(&format!("    {k}: {v}\n"));
                }
                content.push_str("  Buddy Device: \"Null\"\n");
            }
            None => content.push_str(&format!(
                "Player {seat} Input:\n  Handler: Null\n  Device: \"Null\"\n  Buddy Device: \"Null\"\n"
            )),
        }
    }
    vec![Edit::Whole {
        file: cfg.to_path_buf(),
        content,
    }]
}

/// RetroArch through its udev joypad driver, whose numbers are the pad's raw order; the same
/// binds its own profile for this pad would carry, so a missing autoconfig changes nothing.
fn retroarch(cfg: &Path, players: &[Player]) -> Vec<Edit> {
    let q = |s: &str| format!("\"{s}\"");
    let mut out = vec![
        ini(cfg, "", "input_joypad_driver", q("udev")),
        ini(cfg, "", "input_menu_toggle_btn", q("8")),
    ];
    for p in players {
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
            out.push(ini(cfg, "", format!("input_player{n}_{k}"), q(&v)));
        }
    }
    out
}

/// Supermodel on its SDL game-controller input system, where `JOYn_BUTTON1..10` are A, B, X,
/// Y, LB, RB, Back, Start, LS, RS and the axes are named. Start on Start and coin on Back,
/// the cabinet's buttons on the face buttons, pedals on the triggers, wheel on the stick.
fn supermodel(cfg: &Path, players: &[Player]) -> Vec<Edit> {
    let mut out = vec![ini(cfg, "Global", "InputSystem", "sdlgamepad")];
    for p in players.iter().filter(|p| p.seat <= 2) {
        let j = format!("JOY{}", p.pad.index + 1);
        let (s, k) = if p.seat == 1 { ("", "1") } else { ("2", "2") };
        let q = |v: String| format!("\"{v}\"");
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
            out.push(ini(cfg, "Global", k, v));
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
                out.push(ini(cfg, "Global", k, q(v)));
            }
        }
    }
    out
}

fn xemu(cfg: &Path, players: &[Player]) -> Vec<Edit> {
    players
        .iter()
        .filter(|p| p.seat <= 4)
        .map(|p| {
            ini(
                cfg,
                "input.bindings",
                format!("port{}", p.seat),
                format!("\"{}\"", p.pad.sdl_guid(true)),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Exe, InstallKind};

    fn one() -> Vec<Player> {
        vec![Player {
            seat: 1,
            pad: PadRef::xbox360(0),
        }]
    }

    fn install(id: &str, root: &Path) -> Install {
        Install {
            emulator: id.into(),
            kind: InstallKind::Flatpak,
            exe: Exe::FlatpakRun(id.into()),
            version: None,
            config_root: Some(root.to_path_buf()),
        }
    }

    #[test]
    fn sdl_guid_is_what_sdl_reports() {
        // Read back from SDL on a box with this pad plugged in.
        let pad = PadRef::xbox360(0);
        assert_eq!(pad.sdl_guid(true), "030081b85e0400008e02000010010000");
        assert_eq!(pad.sdl_guid(false), "030000005e0400008e02000010010000");
        assert_eq!(pad.sdl_name(), "Xbox 360 Controller");
    }

    #[test]
    fn eden_binds_the_pad_and_revert_removes_what_was_not_there() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("prefix/data/eden");
        let snaps = tmp.path().join("snaps");
        let inst = install("eden", &root);
        // Qt spells `key=value`; a file Eden wrote is what the adapter meets.
        let cfg = tmp.path().join("prefix/config/eden/qt-config.ini");
        let seed = "[UI]\nconfirmClose=true\n\n[Controls]\nplayer_0_type=0\n";
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, seed).unwrap();
        let p = apply("eden", &inst, &one(), &snaps);
        assert!(
            p.steps.iter().all(|s| s.outcome == StepOutcome::Applied),
            "{p:?}"
        );
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.contains(
            "player_0_button_a=\"engine:sdl,guid:030000005e0400008e02000010010000,port:0,button:1\"\n"
        ));
        assert!(text.contains("player_0_button_a\\default=false\n"));
        assert!(text.starts_with("[UI]\nconfirmClose=true\n\n[Controls]\nplayer_0_type=0\n"));
        let again = apply("eden", &inst, &one(), &snaps);
        assert!(
            again
                .steps
                .iter()
                .all(|s| s.outcome == StepOutcome::Present)
        );
        let r = revert(&snaps, "eden");
        assert_eq!(r.len(), 1);
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), seed);
        assert!(outstanding(&snaps).is_empty());
    }

    #[test]
    fn pcsx2_keeps_the_rest_of_the_file_and_revert_restores_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("PCSX2");
        let ini_path = root.join("inis/PCSX2.ini");
        std::fs::create_dir_all(ini_path.parent().unwrap()).unwrap();
        let before = "[UI]\nSettingsVersion = 1\n\n[Pad1]\nType = DualShock2\nCross = Keyboard/X\n";
        std::fs::write(&ini_path, before).unwrap();
        let snaps = tmp.path().join("snaps");
        let p = apply("pcsx2", &install("pcsx2", &root), &one(), &snaps);
        assert_eq!(p.steps.len(), 1);
        let text = std::fs::read_to_string(&ini_path).unwrap();
        assert!(text.starts_with("[UI]\nSettingsVersion = 1\n"));
        assert!(text.contains("Cross = SDL-0/FaceSouth\n"));
        assert!(text.contains("L2 = SDL-0/+LeftTrigger\n"));
        assert!(text.contains("[InputSources]\nSDL = true\n"));
        revert(&snaps, "pcsx2");
        assert_eq!(std::fs::read_to_string(&ini_path).unwrap(), before);
    }

    #[test]
    fn retroarch_writes_top_level_keys_and_supermodel_finds_its_spaced_header() {
        let tmp = tempfile::tempdir().unwrap();
        let ra = tmp.path().join("retroarch");
        std::fs::create_dir_all(&ra).unwrap();
        std::fs::write(
            ra.join("retroarch.cfg"),
            "input_player1_joypad_index = \"3\"\nvideo_fullscreen = \"true\"\n",
        )
        .unwrap();
        let snaps = tmp.path().join("snaps");
        apply("retroarch", &install("retroarch", &ra), &one(), &snaps);
        let text = std::fs::read_to_string(ra.join("retroarch.cfg")).unwrap();
        assert!(text.contains("input_player1_joypad_index = \"0\"\nvideo_fullscreen = \"true\"\n"));
        assert!(text.contains("input_player1_up_btn = \"h0up\"\n"));
        assert!(!text.contains('['));

        let sm = tmp.path().join("supermodel");
        std::fs::create_dir_all(sm.join("Config")).unwrap();
        std::fs::write(
            sm.join("Config/Supermodel.ini"),
            "[ Global ]\nInputStart1 = \"KEY_1,JOY1_BUTTON9\"\nNew3DEngine = 1\n",
        )
        .unwrap();
        apply("supermodel", &install("supermodel", &sm), &one(), &snaps);
        let text = std::fs::read_to_string(sm.join("Config/Supermodel.ini")).unwrap();
        assert!(
            text.starts_with("[ Global ]\nInputStart1 = \"KEY_1,JOY1_BUTTON8\"\nNew3DEngine = 1\n")
        );
        assert_eq!(text.matches("Global").count(), 1);
        assert!(text.contains("InputSystem = sdlgamepad\n"));
    }

    #[test]
    fn two_seats_land_in_order_everywhere_keyed_by_index() {
        let two = vec![
            Player {
                seat: 1,
                pad: PadRef::xbox360(0),
            },
            Player {
                seat: 2,
                pad: PadRef::xbox360(1),
            },
        ];
        let tmp = tempfile::tempdir().unwrap();
        let snaps = tmp.path().join("snaps");
        let az_cfg = tmp.path().join("azahar/qt-config.ini");
        std::fs::create_dir_all(az_cfg.parent().unwrap()).unwrap();
        std::fs::write(&az_cfg, "[Controls]\nprofile=0\n").unwrap();
        for id in [
            "dolphin",
            "cemu",
            "rpcs3",
            "duckstation",
            "azahar",
            "melonds",
            "xemu",
        ] {
            let root = tmp.path().join(id);
            let p = apply(id, &install(id, &root), &two, &snaps);
            assert!(
                p.steps.iter().all(|s| s.outcome == StepOutcome::Applied),
                "{id}: {p:?}"
            );
        }
        let gc = std::fs::read_to_string(tmp.path().join("dolphin/GCPadNew.ini")).unwrap();
        assert!(gc.contains("[GCPad2]\nDevice = evdev/1/Microsoft X-Box 360 pad\n"));
        let wii = std::fs::read_to_string(tmp.path().join("dolphin/WiimoteNew.ini")).unwrap();
        assert!(wii.contains("[Wiimote1]\nSource = 1\n"));
        let c1 =
            std::fs::read_to_string(tmp.path().join("cemu/controllerProfiles/controller1.xml"))
                .unwrap();
        assert!(c1.contains("<uuid>1_030081b85e0400008e02000010010000</uuid>"));
        assert!(c1.contains("<type>Wii U Pro Controller</type>"));
        let yml =
            std::fs::read_to_string(tmp.path().join("rpcs3/input_configs/global/Default.yml"))
                .unwrap();
        assert!(
            yml.contains("Player 2 Input:\n  Handler: SDL\n  Device: \"Xbox 360 Controller 2\"\n")
        );
        assert!(yml.contains("Player 3 Input:\n  Handler: Null\n"));
        let ds = std::fs::read_to_string(tmp.path().join("duckstation/settings.ini")).unwrap();
        assert!(ds.contains("[Pad2]\nType = AnalogController\nUp = SDL-1/DPadUp\n"));
        assert!(ds.contains("Cross = SDL-1/A\n"));
        let az = std::fs::read_to_string(tmp.path().join("azahar/qt-config.ini")).unwrap();
        assert!(az.contains(
            "profiles\\1\\button_a=\"api:controller,button:1,engine:sdl,guid:030081b85e0400008e02000010010000,port:0\"\n"
        ));
        let ds_toml = std::fs::read_to_string(tmp.path().join("melonds/melonDS.toml")).unwrap();
        assert!(ds_toml.contains("[Instance0.Joystick]\nA = 1\nB = 0\n"));
        assert!(ds_toml.contains("Up = 257\n"));
        let xemu = std::fs::read_to_string(tmp.path().join("xemu/xemu.toml")).unwrap();
        assert!(
            xemu.contains(
                "[input.bindings]\nport1 = \"030081b85e0400008e02000010010000\"\nport2 = "
            )
        );
        // A file that was not there before revert goes away with it.
        assert!(
            revert(&snaps, "melonds")
                .iter()
                .all(|s| s.outcome == StepOutcome::Applied)
        );
        assert!(!tmp.path().join("melonds/melonDS.toml").exists());
    }

    #[test]
    fn automatic_and_unsupported_emulators_say_so_without_touching_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let snaps = tmp.path().join("snaps");
        let root = tmp.path().join("ppsspp");
        let p = apply("ppsspp", &install("ppsspp", &root), &one(), &snaps);
        assert_eq!(p.steps[0].outcome, StepOutcome::Present);
        let p = apply("scummvm", &install("scummvm", &root), &one(), &snaps);
        assert_eq!(p.steps[0].outcome, StepOutcome::Skipped);
        assert!(!root.exists());
        assert!(outstanding(&snaps).is_empty());
    }
}
