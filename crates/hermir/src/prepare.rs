//! Making a copy ready to play: the answers to its first-run questions, and its firmware
//! placed or installed. Every step is idempotent and reported; nothing is assumed.
//!
//! First-run answers are permanent, like clicking through the wizard once. They are not a
//! session override and there is nothing to revert.
use std::path::{Path, PathBuf};

use crate::channel::flatpak::Runner;
pub use crate::config::ini::{get as ini_get, set as ini_set};
use crate::model::{
    Entry, Exe, FirmwareInstall, FirstRun, Install, Os, PrepareStep, Prepared, StepOutcome,
};

/// Answers `install`'s first-run questions for `os`, then places `firmware` for `platform`:
/// copies into the emulator's firmware folder, or hands the matching file to its installer.
/// `firmware` is what the consumer's source holds for the platform, companions included.
pub fn prepare(
    entry: &Entry,
    os: Os,
    install: &Install,
    platform: Option<&str>,
    firmware: &[PathBuf],
    runner: &dyn Runner,
) -> Prepared {
    let mut steps = Vec::new();
    let Some(root) = install.config_root.as_deref() else {
        steps.push(PrepareStep {
            kind: "config_root".into(),
            target: PathBuf::new(),
            outcome: StepOutcome::Failed,
            note: Some("the catalog does not know where this copy keeps its config".into()),
        });
        return Prepared {
            emulator: entry.id.clone(),
            steps,
        };
    };
    for answer in entry.first_run.get(os).into_iter().flatten() {
        steps.push(first_run(root, install, answer));
    }
    if let (Some(fw), Some(platform)) = (&entry.firmware, platform)
        && let Some(need) = fw.platforms.get(platform)
    {
        let archive = |f: &PathBuf| {
            fw.unpack
                .as_ref()
                .is_some_and(|u| name_matches(f, &u.any_of))
        };
        if let Some(dir) = &fw.dir {
            let dir = if dir == "." {
                root.to_path_buf()
            } else {
                root.join(dir)
            };
            for file in firmware.iter().filter(|f| !archive(f)) {
                steps.push(place(file, &dir));
            }
            if !need.optional && !any_match(&dir, &need.any_of) {
                steps.push(missing(&dir, need.note.as_deref()));
            }
        }
        if let Some(u) = &fw.unpack
            && let Some(file) = firmware.iter().find(|f| archive(f))
        {
            steps.push(unpack(file, &root.join(&u.into), &u.present));
        }
        if let Some(inst) = &fw.install {
            let file = firmware.iter().find(|f| name_matches(f, &need.any_of));
            steps.push(run_installer(
                install,
                root,
                inst,
                file,
                need.note.as_deref(),
                runner,
            ));
        }
    }
    Prepared {
        emulator: entry.id.clone(),
        steps,
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

fn first_run(root: &Path, install: &Install, answer: &FirstRun) -> PrepareStep {
    match answer {
        FirstRun::Copy { copy, to } => {
            let dest = root.join(to);
            if dest.is_file() {
                return step("first_run", &dest, StepOutcome::Present, None);
            }
            let Exe::Path(exe) = &install.exe else {
                return step(
                    "first_run",
                    &dest,
                    StepOutcome::Failed,
                    Some("no program folder to copy from".into()),
                );
            };
            let src = exe.parent().unwrap_or(Path::new(".")).join(copy);
            let copied = dest
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::copy(&src, &dest));
            match copied {
                Ok(_) => step("first_run", &dest, StepOutcome::Applied, None),
                Err(e) => step(
                    "first_run",
                    &dest,
                    StepOutcome::Failed,
                    Some(format!("copy {}: {e}", src.display())),
                ),
            }
        }
        FirstRun::Ini {
            ini,
            section,
            key,
            value,
            when_unset,
        } => {
            let path = root.join(ini);
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    return step("first_run", &path, StepOutcome::Failed, Some(e.to_string()));
                }
            };
            if *when_unset && ini_get(&text, section, key).is_some_and(|v| !v.is_empty()) {
                return step("first_run", &path, StepOutcome::Present, None);
            }
            let value = &value.replace("{root}", &root.to_string_lossy());
            match ini_set(&text, section, key, value) {
                None => step("first_run", &path, StepOutcome::Present, None),
                Some(next) => match write_atomic(&path, next.as_bytes()) {
                    Ok(()) => step(
                        "first_run",
                        &path,
                        StepOutcome::Applied,
                        Some(format!("[{section}] {key} = {value}")),
                    ),
                    Err(e) => step("first_run", &path, StepOutcome::Failed, Some(e.to_string())),
                },
            }
        }
        FirstRun::Seed { seed, content } => {
            let path = root.join(seed);
            if path.exists() {
                return step("first_run", &path, StepOutcome::Present, None);
            }
            match write_atomic(&path, content.as_bytes()) {
                Ok(()) => step("first_run", &path, StepOutcome::Applied, None),
                Err(e) => step("first_run", &path, StepOutcome::Failed, Some(e.to_string())),
            }
        }
        FirstRun::Dir { dir } => {
            let path = root.join(dir);
            if path.is_dir() {
                return step("first_run", &path, StepOutcome::Present, None);
            }
            match std::fs::create_dir_all(&path) {
                Ok(()) => step("first_run", &path, StepOutcome::Applied, None),
                Err(e) => step("first_run", &path, StepOutcome::Failed, Some(e.to_string())),
            }
        }
    }
}

