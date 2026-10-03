//! Configuring a copy: the consumer's players and settings written into the emulator's own
//! files, snapshotted first, and put back on request. Format editors patch one key of one
//! file and know nothing about emulators; the catalog says which file and key each neutral
//! knob is; adapters know an emulator's player bindings and nothing about text; the
//! transaction between them is shared. Every result says what it did, knob by knob.
pub mod adapters;
pub mod ini;
pub mod json;
mod knobs;
mod txn;
pub mod xml;
pub mod yaml;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::model::{
    Applied, ConfigFile, Entry, Install, KNOBS, Knob, KnobChange, Os, Patch, PlayersSupport,
    PrepareStep, StepOutcome, Support,
};
pub(crate) use knobs::neutral_values;
pub(crate) use txn::Edit;
pub use txn::{outstanding, revert};

/// One copy, as the planners see it: the OS, the config root, the catalog's named files.
pub(crate) struct Cx<'a> {
    pub os: Os,
    pub root: &'a Path,
    pub files: &'a BTreeMap<String, ConfigFile>,
}

static NO_FILES: BTreeMap<String, ConfigFile> = BTreeMap::new();

impl Cx<'_> {
    /// The file the catalog calls `name`, resolved for this copy and OS.
    pub fn file(&self, name: &str) -> Result<(PathBuf, &ConfigFile), String> {
        let f = self
            .files
            .get(name)
            .ok_or_else(|| format!("the catalog names no `{name}` file"))?;
        let rel = f
            .path
            .get(self.os)
            .ok_or_else(|| format!("no `{name}` file on {}", self.os))?;
        Ok((resolve(self.root, rel), f))
    }
}

/// `rel` under `root`; `{config}/…` under the settings directory beside a data root.
pub(crate) fn resolve(root: &Path, rel: &str) -> PathBuf {
    match rel
        .strip_prefix("{config}/")
        .or_else(|| rel.strip_prefix("{config}\\"))
    {
        Some(rest) => settings_dir(root).join(rest),
        None => root.join(rel),
    }
}

/// The settings directory that goes with a data root: `~/.config/<x>` for `~/.local/share/<x>`,
/// `…/config/<x>` for a Flatpak's `…/data/<x>`, and the root itself when it is not a data root.
pub fn settings_dir(root: &Path) -> PathBuf {
    let Some(name) = root.file_name() else {
        return root.to_path_buf();
    };
    let named = |p: Option<&Path>, n: &str| p.and_then(Path::file_name).is_some_and(|d| d == n);
    let up = |n: usize| root.ancestors().nth(n);
    if named(up(1), "data")
        && let Some(base) = up(2)
    {
        return base.join("config").join(name);
    }
    if named(up(1), "share")
        && named(up(2), ".local")
        && let Some(home) = up(3)
    {
        return home.join(".config").join(name);
    }
    root.to_path_buf()
}

