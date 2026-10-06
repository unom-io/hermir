//! Where a copy keeps the player's progress: the catalog's save folders resolved on this copy,
//! a folder the player moved in the emulator's settings followed. hermir reads; it never
//! copies or syncs saves (design §7).
use std::path::{Path, PathBuf};

use crate::config;
use crate::detect::{Env, expand};
use crate::model::{Entry, Install, Os, PlatformSaves, SaveDir, SaveLocation, Saves};

/// `install`'s saves for `platform`, or for every platform of the entry. `Err` names a
/// platform the emulator does not run.
pub fn saves(
    entry: &Entry,
    os: Os,
    install: &Install,
    env: &dyn Env,
    platform: Option<&str>,
) -> Result<Vec<Saves>, String> {
    let platforms: Vec<&str> = match platform {
        Some(p) if entry.platforms.iter().any(|q| q == p) => vec![p],
        Some(p) => {
            return Err(format!(
                "{} does not run {p}; it runs {}",
                entry.name,
                entry.platforms.join(", ")
            ));
        }
        None => entry.platforms.iter().map(String::as_str).collect(),
    };
    Ok(platforms
        .into_iter()
        .map(|p| {
            let mut out = Saves {
                platform: p.to_string(),
                locations: Vec::new(),
                unknown: None,
            };
            match entry.saves.get(p).or_else(|| entry.saves.get("*")) {
                Some(PlatformSaves::Dirs(dirs)) => {
                    out.locations = dirs
                        .iter()
                        .map(|d| locate(entry, os, install, env, d))
                        .collect();
                }
                Some(PlatformSaves::Unknown { unknown }) => out.unknown = Some(unknown.clone()),
                None => {
                    out.unknown = Some(format!(
                        "the catalog does not say where {} keeps {p} saves",
                        entry.name
                    ));
                }
            }
            out
        })
        .collect())
}

pub(crate) fn locate(
    entry: &Entry,
    os: Os,
    install: &Install,
    env: &dyn Env,
    d: &SaveDir,
) -> SaveLocation {
    let root = install.config_root.as_deref();
    let mut loc = SaveLocation {
        kind: d.kind,
        path: None,
        beside_game: false,
        pattern: d.pattern.clone(),
        per_game: d.per_game,
        from_setting: false,
        exists: false,
        note: d.note.clone(),
    };
    let moved = d.setting.as_ref().zip(root).and_then(|(s, root)| {
        let value = config::get_native(entry, os, install, &s.file, &s.section, &s.key).ok()??;
        let value = unquote(value.trim());
        if value.is_empty() || s.unset.iter().any(|u| u == value) {
            return None;
        }
        let at = folder(value, root, env);
        Some(match &s.then {
            Some(under) => at.join(under),
            None => at,
        })
    });
    if let Some(at) = moved {
        loc.path = Some(at);
        loc.from_setting = true;
    } else {
        match d.path.as_ref().and_then(|p| p.get(os)) {
            Some("{game}") => loc.beside_game = true,
            Some(rel) => loc.path = root.map(|r| resolve(r, rel)),
            None => {}
        }
    }
    loc.exists = loc.path.as_deref().is_some_and(|p| env.exists(p));
    loc
}

/// `rel` under the config root; `{data}/…` under the data directory beside it.
fn resolve(root: &Path, rel: &str) -> PathBuf {
    match rel
        .strip_prefix("{data}/")
        .or_else(|| rel.strip_prefix("{data}\\"))
    {
        Some(rest) => config::data_dir(root).join(rest),
        None if rel == "{data}" => config::data_dir(root),
        None => root.join(rel),
    }
}