/// Copies `file` into `dir` unless a file of that name and size is there.
fn place(file: &Path, dir: &Path) -> PrepareStep {
    let Some(name) = file.file_name() else {
        return step(
            "firmware",
            file,
            StepOutcome::Failed,
            Some("not a file".into()),
        );
    };
    let dest = dir.join(name);
    let src_len = match std::fs::metadata(file) {
        Ok(m) if m.is_file() => m.len(),
        Ok(_) => {
            return step(
                "firmware",
                &dest,
                StepOutcome::Failed,
                Some("not a file".into()),
            );
        }
        Err(e) => return step("firmware", &dest, StepOutcome::Failed, Some(e.to_string())),
    };
    if std::fs::metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == src_len) {
        return step("firmware", &dest, StepOutcome::Present, None);
    }
    let copied = std::fs::create_dir_all(dir).and_then(|()| {
        let tmp = dest.with_extension("hermir-part");
        std::fs::copy(file, &tmp)?;
        std::fs::rename(&tmp, &dest)
    });
    match copied {
        Ok(()) => step("firmware", &dest, StepOutcome::Applied, None),
        Err(e) => step("firmware", &dest, StepOutcome::Failed, Some(e.to_string())),
    }
}

fn missing(dir: &Path, note: Option<&str>) -> PrepareStep {
    step(
        "firmware",
        dir,
        StepOutcome::Failed,
        Some(format!("missing: {}", note.unwrap_or("firmware"))),
    )
}

fn any_match(dir: &Path, patterns: &[String]) -> bool {
    std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten().any(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            patterns.iter().any(|p| glob(p, &n))
        })
    })
}

fn name_matches(file: &Path, patterns: &[String]) -> bool {
    file.file_name()
        .is_some_and(|n| patterns.iter().any(|p| glob(p, &n.to_string_lossy())))
}

/// Unpacks `archive` into `into` unless something matching `present` is there already.
fn unpack(archive: &Path, into: &Path, present: &str) -> PrepareStep {
    let present = [present.to_owned()];
    if any_match(into, &present) {
        return step("firmware_unpack", into, StepOutcome::Present, None);
    }
    let staging = into.with_extension("hermir-part");
    let _ = std::fs::remove_dir_all(&staging);
    let moved = crate::channel::extract::extract(archive, &staging)
        .map_err(|e| e.to_string())
        .and_then(|()| move_into(&staging, into).map_err(|e| e.to_string()));
    let _ = std::fs::remove_dir_all(&staging);
    match moved {
        Ok(()) if any_match(into, &present) => {
            step("firmware_unpack", into, StepOutcome::Applied, None)
        }
        Ok(()) => step(
            "firmware_unpack",
            into,
            StepOutcome::Failed,
            Some(format!("{} holds no {}", archive.display(), present[0])),
        ),
        Err(why) => step("firmware_unpack", into, StepOutcome::Failed, Some(why)),
    }
}