/// Writes `patch` into `install`'s files, every file snapshotted under `snapshots/<id>/` before
/// its first edit (see [`revert`]). One `KnobChange` per knob asked for, one step per file.
pub fn apply(entry: &Entry, os: Os, install: &Install, patch: &Patch, snapshots: &Path) -> Applied {
    let Some(root) = install.config_root.as_deref() else {
        return Applied {
            emulator: entry.id.clone(),
            knobs: Vec::new(),
            steps: vec![PrepareStep {
                kind: "config".into(),
                target: PathBuf::new(),
                outcome: StepOutcome::Failed,
                note: Some("the catalog does not know where this copy keeps its config".into()),
            }],
        };
    };
    let files = entry.config.as_ref().map_or(&NO_FILES, |c| &c.files);
    let cx = Cx { os, root, files };
    let mut planned = knobs::plan(entry, &cx, patch);
    if let Some(players) = &patch.players {
        planned.push(adapters::plan(entry, &cx, players));
    }
    for n in &patch.native {
        let knob = format!("native:{}:{}", n.file, n.key);
        planned.push(match cx.file(&n.file) {
            Ok((path, f)) => (
                KnobChange {
                    knob,
                    support: Support::Applied,
                    note: None,
                    file: Some(path.clone()),
                },
                Edit::set(&path, f, &n.section, &n.key, &n.value),
            ),
            Err(why) => (
                KnobChange {
                    knob,
                    support: Support::Unsupported,
                    note: Some(why),
                    file: None,
                },
                Vec::new(),
            ),
        });
    }
    let edits: Vec<Edit> = planned
        .iter()
        .flat_map(|(_, e)| e.iter().cloned())
        .collect();
    let steps = if edits.is_empty() {
        Vec::new()
    } else {
        txn::apply_edits(snapshots, &entry.id, &edits)
    };
    // A knob is what became of its files: one whose write failed says so, with the reason.
    let knobs = planned
        .into_iter()
        .map(|(mut change, edits)| {
            if let Some(failed) = steps.iter().find(|s| {
                s.outcome == StepOutcome::Failed && edits.iter().any(|e| e.file() == s.target)
            }) {
                change.support = Support::Failed;
                change.note = Some(match &failed.note {
                    Some(why) => format!("{}: {why}", failed.target.display()),
                    None => format!("{} could not be written", failed.target.display()),
                });
            }
            change
        })
        .collect();
    Applied {
        emulator: entry.id.clone(),
        knobs,
        steps,
    }
}

