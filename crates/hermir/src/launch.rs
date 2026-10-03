//! A copy started with a game: the catalog's launch template rendered into a [`LaunchSpec`]
//! the consumer runs its own way. hermir builds the command and never runs it (the CLI's `run`
//! does, for convenience). Every argument stays one argument; nothing is joined into a shell
//! string.
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::model::{Entry, Exe, Install, Knob, LaunchRequest, LaunchSpec, Os};

/// One argument of a template.
#[derive(Debug, PartialEq)]
pub(crate) enum Arg<'a> {
    Literal(&'a str),
    File,
    Fullscreen(&'a str),
    Platform,
    Core,
}

/// `arg` as the template grammar reads it, or why it is not one.
pub(crate) fn parse(arg: &str) -> Result<Arg<'_>, String> {
    let Some(inner) = arg.strip_prefix('{').and_then(|r| r.strip_suffix('}')) else {
        if arg.contains(['{', '}']) {
            return Err(format!(
                "{arg:?}: a placeholder is a whole argument of its own"
            ));
        }
        return Ok(Arg::Literal(arg));
    };
    match inner {
        "file" => Ok(Arg::File),
        "platform" => Ok(Arg::Platform),
        "core" => Ok(Arg::Core),
        _ => match inner.split_once(':') {
            // Everything after the first colon: Flycast's `window:fullscreen=yes` keeps its own.
            Some(("fullscreen", flag)) if !flag.is_empty() => Ok(Arg::Fullscreen(flag)),
            _ => Err(format!("{arg:?} is not a placeholder hermir knows")),
        },
    }
}

/// The command that starts `install` with `req`.
pub(crate) fn launch(
    entry: &Entry,
    os: Os,
    install: &Install,
    req: &LaunchRequest,
    profile: Option<&std::path::Path>,
) -> Result<LaunchSpec, String> {
    let fullscreen = req
        .fullscreen
        .or_else(|| req.patch.as_ref()?.video?.fullscreen)
        .unwrap_or(false);
    let file = match &req.file {
        Some(f) => Some(
            f.to_str()
                .ok_or_else(|| format!("{}: the path is not UTF-8", f.display()))?,
        ),
        None => None,
    };
    let mut args = Vec::new();
    // The profile's flag first: the emulator reads its settings from there.
    if let Some(dir) = profile {
        let p = entry
            .profile
            .as_ref()
            .ok_or_else(|| format!("{} has no profiles", entry.id))?;
        let dir = dir
            .to_str()
            .ok_or_else(|| format!("{}: the path is not UTF-8", dir.display()))?;
        args.extend(p.args.iter().map(|a| a.replace("{profile}", dir)));
    }
    for a in &entry.launch.args {
        match parse(a)? {
            Arg::Fullscreen(flag) => {
                if fullscreen {
                    args.push(flag.to_string());
                }
            }
            // Without a game only the fullscreen flags stay: the emulator opens on its own.
            _ if file.is_none() => {}
            Arg::Literal(l) => args.push(l.to_string()),
            Arg::File => args.push(file.unwrap_or_default().to_string()),
            Arg::Platform => args.push(
                req.platform
                    .clone()
                    .ok_or("the launch needs the game's platform")?,
            ),
            Arg::Core => args.push(core(entry, os, install, req)?),
        }
    }
    let mut sandbox = Vec::new();
    // A Flatpak reads only what it was granted: the game's folder, for this run.
    if let (Exe::FlatpakRun(_), Some(f)) = (&install.exe, &req.file)
        && let Some(dir) = f.parent().filter(|d| !d.as_os_str().is_empty())
    {
        sandbox.push(format!("--filesystem={}", dir.display()));
    }
    Ok(LaunchSpec {
        exe: install.exe.clone(),
        args,
        env: BTreeMap::new(),
        cwd: None,
        sandbox,
    })
}