/// The folder a setting names: `~` the home, a leading `:` the config root (RetroArch's
/// "next to the config"), a relative path under the config root (PCSX2's `memcards`).
fn folder(value: &str, root: &Path, env: &dyn Env) -> PathBuf {
    if let Some(rest) = value.strip_prefix(':') {
        return root.join(rest.trim_start_matches(['/', '\\']));
    }
    let path = if value.starts_with('~') {
        expand(value, env, None)
    } else {
        PathBuf::from(value)
    };
    let drive = value.as_bytes().get(1) == Some(&b':');
    if path.is_absolute() || drive || value.starts_with(['/', '\\']) {
        path
    } else {
        root.join(path)
    }
}

fn unquote(v: &str) -> &str {
    ['"', '\'']
        .iter()
        .find_map(|q| v.strip_prefix(*q).and_then(|r| r.strip_suffix(*q)))
        .unwrap_or(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::detect::fake::FakeEnv;
    use crate::model::{Exe, InstallKind, SaveKind};

    fn flatpak(id: &str, root: &Path) -> Install {
        Install::new(
            id,
            InstallKind::Flatpak,
            Exe::FlatpakRun(id.into()),
            Some(root.to_path_buf()),
        )
    }

    #[test]
    fn folders_resolve_under_the_root_beside_it_and_where_the_settings_moved_them() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let env = FakeEnv::new(Os::Linux, "/home/u");
        // PCSX2: relative to its root, moved by its own setting.
        let root = tmp.path().join("config/PCSX2");
        std::fs::create_dir_all(root.join("inis")).unwrap();
        let pcsx2 = c.get("pcsx2").unwrap();
        let s = saves(pcsx2, Os::Linux, &flatpak("pcsx2", &root), &env, None).unwrap();
        assert_eq!(s.len(), 1);
        let cards = &s[0].locations[0];
        assert_eq!(cards.kind, SaveKind::Memcard);
        assert_eq!(cards.path.as_deref(), Some(root.join("memcards").as_path()));
        assert!(!cards.from_setting);
        std::fs::write(
            root.join("inis/PCSX2.ini"),
            "[Folders]\nMemoryCards = /games/cards\nSavestates = states\n",
        )
        .unwrap();
        let s = saves(
            pcsx2,
            Os::Linux,
            &flatpak("pcsx2", &root),
            &env,
            Some("ps2"),
        )
        .unwrap();
        assert_eq!(
            s[0].locations[0].path.as_deref(),
            Some(Path::new("/games/cards"))
        );
        assert!(s[0].locations[0].from_setting);
        assert_eq!(
            s[0].locations[1].path.as_deref(),
            Some(root.join("states").as_path())
        );

        // Dolphin: `{data}` is the Flatpak's data/ beside its config/.
        let root = tmp.path().join("app/config/dolphin-emu");
        let dolphin = c.get("dolphin").unwrap();
        let s = saves(
            dolphin,
            Os::Linux,
            &flatpak("dolphin", &root),
            &env,
            Some("ngc"),
        )
        .unwrap();
        assert_eq!(
            s[0].locations[0].path.as_deref(),
            Some(tmp.path().join("app/data/dolphin-emu/GC").as_path())
        );

        // RetroArch: `default` is no folder; `~` is the home.
        let root = tmp.path().join("retroarch");
        std::fs::create_dir_all(&root).unwrap();
        let ra = c.get("retroarch").unwrap();
        std::fs::write(
            root.join("retroarch.cfg"),
            "savefile_directory = \"default\"\nsavestate_directory = \"~/states\"\n",
        )
        .unwrap();
        let s = saves(
            ra,
            Os::Linux,
            &flatpak("retroarch", &root),
            &env,
            Some("snes"),
        )
        .unwrap();
        assert_eq!(
            s[0].locations[0].path.as_deref(),
            Some(root.join("saves").as_path())
        );
        assert_eq!(
            s[0].locations[1].path.as_deref(),
            Some(Path::new("/home/u/states"))
        );

        // mGBA: beside each game.
        let mgba = c.get("mgba").unwrap();
        let s = saves(mgba, Os::Linux, &flatpak("mgba", &root), &env, Some("gba")).unwrap();
        assert!(s[0].locations[0].beside_game);
        assert_eq!(s[0].locations[0].path, None);

        assert!(saves(mgba, Os::Linux, &flatpak("mgba", &root), &env, Some("ps2")).is_err());
    }

    #[test]
    fn settings_in_every_format_move_the_folder() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        // The real file system: whether a folder is there is part of the answer.
        let env = crate::detect::RealEnv::with_home(Os::Linux, tmp.path().join("home"));
        let at = |id: &str, root: &Path, platform: &str| {
            let s = saves(
                c.get(id).unwrap(),
                Os::Linux,
                &flatpak(id, root),
                &env,
                Some(platform),
            );
            s.unwrap().remove(0).locations
        };
        // Cemu: XML, and the saves under the folder the key names.
        let root = tmp.path().join("config/Cemu");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(
            at("cemu", &root, "wiiu")[0].path.as_deref(),
            Some(
                tmp.path()
                    .join("data/Cemu/mlc01/usr/save/00050000")
                    .as_path()
            )
        );
        std::fs::write(
            root.join("settings.xml"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<content>\n\t<mlc_path>/games/mlc</mlc_path>\n</content>\n",
        )
        .unwrap();
        let l = at("cemu", &root, "wiiu");
        assert_eq!(
            l[0].path.as_deref(),
            Some(Path::new("/games/mlc/usr/save/00050000"))
        );
        assert!(l[0].from_setting);
        // DuckStation: a relative folder is under its root; the folder exists.
        let root = tmp.path().join("duckstation");
        std::fs::create_dir_all(root.join("cards")).unwrap();
        std::fs::write(
            root.join("settings.ini"),
            "[MemoryCards]\nDirectory = cards\n",
        )
        .unwrap();
        let l = at("duckstation", &root, "psx");
        assert_eq!(l[0].path.as_deref(), Some(root.join("cards").as_path()));
        assert!(l[0].exists);
        assert!(!l[1].exists, "savestates/ appears at the first state");
        // xemu: nothing until a disk image is set.
        let root = tmp.path().join("xemu");
        std::fs::create_dir_all(&root).unwrap();
        let l = at("xemu", &root, "xbox");
        assert_eq!((l[0].path.as_deref(), l[0].beside_game), (None, false));
        std::fs::write(
            root.join("xemu.toml"),
            "[sys.files]\nhdd_path = '/games/xbox_hdd.qcow2'\n",
        )
        .unwrap();
        let l = at("xemu", &root, "xbox");
        assert_eq!(
            l[0].path.as_deref(),
            Some(Path::new("/games/xbox_hdd.qcow2"))
        );
    }

    #[test]
    fn the_data_directory_is_beside_the_config_root_where_there_is_one() {
        assert_eq!(
            config::data_dir(Path::new("/h/.var/app/x/config/Cemu")),
            Path::new("/h/.var/app/x/data/Cemu")
        );
        assert_eq!(
            config::data_dir(Path::new("/h/.config/Cemu")),
            Path::new("/h/.local/share/Cemu")
        );
        assert_eq!(
            config::data_dir(Path::new("C:\\Users\\u\\AppData\\Roaming\\Cemu")),
            Path::new("C:\\Users\\u\\AppData\\Roaming\\Cemu")
        );
        assert_eq!(
            config::data_dir(Path::new("/opt/cemu/portable")),
            Path::new("/opt/cemu/portable")
        );
        assert_eq!(resolve(Path::new("/r"), "{game}"), Path::new("/r/{game}"));
        assert_eq!(
            folder(":/saves", Path::new("/r"), &FakeEnv::new(Os::Windows, "/h")),
            Path::new("/r/saves")
        );
        assert_eq!(
            folder(
                "D:\\Saves",
                Path::new("/r"),
                &FakeEnv::new(Os::Windows, "/h")
            ),
            Path::new("D:\\Saves")
        );
    }
}