fn move_into(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        std::fs::rename(e.path(), to.join(e.file_name()))?;
    }
    Ok(())
}

/// Runs the emulator's installer on `file` once; `done` existing afterwards is the verdict.
fn run_installer(
    install: &Install,
    root: &Path,
    inst: &FirmwareInstall,
    file: Option<&PathBuf>,
    note: Option<&str>,
    runner: &dyn Runner,
) -> PrepareStep {
    let done = root.join(&inst.done);
    if done.is_file() {
        return step("firmware_install", &done, StepOutcome::Present, None);
    }
    let Some(file) = file else {
        return missing(&done, note);
    };
    let file_arg = file.to_string_lossy().into_owned();
    let rendered: Vec<String> = inst
        .args
        .iter()
        .map(|a| a.replace("{file}", &file_arg))
        .collect();
    let (program, mut args) = match &install.exe {
        // The sandbox sees only the folder the file sits in, read-only.
        Exe::FlatpakRun(id) => {
            let dir = file
                .parent()
                .unwrap_or(Path::new("/"))
                .to_string_lossy()
                .into_owned();
            (
                "flatpak".to_string(),
                vec![
                    "run".to_string(),
                    format!("--filesystem={dir}:ro"),
                    id.clone(),
                ],
            )
        }
        Exe::Path(p) => (p.to_string_lossy().into_owned(), Vec::new()),
    };
    args.extend(rendered);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    // ponytail: no timeout; RPCS3 finishes in seconds. Bound it if an installer ever hangs.
    let out = runner.run(&program, &argv);
    if done.is_file() {
        return step("firmware_install", &done, StepOutcome::Applied, None);
    }
    let last = |s: &str| {
        s.lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
    };
    let why = match out {
        Ok(o) => last(&o.stderr)
            .or_else(|| last(&o.stdout))
            .unwrap_or_else(|| "the installer wrote nothing".into()),
        Err(e) => e.to_string(),
    };
    step("firmware_install", &done, StepOutcome::Failed, Some(why))
}