/// RetroArch's core for the request: named, or the platform's default from the catalog, as
/// the path RetroArch loads it from (`<config root>/cores/<core>_libretro.<so|dll|dylib>`).
fn core(entry: &Entry, os: Os, install: &Install, req: &LaunchRequest) -> Result<String, String> {
    let name = match (&req.core, &req.platform) {
        (Some(c), _) => c.clone(),
        (None, Some(p)) => entry
            .launch
            .cores
            .get(p)
            .cloned()
            .ok_or_else(|| format!("{} has no default core for {p}; name one", entry.id))?,
        (None, None) => return Err("the launch needs a core, or the game's platform".into()),
    };
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!("{name:?} is not a core name"));
    }
    let root = install
        .config_root
        .as_deref()
        .ok_or("RetroArch's config root is unknown")?;
    let ext = match os {
        Os::Windows => "dll",
        Os::Macos => "dylib",
        Os::Linux => "so",
    };
    let path: PathBuf = root.join("cores").join(format!("{name}_libretro.{ext}"));
    if !path.is_file() {
        return Err(format!(
            "core {name} is not installed (`hermir core install {name}`)"
        ));
    }
    Ok(path.to_string_lossy().into_owned())
}

/// What the catalog's launch data must hold: arguments the grammar reads, `{core}` only with
/// cores to choose from, and a launch-only knob only where the template has its flag.
pub(crate) fn validate(entry: &Entry) -> Result<(), String> {
    let mut has_fullscreen = false;
    for a in &entry.launch.args {
        match parse(a)? {
            Arg::Fullscreen(_) => has_fullscreen = true,
            Arg::Core if entry.launch.cores.is_empty() => {
                return Err("{core} needs `cores` to choose from".into());
            }
            _ => {}
        }
    }
    if let Some(p) = &entry.profile {
        if !p.args.iter().any(|a| a.contains("{profile}")) {
            return Err("profile.args never name {profile}".into());
        }
        if let Some(a) = p
            .args
            .iter()
            .find(|a| a.replace("{profile}", "").contains(['{', '}']))
        {
            return Err(format!(
                "profile.args: {a:?} holds a placeholder other than {{profile}}"
            ));
        }
        let names = entry.config.as_ref().map(|c| &c.files);
        for (name, rel) in &p.files {
            if !names.is_some_and(|f| f.contains_key(name)) {
                return Err(format!(
                    "profile.files names {name}, which config.files does not"
                ));
            }
            if rel.is_empty()
                || !crate::channel::extract::is_safe_relative(std::path::Path::new(rel))
            {
                return Err(format!(
                    "profile.files: {rel} is not a path under the profile"
                ));
            }
        }
    }
    for (knob, k) in entry.config.iter().flat_map(|c| &c.knobs) {
        if let Knob::Launch { .. } = k {
            if knob != "video.fullscreen" {
                return Err(format!(
                    "{knob} cannot be set at launch; only video.fullscreen"
                ));
            }
            if !has_fullscreen {
                return Err(format!(
                    "{knob} is set at launch, and the template has no {{fullscreen:…}}"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::model::{InstallKind, Patch, Video};
    use std::path::Path;

    fn flatpak(id: &str, app: &str, root: &Path) -> Install {
        Install {
            emulator: id.into(),
            kind: InstallKind::Flatpak,
            exe: Exe::FlatpakRun(app.into()),
            version: None,
            config_root: Some(root.to_path_buf()),
        }
    }

    fn game(path: &str, fullscreen: Option<bool>) -> LaunchRequest {
        LaunchRequest {
            file: Some(PathBuf::from(path)),
            fullscreen,
            ..Default::default()
        }
    }

    #[test]
    fn the_grammar_reads_whole_placeholders_only() {
        assert_eq!(parse("-batch"), Ok(Arg::Literal("-batch")));
        assert_eq!(parse("{file}"), Ok(Arg::File));
        assert_eq!(
            parse("{fullscreen:window:fullscreen=yes}"),
            Ok(Arg::Fullscreen("window:fullscreen=yes"))
        );
        assert!(parse("--rom={file}").is_err());
        assert!(parse("{fullscreen}").is_err());
        assert!(parse("{nope}").is_err());
    }

    #[test]
    fn every_entry_renders_for_a_game_and_for_none() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        for e in c.entries() {
            validate(e).unwrap_or_else(|why| panic!("{}: {why}", e.id));
            let copy = flatpak(&e.id, "x.y.Z", tmp.path());
            let req = LaunchRequest {
                file: Some(PathBuf::from("/games/a game.iso")),
                platform: e.platforms.first().cloned(),
                fullscreen: Some(true),
                ..Default::default()
            };
            match launch(e, Os::Linux, &copy, &req, None) {
                Ok(spec) => assert!(
                    spec.args.iter().any(|a| a == "/games/a game.iso"),
                    "{}: {:?}",
                    e.id,
                    spec.args
                ),
                // RetroArch's core is not installed in a temp dir: said, not guessed.
                Err(why) => assert!(why.contains("is not installed"), "{}: {why}", e.id),
            }
            let alone = launch(e, Os::Linux, &copy, &LaunchRequest::default(), None).unwrap();
            assert!(alone.args.is_empty(), "{}: {:?}", e.id, alone.args);
        }
    }

    #[test]
    fn pcsx2_starts_the_game_in_batch_mode_and_fullscreen_when_asked() {
        let c = Catalog::embedded().unwrap();
        let e = c.get("pcsx2").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let copy = flatpak("pcsx2", "net.pcsx2.PCSX2", tmp.path());
        let spec = launch(
            e,
            Os::Linux,
            &copy,
            &game("/roms/ps2/Game.iso", Some(true)),
            None,
        )
        .unwrap();
        assert_eq!(
            spec.args,
            [
                "-batch",
                "-nogui",
                "-fullscreen",
                "--",
                "/roms/ps2/Game.iso"
            ]
        );
        assert_eq!(spec.sandbox, ["--filesystem=/roms/ps2"]);
        assert_eq!(
            spec.argv(),
            [
                "flatpak",
                "run",
                "--filesystem=/roms/ps2",
                "net.pcsx2.PCSX2",
                "-batch",
                "-nogui",
                "-fullscreen",
                "--",
                "/roms/ps2/Game.iso"
            ]
        );
        // Fullscreen from the session's patch when the request does not say.
        let req = LaunchRequest {
            patch: Some(Patch {
                video: Some(Video {
                    fullscreen: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..game("/roms/ps2/Game.iso", None)
        };
        let spec = launch(e, Os::Linux, &copy, &req, None).unwrap();
        assert!(!spec.args.iter().any(|a| a == "-fullscreen"));
    }

    #[test]
    fn retroarch_loads_the_platforms_core_or_the_one_named() {
        let c = Catalog::embedded().unwrap();
        let e = c.get("retroarch").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cores = tmp.path().join("cores");
        std::fs::create_dir_all(&cores).unwrap();
        std::fs::write(cores.join("snes9x_libretro.so"), b"").unwrap();
        std::fs::write(cores.join("bsnes_libretro.so"), b"").unwrap();
        let copy = flatpak("retroarch", "org.libretro.RetroArch", tmp.path());
        let req = LaunchRequest {
            platform: Some("snes".into()),
            ..game("/roms/snes/Game.sfc", None)
        };
        let spec = launch(e, Os::Linux, &copy, &req, None).unwrap();
        let core = cores.join("snes9x_libretro.so");
        assert_eq!(
            spec.args,
            ["-L", core.to_str().unwrap(), "/roms/snes/Game.sfc"]
        );
        let named = LaunchRequest {
            core: Some("bsnes".into()),
            ..req.clone()
        };
        let spec = launch(e, Os::Linux, &copy, &named, None).unwrap();
        assert!(spec.args[1].ends_with("bsnes_libretro.so"));
        let missing = LaunchRequest {
            core: Some("mesen".into()),
            ..req
        };
        let err = launch(e, Os::Linux, &copy, &missing, None).unwrap_err();
        assert!(err.contains("hermir core install mesen"), "{err}");
        let no_platform = game("/roms/snes/Game.sfc", None);
        assert!(launch(e, Os::Linux, &copy, &no_platform, None).is_err());
    }

    #[test]
    fn a_path_copy_gets_no_sandbox_grant() {
        let c = Catalog::embedded().unwrap();
        let e = c.get("dolphin").unwrap();
        let copy = Install {
            emulator: "dolphin".into(),
            kind: InstallKind::Managed,
            exe: Exe::Path(PathBuf::from("C:\\hermir\\dolphin\\app\\Dolphin.exe")),
            version: None,
            config_root: None,
        };
        let spec = launch(
            e,
            Os::Windows,
            &copy,
            &game("D:\\Games\\Metroid.rvz", None),
            None,
        )
        .unwrap();
        assert!(spec.sandbox.is_empty());
        assert_eq!(spec.argv()[0], "C:\\hermir\\dolphin\\app\\Dolphin.exe");
    }
}