/// What `apply` can do for this emulator, knob by knob, from the catalog and the adapters:
/// the matrix a UI shows before asking.
pub fn support(entry: &Entry) -> Vec<KnobChange> {
    let cfg = entry.config.as_ref();
    let not_described = || Some(format!("not described for {} yet", entry.id));
    let mut out: Vec<KnobChange> = KNOBS
        .iter()
        .map(|&name| {
            let (support, note) = match cfg.and_then(|c| c.knobs.get(name)) {
                None => (Support::Unsupported, not_described()),
                Some(Knob::Unsupported { unsupported }) => {
                    (Support::Unsupported, Some(unsupported.clone()))
                }
                Some(Knob::Bound(b)) => {
                    let mut notes: Vec<String> = b.note.iter().cloned().collect();
                    if let Some(values) = &b.values {
                        let missing: Vec<&str> = knobs::neutral_values(name)
                            .iter()
                            .copied()
                            .filter(|v| !values.contains_key(*v))
                            .collect();
                        if !missing.is_empty() {
                            let have: Vec<&str> = values.keys().map(String::as_str).collect();
                            notes.push(format!("only {}", have.join(", ")));
                        }
                    }
                    if let Some(scale) = &b.scale {
                        let (min, max) = scale.range();
                        if (min, max) != (1, 8) {
                            notes.push(format!("{min}×–{max}×"));
                        }
                    }
                    if notes.is_empty() {
                        (Support::Applied, None)
                    } else {
                        (Support::Partial, Some(notes.join("; ")))
                    }
                }
            };
            KnobChange {
                knob: name.into(),
                support,
                note,
                file: None,
            }
        })
        .collect();
    let (support, note) = match cfg.and_then(|c| c.players.as_ref()) {
        None => (Support::Unsupported, not_described()),
        Some(PlayersSupport::Automatic { automatic }) => {
            (Support::Partial, Some(automatic.clone()))
        }
        Some(PlayersSupport::Unsupported { unsupported }) => {
            (Support::Unsupported, Some(unsupported.clone()))
        }
        Some(PlayersSupport::Adapter { adapter }) if adapters::exists(adapter) => {
            (Support::Applied, None)
        }
        Some(PlayersSupport::Adapter { adapter }) => (
            Support::Unsupported,
            Some(format!("no adapter named {adapter}")),
        ),
    };
    out.push(KnobChange {
        knob: "players".into(),
        support,
        note,
        file: None,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::model::{Aspect, Exe, InstallKind, Native, PadRef, Player, Region, Video};

    fn install(id: &str, root: &Path) -> Install {
        Install {
            emulator: id.into(),
            kind: InstallKind::Flatpak,
            exe: Exe::FlatpakRun(id.into()),
            version: None,
            config_root: Some(root.to_path_buf()),
        }
    }

    fn one() -> Vec<Player> {
        vec![Player {
            seat: 1,
            pad: PadRef::xbox360(0),
        }]
    }

    fn players(p: Vec<Player>) -> Patch {
        Patch {
            players: Some(p),
            ..Default::default()
        }
    }

    /// Fullscreen, 2×, vsync, 16:9, US.
    fn everything() -> Patch {
        Patch {
            video: Some(Video {
                fullscreen: Some(true),
                scale: Some(2),
                vsync: Some(true),
                aspect: Some(Aspect::SixteenNine),
            }),
            region: Some(Region::Usa),
            ..Default::default()
        }
    }

    fn run(c: &Catalog, id: &str, os: Os, root: &Path, patch: &Patch, snaps: &Path) -> Applied {
        apply(c.get(id).unwrap(), os, &install(id, root), patch, snaps)
    }

    fn knob<'a>(a: &'a Applied, name: &str) -> &'a KnobChange {
        a.knobs
            .iter()
            .find(|k| k.knob == name)
            .unwrap_or_else(|| panic!("no knob {name} in {a:?}"))
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    #[test]
    fn settings_dir_follows_the_xdg_and_flatpak_layouts() {
        assert_eq!(
            settings_dir(Path::new("/home/u/.local/share/eden")),
            PathBuf::from("/home/u/.config/eden")
        );
        assert_eq!(
            settings_dir(Path::new("/home/u/.var/app/dev.eden_emu.eden/data/eden")),
            PathBuf::from("/home/u/.var/app/dev.eden_emu.eden/config/eden")
        );
        assert_eq!(
            settings_dir(Path::new("/opt/shadps4/user")),
            PathBuf::from("/opt/shadps4/user")
        );
        assert_eq!(
            resolve(
                Path::new("/home/u/.local/share/eden"),
                "{config}/qt-config.ini"
            ),
            PathBuf::from("/home/u/.config/eden/qt-config.ini")
        );
        assert_eq!(
            resolve(Path::new("/r"), "inis/PCSX2.ini"),
            PathBuf::from("/r/inis/PCSX2.ini")
        );
    }

    #[test]
    fn eden_binds_the_pad_and_revert_removes_what_was_not_there() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("prefix/data/eden");
        let snaps = tmp.path().join("snaps");
        // Qt spells `key=value`; a file Eden wrote is what the adapter meets.
        let cfg = tmp.path().join("prefix/config/eden/qt-config.ini");
        let seed = "[UI]\nconfirmClose=true\n\n[Controls]\nplayer_0_type=0\n";
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, seed).unwrap();
        let p = run(&c, "eden", Os::Linux, &root, &players(one()), &snaps);
        assert_eq!(knob(&p, "players").support, Support::Applied);
        assert!(
            p.steps.iter().all(|s| s.outcome == StepOutcome::Applied),
            "{p:?}"
        );
        let text = read(&cfg);
        assert!(text.contains(
            "player_0_button_a=\"engine:sdl,guid:030000005e0400008e02000010010000,port:0,button:1\"\n"
        ));
        assert!(text.contains("player_0_button_a\\default=false\n"));
        assert!(text.starts_with("[UI]\nconfirmClose=true\n\n[Controls]\nplayer_0_type=0\n"));
        let again = run(&c, "eden", Os::Linux, &root, &players(one()), &snaps);
        assert!(
            again
                .steps
                .iter()
                .all(|s| s.outcome == StepOutcome::Present)
        );
        let r = revert(&snaps, "eden");
        assert_eq!(r.len(), 1);
        assert_eq!(read(&cfg), seed);
        assert!(outstanding(&snaps).is_empty());
    }

    #[test]
    fn pcsx2_takes_players_and_video_and_revert_restores_the_file() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("PCSX2");
        let ini = root.join("inis/PCSX2.ini");
        std::fs::create_dir_all(ini.parent().unwrap()).unwrap();
        let before = "[UI]\nSettingsVersion = 1\n\n[Pad1]\nType = DualShock2\nCross = Keyboard/X\n";
        std::fs::write(&ini, before).unwrap();
        let snaps = tmp.path().join("snaps");
        let p = run(&c, "pcsx2", Os::Linux, &root, &players(one()), &snaps);
        assert_eq!(p.steps.len(), 1);
        let text = read(&ini);
        assert!(text.starts_with("[UI]\nSettingsVersion = 1\n"));
        assert!(text.contains("Cross = SDL-0/FaceSouth\n"));
        assert!(text.contains("L2 = SDL-0/+LeftTrigger\n"));
        assert!(text.contains("[InputSources]\nSDL = true\n"));

        // A second apply, with settings, shares the snapshot: one revert undoes both.
        let mut patch = everything();
        patch.video.as_mut().unwrap().scale = Some(3);
        patch.region = Some(Region::Europe);
        let a = run(&c, "pcsx2", Os::Linux, &root, &patch, &snaps);
        assert_eq!(knob(&a, "video.scale").support, Support::Applied);
        assert_eq!(knob(&a, "video.scale").file.as_deref(), Some(ini.as_path()));
        let region = knob(&a, "region");
        assert_eq!(region.support, Support::Unsupported);
        assert!(region.note.as_deref().unwrap().contains("BIOS"));
        let text = read(&ini);
        assert!(
            text.contains("[UI]\nSettingsVersion = 1\nStartFullscreen = true\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "[EmuCore/GS]\nupscale_multiplier = 3\nVsyncEnable = true\nAspectRatio = 16:9\n"
            ),
            "{text}"
        );
        assert!(!a.failed());
        assert!(
            revert(&snaps, "pcsx2")
                .iter()
                .all(|s| s.outcome == StepOutcome::Applied)
        );
        assert_eq!(read(&ini), before);
        assert!(outstanding(&snaps).is_empty());
    }

    #[test]
    fn retroarch_writes_top_level_keys_and_supermodel_finds_its_spaced_header() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let ra = tmp.path().join("retroarch");
        std::fs::create_dir_all(&ra).unwrap();
        std::fs::write(
            ra.join("retroarch.cfg"),
            "input_player1_joypad_index = \"3\"\nvideo_fullscreen = \"true\"\n",
        )
        .unwrap();
        let snaps = tmp.path().join("snaps");
        run(&c, "retroarch", Os::Linux, &ra, &players(one()), &snaps);
        let text = read(&ra.join("retroarch.cfg"));
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
        run(&c, "supermodel", Os::Linux, &sm, &players(one()), &snaps);
        let text = read(&sm.join("Config/Supermodel.ini"));
        assert!(
            text.starts_with("[ Global ]\nInputStart1 = \"KEY_1,JOY1_BUTTON8\"\nNew3DEngine = 1\n")
        );
        assert_eq!(text.matches("Global").count(), 1);
        assert!(text.contains("InputSystem = sdlgamepad\n"));
    }

    #[test]
    fn two_seats_land_in_order_everywhere_keyed_by_index() {
        let c = Catalog::embedded().unwrap();
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
            let p = run(&c, id, Os::Linux, &root, &players(two.clone()), &snaps);
            assert!(
                p.steps.iter().all(|s| s.outcome == StepOutcome::Applied),
                "{id}: {p:?}"
            );
            assert_ne!(knob(&p, "players").support, Support::Unsupported, "{id}");
        }
        let gc = read(&tmp.path().join("dolphin/GCPadNew.ini"));
        assert!(gc.contains("[GCPad2]\nDevice = evdev/1/Microsoft X-Box 360 pad\n"));
        let wii = read(&tmp.path().join("dolphin/WiimoteNew.ini"));
        assert!(wii.contains("[Wiimote1]\nSource = 1\n"));
        let c1 = read(&tmp.path().join("cemu/controllerProfiles/controller1.xml"));
        assert!(c1.contains("<uuid>1_030081b85e0400008e02000010010000</uuid>"));
        assert!(c1.contains("<type>Wii U Pro Controller</type>"));
        let yml = read(&tmp.path().join("rpcs3/input_configs/global/Default.yml"));
        assert!(
            yml.contains("Player 2 Input:\n  Handler: SDL\n  Device: \"Xbox 360 Controller 2\"\n")
        );
        assert!(yml.contains("Player 3 Input:\n  Handler: Null\n"));
        let ds = read(&tmp.path().join("duckstation/settings.ini"));
        assert!(ds.contains("[Pad2]\nType = AnalogController\nUp = SDL-1/DPadUp\n"));
        assert!(ds.contains("Cross = SDL-1/A\n"));
        let az = read(&az_cfg);
        assert!(az.contains(
            "profiles\\1\\button_a=\"api:controller,button:1,engine:sdl,guid:030081b85e0400008e02000010010000,port:0\"\n"
        ));
        let ds_toml = read(&tmp.path().join("melonds/melonDS.toml"));
        assert!(ds_toml.contains("[Instance0.Joystick]\nA = 1\nB = 0\n"));
        assert!(ds_toml.contains("Up = 257\n"));
        let xemu = read(&tmp.path().join("xemu/xemu.toml"));
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
    fn windows_copies_get_their_own_backends_and_layouts() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let snaps = tmp.path().join("snaps");
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
        let dolphin = tmp.path().join("Dolphin Emulator");
        let p = run(
            &c,
            "dolphin",
            Os::Windows,
            &dolphin,
            &players(two.clone()),
            &snaps,
        );
        assert_eq!(knob(&p, "players").support, Support::Applied, "{p:?}");
        let gc = read(&dolphin.join("Config/GCPadNew.ini"));
        assert!(
            gc.contains("[GCPad2]\nDevice = XInput/1/Gamepad\nButtons/A = `Button A`\n"),
            "{gc}"
        );
        assert!(gc.contains("Main Stick/Up = `Left Y+`\n"));
        assert!(!dolphin.join("GCPadNew.ini").exists());
        let a = run(&c, "dolphin", Os::Windows, &dolphin, &everything(), &snaps);
        assert_eq!(knob(&a, "video.scale").support, Support::Applied);
        assert!(
            read(&dolphin.join("Config/GFX.ini"))
                .contains("[Settings]\nInternalResolution = 2\nAspectRatio = 1\n")
        );
        assert!(
            read(&dolphin.join("Config/Dolphin.ini")).contains("[Display]\nFullscreen = True\n")
        );

        let ra = tmp.path().join("RetroArch");
        run(&c, "retroarch", Os::Windows, &ra, &players(two), &snaps);
        let cfg = read(&ra.join("retroarch.cfg"));
        assert_eq!(
            cfg,
            "input_joypad_driver = \"xinput\"\ninput_player1_joypad_index = \"0\"\ninput_player2_joypad_index = \"1\"\n"
        );

        let xemu = tmp.path().join("xemu");
        let p = run(&c, "xemu", Os::Windows, &xemu, &players(one()), &snaps);
        let k = knob(&p, "players");
        assert_eq!(k.support, Support::Partial);
        assert!(k.note.as_deref().unwrap().contains("GUID"));

        // RPCS3 keeps config.yml and the input configs under config/ on Windows.
        let rpcs3 = tmp.path().join("RPCS3");
        run(&c, "rpcs3", Os::Windows, &rpcs3, &players(one()), &snaps);
        assert!(
            rpcs3
                .join("config/input_configs/global/Default.yml")
                .is_file()
        );
        run(&c, "rpcs3", Os::Windows, &rpcs3, &everything(), &snaps);
        assert!(rpcs3.join("config/config.yml").is_file());

        // No macOS layout is described: honest, not a crash.
        let a = run(&c, "dolphin", Os::Macos, &dolphin, &everything(), &snaps);
        let k = knob(&a, "video.fullscreen");
        assert_eq!(k.support, Support::Unsupported);
        assert!(k.note.as_deref().unwrap().contains("macos"));
    }

    #[test]
    fn automatic_and_unsupported_emulators_say_so_without_touching_anything() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let snaps = tmp.path().join("snaps");
        let root = tmp.path().join("ppsspp");
        let p = run(&c, "ppsspp", Os::Linux, &root, &players(one()), &snaps);
        assert_eq!(knob(&p, "players").support, Support::Partial);
        assert!(p.steps.is_empty());
        let p = run(&c, "scummvm", Os::Linux, &root, &players(one()), &snaps);
        assert_eq!(knob(&p, "players").support, Support::Unsupported);
        assert!(!root.exists());
        assert!(outstanding(&snaps).is_empty());
        let xemu = tmp.path().join("xemu");
        let five: Vec<Player> = (1..=5u8)
            .map(|seat| Player {
                seat,
                pad: PadRef::xbox360(u32::from(seat) - 1),
            })
            .collect();
        let p = run(&c, "xemu", Os::Linux, &xemu, &players(five), &snaps);
        let k = knob(&p, "players");
        assert_eq!(k.support, Support::Partial);
        assert!(
            k.note.as_deref().unwrap().contains("seat 5 left out"),
            "{k:?}"
        );
    }

    #[test]
    fn yaml_xml_and_json_emulators_take_the_video_knobs() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let snaps = tmp.path().join("snaps");

        let rpcs3 = tmp.path().join("rpcs3");
        let a = run(&c, "rpcs3", Os::Linux, &rpcs3, &everything(), &snaps);
        assert!(
            a.knobs.iter().all(|k| k.support == Support::Applied),
            "{a:?}"
        );
        assert_eq!(
            read(&rpcs3.join("config.yml")),
            "Miscellaneous:\n  Start games in fullscreen mode: true\nVideo:\n  Resolution Scale: 200\n  VSync Mode: Full\n  Aspect ratio: 16:9\nSystem:\n  License Area: SCEA\n"
        );

        let cemu = tmp.path().join("cemu");
        let a = run(&c, "cemu", Os::Linux, &cemu, &everything(), &snaps);
        for k in ["video.fullscreen", "video.scale", "region"] {
            assert_eq!(knob(&a, k).support, Support::Unsupported, "{k}");
        }
        assert_eq!(
            read(&cemu.join("settings.xml")),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<content>\n\t<Graphic>\n\t\t<VSync>1</VSync>\n\t\t<FullscreenScaling>0</FullscreenScaling>\n\t</Graphic>\n</content>\n"
        );

        let ryu = tmp.path().join("Ryujinx");
        let a = run(&c, "ryujinx", Os::Linux, &ryu, &everything(), &snaps);
        assert_eq!(knob(&a, "video.vsync").support, Support::Unsupported);
        assert_eq!(
            read(&ryu.join("Config.json")),
            "{\n  \"system_region\": \"USA\",\n  \"aspect_ratio\": \"Fixed16x9\",\n  \"res_scale\": 2,\n  \"start_fullscreen\": true\n}\n"
        );

        let vita = tmp.path().join("Vita3K");
        run(&c, "vita3k", Os::Linux, &vita, &everything(), &snaps);
        assert_eq!(
            read(&vita.join("config.yml")),
            "boot-apps-full-screen: true\nresolution-multiplier: 2\nv-sync: true\nstretch_the_display_area: false\n"
        );
    }

    #[test]
    fn ini_emulators_take_the_video_knobs_in_their_own_spelling() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let snaps = tmp.path().join("snaps");

        let pcsx2 = tmp.path().join("pcsx2");
        run(&c, "pcsx2", Os::Linux, &pcsx2, &everything(), &snaps);
        assert_eq!(
            read(&pcsx2.join("inis/PCSX2.ini")),
            "[UI]\nStartFullscreen = true\n\n[EmuCore/GS]\nupscale_multiplier = 2\nVsyncEnable = true\nAspectRatio = 16:9\n"
        );

        let ra = tmp.path().join("retroarch");
        run(&c, "retroarch", Os::Linux, &ra, &everything(), &snaps);
        assert_eq!(
            read(&ra.join("retroarch.cfg")),
            "video_fullscreen = \"true\"\nvideo_vsync = \"true\"\naspect_ratio_index = \"1\"\n"
        );

        // Eden: Qt spelling on a fresh file, a scale table, `{config}` beside a data root.
        let eden = tmp.path().join("home/.local/share/eden");
        let a = run(&c, "eden", Os::Linux, &eden, &everything(), &snaps);
        assert_eq!(knob(&a, "video.vsync").support, Support::Partial);
        let qt = read(&tmp.path().join("home/.config/eden/qt-config.ini"));
        assert!(
            qt.starts_with("[UI]\nfullscreen=true\nfullscreen\\default=false\n"),
            "{qt}"
        );
        assert!(
            qt.contains(
                "[Renderer]\nresolution_setup=4\nresolution_setup\\default=false\nvsync_mode=2\n"
            ),
            "{qt}"
        );
        assert!(
            qt.contains("[System]\nregion_index=1\nregion_index\\default=false\n"),
            "{qt}"
        );

        // Flycast: lines, yes/no, the region table; the 16:9 hack is partial.
        let fly = tmp.path().join("flycast");
        let a = run(&c, "flycast", Os::Linux, &fly, &everything(), &snaps);
        assert_eq!(knob(&a, "video.aspect").support, Support::Partial);
        assert_eq!(
            read(&fly.join("emu.cfg")),
            "[window]\nfullscreen = yes\n\n[config]\nrend.Resolution = 960\nrend.vsync = yes\nrend.WideScreen = yes\nDreamcast.Region = 1\nDreamcast.Broadcast = 0\n"
        );

        // Xenia: a second key with the same value; shadPS4: a second key with its own bool.
        let xenia = tmp.path().join("Xenia");
        run(&c, "xenia-canary", Os::Linux, &xenia, &everything(), &snaps);
        assert!(read(&xenia.join("xenia-canary.config.toml")).contains(
            "[GPU]\ndraw_resolution_scale_x = 2\ndraw_resolution_scale_y = 2\nvsync = true\n"
        ));
        let shad = tmp.path().join("home/.local/share/shadPS4");
        std::fs::create_dir_all(&shad).unwrap();
        let before = "{\n  \"General\": {\n    \"volume_slider\": 100\n  },\n  \"GPU\": {\n    \"full_screen\": false,\n    \"full_screen_mode\": \"Windowed\"\n  }\n}\n";
        std::fs::write(shad.join("config.json"), before).unwrap();
        run(&c, "shadps4", Os::Linux, &shad, &everything(), &snaps);
        assert_eq!(
            read(&shad.join("config.json")),
            before
                .replace("\"full_screen\": false", "\"full_screen\": true")
                .replace("\"Windowed\"", "\"Fullscreen\"")
        );

        // melonDS: the scale selects the renderer that scales, and says so.
        let melon = tmp.path().join("melonDS");
        let a = run(&c, "melonds", Os::Linux, &melon, &everything(), &snaps);
        let k = knob(&a, "video.scale");
        assert_eq!(k.support, Support::Partial);
        assert!(k.note.as_deref().unwrap().contains("OpenGL"));
        assert!(read(&melon.join("melonDS.toml")).contains("[3D.GL]\nScaleFactor = 2\n"));
        assert!(read(&melon.join("melonDS.toml")).contains("[3D]\nRenderer = 1\n"));
        assert_eq!(knob(&a, "video.fullscreen").support, Support::Unsupported);

        let duck = tmp.path().join("duckstation");
        run(&c, "duckstation", Os::Linux, &duck, &everything(), &snaps);
        let ini = read(&duck.join("settings.ini"));
        assert!(ini.contains("[Console]\nRegion = NTSC-U\n"), "{ini}");
        assert!(
            ini.contains("[Display]\nVSync = true\nAspectRatio = 16:9\n"),
            "{ini}"
        );
    }

    #[test]
    fn out_of_range_and_unknown_values_are_reported_not_written() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let snaps = tmp.path().join("snaps");
        let root = tmp.path().join("pcsx2");
        let patch = Patch {
            video: Some(Video {
                scale: Some(9),
                ..Default::default()
            }),
            ..Default::default()
        };
        let a = run(&c, "pcsx2", Os::Linux, &root, &patch, &snaps);
        let k = knob(&a, "video.scale");
        assert_eq!(k.support, Support::Unsupported);
        assert_eq!(k.note.as_deref(), Some("9× is outside pcsx2's 1×–8×"));
        assert!(a.steps.is_empty() && !root.exists());

        let patch = Patch {
            region: Some(Region::Auto),
            ..Default::default()
        };
        let a = run(
            &c,
            "rpcs3",
            Os::Linux,
            &tmp.path().join("rpcs3"),
            &patch,
            &snaps,
        );
        assert_eq!(
            knob(&a, "region").note.as_deref(),
            Some("rpcs3 has no region auto")
        );

        let a = run(
            &c,
            "mgba",
            Os::Linux,
            &tmp.path().join("mgba"),
            &everything(),
            &snaps,
        );
        assert!(
            knob(&a, "video.scale")
                .note
                .as_deref()
                .unwrap()
                .contains("2D")
        );
        assert_eq!(knob(&a, "video.fullscreen").support, Support::Applied);

        let patch = Patch {
            native: vec![
                Native {
                    file: "main".into(),
                    section: "EmuCore/GS".into(),
                    key: "upscale_multiplier".into(),
                    value: "1.5".into(),
                },
                Native {
                    file: "nope".into(),
                    section: String::new(),
                    key: "k".into(),
                    value: "v".into(),
                },
            ],
            ..Default::default()
        };
        let a = run(&c, "pcsx2", Os::Linux, &root, &patch, &snaps);
        assert_eq!(
            knob(&a, "native:main:upscale_multiplier").support,
            Support::Applied
        );
        assert_eq!(knob(&a, "native:nope:k").support, Support::Unsupported);
        assert_eq!(
            read(&root.join("inis/PCSX2.ini")),
            "[EmuCore/GS]\nupscale_multiplier = 1.5\n"
        );

        let no_root = Install {
            config_root: None,
            ..install("pcsx2", &root)
        };
        let a = apply(
            c.get("pcsx2").unwrap(),
            Os::Linux,
            &no_root,
            &everything(),
            &snaps,
        );
        assert!(a.failed() && a.knobs.is_empty());
    }

    #[test]
    fn a_bom_and_crlf_survive_an_apply() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("pcsx2");
        let ini = root.join("inis/PCSX2.ini");
        std::fs::create_dir_all(ini.parent().unwrap()).unwrap();
        std::fs::write(&ini, "\u{feff}[UI]\r\nSettingsVersion = 1\r\n").unwrap();
        let patch = Patch {
            video: Some(Video {
                fullscreen: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        run(
            &c,
            "pcsx2",
            Os::Linux,
            &root,
            &patch,
            &tmp.path().join("snaps"),
        );
        assert_eq!(
            read(&ini),
            "\u{feff}[UI]\r\nSettingsVersion = 1\r\nStartFullscreen = true\r\n"
        );
    }

    #[test]
    fn the_support_matrix_covers_every_knob_of_every_entry() {
        let c = Catalog::embedded().unwrap();
        for e in c.entries() {
            let s = support(e);
            assert_eq!(s.len(), KNOBS.len() + 1, "{}", e.id);
            for name in KNOBS.iter().copied().chain(std::iter::once("players")) {
                let k = s.iter().find(|k| k.knob == name).unwrap();
                if k.support == Support::Unsupported {
                    assert!(
                        k.note.is_some(),
                        "{}: {name} is unsupported without a reason",
                        e.id
                    );
                }
            }
        }
        let by = |id: &str, knob: &str| {
            support(c.get(id).unwrap())
                .into_iter()
                .find(|k| k.knob == knob)
                .unwrap()
        };
        assert_eq!(by("pcsx2", "video.fullscreen").support, Support::Applied);
        assert_eq!(by("pcsx2", "region").support, Support::Unsupported);
        assert_eq!(by("pcsx2", "players").support, Support::Applied);
        assert_eq!(by("ppsspp", "players").support, Support::Partial);
        assert_eq!(by("ryujinx", "players").support, Support::Unsupported);
        let aspect = by("rpcs3", "video.aspect");
        assert_eq!(aspect.support, Support::Partial);
        assert_eq!(aspect.note.as_deref(), Some("only 16:9, 4:3"));
        let scale = by("xenia-canary", "video.scale");
        assert_eq!(scale.note.as_deref(), Some("1×–3×"));
    }
}