/// `*` as any run of characters, case-insensitive: enough for firmware names.
pub fn glob(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.to_ascii_lowercase(), name.to_ascii_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == n;
    }
    let mut rest = n.as_str();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else {
                return false;
            };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(at) = rest.find(part) else {
                return false;
            };
            rest = &rest[at + part.len()..];
        }
    }
    true
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("hermir-part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::channel::flatpak::fake::FakeRunner;
    use crate::model::InstallKind;

    #[test]
    fn globs_match_firmware_names() {
        assert!(glob("*.bin", "SCPH39001.BIN"));
        assert!(glob("scph*.bin", "scph5501.bin"));
        assert!(!glob("scph*.bin", "rom1.bin"));
        assert!(glob("PS3UPDAT.PUP", "ps3updat.pup"));
        assert!(!glob("*.bin", "bios.bin.part"));
    }

    fn install_at(id: &str, root: &Path, exe: Exe) -> Install {
        Install {
            emulator: id.into(),
            kind: InstallKind::Flatpak,
            exe,
            version: None,
            config_root: Some(root.to_path_buf()),
        }
    }

    #[test]
    fn a_fresh_pcsx2_gets_its_wizard_answered_and_its_bios() {
        let c = Catalog::embedded().unwrap();
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let bios = src.path().join("scph39001.bin");
        std::fs::write(&bios, b"bios").unwrap();
        let inst = install_at(
            "pcsx2",
            root.path(),
            Exe::FlatpakRun("net.pcsx2.PCSX2".into()),
        );
        let e = c.get("pcsx2").unwrap();
        let first = prepare(
            e,
            Os::Linux,
            &inst,
            Some("ps2"),
            std::slice::from_ref(&bios),
            &FakeRunner::default(),
        );
        assert!(
            first
                .steps
                .iter()
                .all(|s| s.outcome == StepOutcome::Applied),
            "{first:?}"
        );
        // Seeded with the settings version first: without it PCSX2 offers to reset everything.
        let ini = std::fs::read_to_string(root.path().join("inis/PCSX2.ini")).unwrap();
        assert_eq!(
            ini,
            "[UI]\nSettingsVersion = 1\nSetupWizardIncomplete = false\n"
        );
        assert!(root.path().join("bios/scph39001.bin").is_file());
        let again = prepare(
            e,
            Os::Linux,
            &inst,
            Some("ps2"),
            &[bios],
            &FakeRunner::default(),
        );
        assert!(
            again
                .steps
                .iter()
                .all(|s| s.outcome == StepOutcome::Present),
            "{again:?}"
        );
    }

    #[test]
    fn a_platform_without_its_firmware_says_what_is_missing() {
        let c = Catalog::embedded().unwrap();
        let root = tempfile::tempdir().unwrap();
        let inst = install_at(
            "pcsx2",
            root.path(),
            Exe::FlatpakRun("net.pcsx2.PCSX2".into()),
        );
        let p = prepare(
            c.get("pcsx2").unwrap(),
            Os::Linux,
            &inst,
            Some("ps2"),
            &[],
            &FakeRunner::default(),
        );
        let miss = p.steps.iter().find(|s| s.kind == "firmware").unwrap();
        assert_eq!(miss.outcome, StepOutcome::Failed);
        assert!(miss.note.as_deref().unwrap().starts_with("missing:"));
    }

    #[test]
    fn melonds_saves_go_to_its_config_unless_the_user_chose() {
        let c = Catalog::embedded().unwrap();
        let root = tempfile::tempdir().unwrap();
        let inst = install_at(
            "melonds",
            root.path(),
            Exe::FlatpakRun("net.kuribo64.melonDS".into()),
        );
        let e = c.get("melonds").unwrap();
        prepare(
            e,
            Os::Linux,
            &inst,
            Some("nds"),
            &[],
            &FakeRunner::default(),
        );
        let toml = std::fs::read_to_string(root.path().join("melonDS.toml")).unwrap();
        let saves = format!("SaveFilePath = '{}/saves'", root.path().display());
        assert!(toml.contains(&saves), "{toml}");
        assert!(root.path().join("saves").is_dir() && root.path().join("states").is_dir());
        let mine = "[Instance0]\nSaveFilePath = \"/mine\"\nSavestatePath = \"\"\n";
        std::fs::write(root.path().join("melonDS.toml"), mine).unwrap();
        prepare(
            e,
            Os::Linux,
            &inst,
            Some("nds"),
            &[],
            &FakeRunner::default(),
        );
        let toml = std::fs::read_to_string(root.path().join("melonDS.toml")).unwrap();
        assert!(toml.contains("SaveFilePath = \"/mine\""), "{toml}");
        assert!(
            toml.contains("/states'"),
            "an empty one is still answered: {toml}"
        );
    }

    #[test]
    fn optional_firmware_missing_is_no_failure() {
        let c = Catalog::embedded().unwrap();
        let root = tempfile::tempdir().unwrap();
        let inst = install_at(
            "flycast",
            root.path(),
            Exe::FlatpakRun("org.flycast.Flycast".into()),
        );
        let p = prepare(
            c.get("flycast").unwrap(),
            Os::Linux,
            &inst,
            Some("dc"),
            &[],
            &FakeRunner::default(),
        );
        assert!(
            p.steps.iter().all(|s| s.outcome != StepOutcome::Failed),
            "{p:?}"
        );
    }

    #[test]
    fn eden_gets_its_keys_and_firmware_unpacked_once() {
        use std::io::Write;
        let c = Catalog::embedded().unwrap();
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let keys = src.path().join("prod.keys");
        std::fs::write(&keys, b"master_key_00 = 00").unwrap();
        let fw = src.path().join("Firmware 16.1.0.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&fw).unwrap());
        for n in ["a.nca", "b.nca"] {
            w.start_file(n, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(n.as_bytes()).unwrap();
        }
        w.finish().unwrap();
        let inst = install_at(
            "eden",
            root.path(),
            Exe::FlatpakRun("dev.eden_emu.eden".into()),
        );
        let e = c.get("eden").unwrap();
        let files = [keys, fw];
        let run = || {
            prepare(
                e,
                Os::Linux,
                &inst,
                Some("switch"),
                &files,
                &FakeRunner::default(),
            )
        };
        let first = run();
        assert!(
            first
                .steps
                .iter()
                .all(|s| s.outcome == StepOutcome::Applied),
            "{first:?}"
        );
        let reg = root.path().join("nand/system/Contents/registered");
        assert!(reg.join("a.nca").is_file() && reg.join("b.nca").is_file());
        assert!(root.path().join("keys/prod.keys").is_file());
        assert!(!root.path().join("keys/Firmware 16.1.0.zip").exists());
        // Firmware already there, the player's own included, is never unpacked over.
        std::fs::remove_file(reg.join("b.nca")).unwrap();
        assert!(
            run()
                .steps
                .iter()
                .all(|s| s.outcome == StepOutcome::Present),
            "{:?}",
            run()
        );
        assert!(!reg.join("b.nca").exists());
    }

    #[test]
    fn rpcs3_firmware_goes_through_its_installer_in_the_flatpak_sandbox() {
        let c = Catalog::embedded().unwrap();
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let pup = src.path().join("PS3UPDAT.PUP");
        std::fs::write(&pup, b"pup").unwrap();
        let inst = install_at(
            "rpcs3",
            root.path(),
            Exe::FlatpakRun("net.rpcs3.RPCS3".into()),
        );
        let runner = FakeRunner::default();
        let p = prepare(
            c.get("rpcs3").unwrap(),
            Os::Linux,
            &inst,
            Some("ps3"),
            std::slice::from_ref(&pup),
            &runner,
        );
        let ran = runner.calls.lock().unwrap();
        let call = ran
            .iter()
            .find(|c| c.iter().any(|a| a == "--installfw"))
            .expect("installer ran")
            .join(" ");
        assert!(call.starts_with("flatpak run --filesystem="), "{call}");
        assert!(
            call.contains(":ro net.rpcs3.RPCS3 --headless --installfw"),
            "{call}"
        );
        // The fake writes nothing, so the done file is the verdict: failed, with a reason.
        let s = p
            .steps
            .iter()
            .find(|s| s.kind == "firmware_install")
            .unwrap();
        assert_eq!(s.outcome, StepOutcome::Failed);
        std::fs::create_dir_all(root.path().join("dev_flash/vsh/etc")).unwrap();
        std::fs::write(root.path().join("dev_flash/vsh/etc/version.txt"), b"4.90").unwrap();
        let p = prepare(
            c.get("rpcs3").unwrap(),
            Os::Linux,
            &inst,
            Some("ps3"),
            &[pup],
            &runner,
        );
        let s = p
            .steps
            .iter()
            .find(|s| s.kind == "firmware_install")
            .unwrap();
        assert_eq!(s.outcome, StepOutcome::Present);
    }

    #[test]
    fn cemu_is_seeded_once_and_never_overwritten() {
        let c = Catalog::embedded().unwrap();
        let root = tempfile::tempdir().unwrap();
        let inst = install_at(
            "cemu",
            root.path(),
            Exe::FlatpakRun("info.cemu.Cemu".into()),
        );
        let e = c.get("cemu").unwrap();
        prepare(
            e,
            Os::Linux,
            &inst,
            Some("wiiu"),
            &[],
            &FakeRunner::default(),
        );
        let settings = root.path().join("settings.xml");
        assert!(settings.is_file());
        std::fs::write(&settings, b"<content><mine/></content>").unwrap();
        prepare(
            e,
            Os::Linux,
            &inst,
            Some("wiiu"),
            &[],
            &FakeRunner::default(),
        );
        assert_eq!(
            std::fs::read(&settings).unwrap(),
            b"<content><mine/></content>"
        );
    }
}
