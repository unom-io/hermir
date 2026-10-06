use super::*;
use crate::catalog::Catalog;
use crate::detect::fake::FakeEnv;
use crate::model::{Exe, InstallKind};

fn copy(id: &str, root: &Path) -> Install {
    Install::new(
        id,
        InstallKind::Native,
        Exe::Path("/usr/bin/x".into()),
        Some(root.to_path_buf()),
    )
}

fn write(p: &Path, bytes: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn names(units: &[SaveUnit]) -> Vec<(String, bool)> {
    units.iter().map(|u| (u.name.clone(), u.shared)).collect()
}

#[test]
fn globs_cover_what_the_catalog_says() {
    assert!(glob("Mcd00?.ps2", "Mcd001.ps2"));
    assert!(!glob("Mcd00?.ps2", "Mcd0012.ps2"));
    assert!(glob("*/Card ?.raw", "USA/Card A.raw"));
    assert!(!glob("*/Card ?.raw", "USA/x/Card A.raw"));
    assert!(glob("00000001/**", "00000001/a/b"));
    assert!(!glob("00000001/**", "00000002/a"));
    assert!(glob("Headers/00000001/**", "Headers/00000001/x.header"));
    assert!(glob("**/x.bin", "x.bin"));
    assert!(glob("**/x.bin", "a/b/x.bin"));
    assert!(glob("Card [AB].raw", "Card B.raw"));
    assert!(!glob("Card [AB].raw", "Card C.raw"));
}

/// The names are the ones ROM Manager stored on RomM servers before hermir did this: a server's
/// saves keep matching.
#[test]
fn units_are_named_as_the_server_already_has_them() {
    let c = Catalog::embedded().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let env = FakeEnv::new(Os::Linux, "/home/u");

    // PCSX2: a card per file; the two shared cards are marked.
    let root = tmp.path().join("pcsx2");
    write(&root.join("memcards/Mcd001.ps2"), b"a");
    write(&root.join("memcards/Gran Turismo 4.ps2"), b"b");
    write(&root.join("memcards/x.log"), b"noise");
    let u = list(
        c.get("pcsx2").unwrap(),
        Os::Linux,
        &copy("pcsx2", &root),
        &env,
        "ps2",
        None,
    )
    .unwrap();
    assert_eq!(
        names(&u),
        [
            ("Gran Turismo 4.ps2".into(), false),
            ("Mcd001.ps2".into(), true)
        ]
    );

    // Dolphin on Wii: a disc game's title folder, and a channel under its own lead.
    let root = tmp.path().join("app/config/dolphin-emu");
    let data = tmp.path().join("app/data/dolphin-emu");
    write(
        &data.join("Wii/title/00010000/52534245/data/save.bin"),
        b"s",
    );
    write(&data.join("Wii/title/00010004/524d4350/data/x.bin"), b"m");
    write(&data.join("StateSaves/RSBE01.s01"), b"st");
    let u = list(
        c.get("dolphin").unwrap(),
        Os::Linux,
        &copy("dolphin", &root),
        &env,
        "wii",
        None,
    )
    .unwrap();
    let mut n: Vec<String> = u.iter().map(|x| x.name.clone()).collect();
    n.sort();
    assert_eq!(n, ["00010004__524d4350.tar", "52534245.tar", "RSBE01.s01"]);

    // Dolphin on GameCube: GCI files by their path, raw cards shared.
    write(&data.join("GC/USA/Card A/01-GALE-x.gci"), b"g");
    write(&data.join("GC/USA/Card A.raw"), b"r");
    let u = list(
        c.get("dolphin").unwrap(),
        Os::Linux,
        &copy("dolphin", &root),
        &env,
        "ngc",
        None,
    )
    .unwrap();
    let cards: Vec<(String, bool)> = names(&u)
        .into_iter()
        .filter(|(n, _)| !n.ends_with(".s01"))
        .collect();
    assert_eq!(
        cards,
        [
            ("USA__Card A.raw".into(), true),
            ("USA__Card A__01-GALE-x.gci".into(), false)
        ]
    );

    // Xenia: one unit per profile and title, its DLC beside it left out.
    let root = tmp.path().join("xenia");
    write(
        &root.join("content/B13EBABEBABEBABE/4D5309C9/00000001/save"),
        b"x",
    );
    write(
        &root.join("content/B13EBABEBABEBABE/4D5309C9/00000002/dlc"),
        b"dlc",
    );
    write(
        &root.join("content/B13EBABEBABEBABE/FFFE07D1/00010000/profile"),
        b"p",
    );
    let u = list(
        c.get("xenia-canary").unwrap(),
        Os::Linux,
        &copy("xenia-canary", &root),
        &env,
        "xbox360",
        None,
    )
    .unwrap();
    assert_eq!(
        names(&u),
        [
            ("B13EBABEBABEBABE__4D5309C9.tar".into(), false),
            ("B13EBABEBABEBABE__FFFE07D1.tar".into(), false)
        ]
    );
    assert_eq!(u[0].size, 1, "the DLC is not part of the save");

    // mGBA: beside the game, only what is a save.
    let games = tmp.path().join("gba");
    write(&games.join("Emerald.gba"), b"rom");
    write(&games.join("Emerald.sav"), b"sav");
    let u = list(
        c.get("mgba").unwrap(),
        Os::Linux,
        &copy("mgba", &tmp.path().join("mgba")),
        &env,
        "gba",
        Some(&games),
    )
    .unwrap();
    assert_eq!(names(&u), [("Emerald.sav".into(), false)]);
}

#[test]
fn a_folder_unit_round_trips_and_its_tar_is_the_same_on_every_machine() {
    let c = Catalog::embedded().unwrap();
    let e = c.get("ppsspp").unwrap();
    let env = FakeEnv::new(Os::Linux, "/home/u");
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    write(&a.join("PSP/SAVEDATA/UCUS98632/DATA.BIN"), b"progress");
    write(&a.join("PSP/SAVEDATA/UCUS98632/ICON0.PNG"), b"icon");
    let out = tmp.path().join("out");
    let one = export(
        e,
        Os::Linux,
        &copy("ppsspp", &a),
        &env,
        "psp",
        None,
        SaveKind::Save,
        "UCUS98632.tar",
        &out,
    )
    .unwrap();
    // The same files, touched later: the same bytes.
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(a.join("PSP/SAVEDATA/UCUS98632/DATA.BIN"), b"progress").unwrap();
    let two = export(
        e,
        Os::Linux,
        &copy("ppsspp", &a),
        &env,
        "psp",
        None,
        SaveKind::Save,
        "UCUS98632.tar",
        &tmp.path().join("out2"),
    )
    .unwrap();
    assert_eq!(one.md5, two.md5);

    // Into another copy, where an older save sits: it is backed up, then replaced.
    let b = tmp.path().join("b");
    write(&b.join("PSP/SAVEDATA/UCUS98632/DATA.BIN"), b"old");
    let backups = tmp.path().join("backups");
    let step = import(
        e,
        Os::Linux,
        &copy("ppsspp", &b),
        &env,
        "psp",
        None,
        SaveKind::Save,
        "UCUS98632.tar",
        &one.file,
        &[],
        &backups,
    )
    .unwrap();
    assert_eq!(step.outcome, StepOutcome::Applied);
    assert_eq!(
        fs::read(b.join("PSP/SAVEDATA/UCUS98632/DATA.BIN")).unwrap(),
        b"progress"
    );
    assert_eq!(
        fs::read(b.join("PSP/SAVEDATA/UCUS98632/ICON0.PNG")).unwrap(),
        b"icon"
    );
    let kept: Vec<_> = walk(&backups).into_iter().map(|f| f.rel).collect();
    assert_eq!(kept.len(), 1);
    assert!(kept[0].ends_with("UCUS98632.tar/DATA.BIN"), "{kept:?}");
}

#[test]
fn a_restore_leaves_what_sits_beside_the_save() {
    let c = Catalog::embedded().unwrap();
    let e = c.get("xenia-canary").unwrap();
    let env = FakeEnv::new(Os::Linux, "/home/u");
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    write(&a.join("content/P/T/00000001/save"), b"new");
    let out = tmp.path().join("out");
    let x = export(
        e,
        Os::Linux,
        &copy("xenia-canary", &a),
        &env,
        "xbox360",
        None,
        SaveKind::Save,
        "P__T.tar",
        &out,
    )
    .unwrap();
    let b = tmp.path().join("b");
    write(&b.join("content/P/T/00000001/save"), b"old");
    write(&b.join("content/P/T/00000002/dlc"), b"dlc");
    import(
        e,
        Os::Linux,
        &copy("xenia-canary", &b),
        &env,
        "xbox360",
        None,
        SaveKind::Save,
        "P__T.tar",
        &x.file,
        &[],
        &tmp.path().join("bk"),
    )
    .unwrap();
    assert_eq!(
        fs::read(b.join("content/P/T/00000001/save")).unwrap(),
        b"new"
    );
    assert_eq!(
        fs::read(b.join("content/P/T/00000002/dlc")).unwrap(),
        b"dlc"
    );
}

#[test]
fn a_tar_from_the_old_plugin_imports_and_a_hostile_one_writes_nothing() {
    let c = Catalog::embedded().unwrap();
    let e = c.get("rpcs3").unwrap();
    let env = FakeEnv::new(Os::Linux, "/home/u");
    let tmp = tempfile::tempdir().unwrap();
    // Times and a mode in the headers, as the plugin's own writer made them.
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_ustar();
    h.set_size(4);
    h.set_mode(0o644);
    h.set_mtime(1_727_000_000);
    b.append_data(&mut h, "PARAM.SFO", &b"sfo!"[..]).unwrap();
    let old = tmp.path().join("old.tar");
    fs::write(&old, b.into_inner().unwrap()).unwrap();
    let root = tmp.path().join("rpcs3");
    let step = import(
        e,
        Os::Linux,
        &copy("rpcs3", &root),
        &env,
        "ps3",
        None,
        SaveKind::Save,
        "BLES00932-SAVE.tar",
        &old,
        &[],
        &tmp.path().join("bk"),
    )
    .unwrap();
    assert_eq!(step.outcome, StepOutcome::Applied);
    assert_eq!(
        fs::read(root.join("dev_hdd0/home/00000001/savedata/BLES00932-SAVE/PARAM.SFO")).unwrap(),
        b"sfo!"
    );

    // An entry that climbs out.
    let mut raw = vec![0u8; 512];
    raw[..17].copy_from_slice(b"../../etc/evilxxx");
    raw[100..108].copy_from_slice(b"0000644\0");
    raw[124..136].copy_from_slice(b"00000000004\0");
    raw[156] = b'0';
    raw[257..263].copy_from_slice(b"ustar\0");
    raw[263..265].copy_from_slice(b"00");
    let sum: u32 = raw.iter().map(|b| u32::from(*b)).sum::<u32>() + 8 * 32;
    raw[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    raw.extend(b"evil");
    raw.resize(512 * 4, 0);
    let bad = tmp.path().join("bad.tar");
    fs::write(&bad, raw).unwrap();
    let err = import(
        e,
        Os::Linux,
        &copy("rpcs3", &root),
        &env,
        "ps3",
        None,
        SaveKind::Save,
        "BLES00932-SAVE.tar",
        &bad,
        &[],
        &tmp.path().join("bk"),
    );
    assert!(err.is_err());
    assert!(!tmp.path().join("etc").exists());
    // Names never carry a path.
    assert!(
        export(
            e,
            Os::Linux,
            &copy("rpcs3", &root),
            &env,
            "ps3",
            None,
            SaveKind::Save,
            "../x.tar",
            &tmp.path().join("o")
        )
        .is_err()
    );
}

#[test]
fn a_switch_save_lands_on_the_hosts_profile_and_its_working_copy() {
    let c = Catalog::embedded().unwrap();
    let e = c.get("ryujinx").unwrap();
    let env = FakeEnv::new(Os::Linux, "/home/u");
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Ryujinx");
    let mut user = [0u8; 16];
    user[0] = 1;
    let idx = switch_index(&[(0x01006B601380E000, user, 1, 0x10)]);
    write(
        &root.join("bis/system/save/8000000000000000/0/imkvdb.arc"),
        &idx,
    );
    write(
        &root.join("system/Profiles.json"),
        b"{\"profiles\":[{\"user_id\":\"00000000000000010000000000000000\"}]}",
    );
    write(&root.join("bis/user/save/0000000000000010/0/data"), b"old");
    write(&root.join("bis/user/save/0000000000000010/1/data"), b"old");
    let u = list(e, Os::Linux, &copy("ryujinx", &root), &env, "switch", None).unwrap();
    let name = "01006B601380E000.account.00000000000000010000000000000000.tar";
    assert_eq!(names(&u), [(name.into(), false)]);
    assert!(u[0].written_at_start);

    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_ustar();
    h.set_size(3);
    b.append_data(&mut h, "data", &b"new"[..]).unwrap();
    let from = tmp.path().join("in.tar");
    fs::write(&from, b.into_inner().unwrap()).unwrap();
    let s = import(
        e,
        Os::Linux,
        &copy("ryujinx", &root),
        &env,
        "switch",
        None,
        SaveKind::Save,
        name,
        &from,
        &[],
        &tmp.path().join("bk"),
    )
    .unwrap();
    assert_eq!(s.outcome, StepOutcome::Applied);
    assert_eq!(
        fs::read(root.join("bis/user/save/0000000000000010/0/data")).unwrap(),
        b"new"
    );
    assert_eq!(
        fs::read(root.join("bis/user/save/0000000000000010/1/data")).unwrap(),
        b"new"
    );

    // A title that never ran here has no save folder: nothing is written, and the step says so.
    let s = import(
        e,
        Os::Linux,
        &copy("ryujinx", &root),
        &env,
        "switch",
        None,
        SaveKind::Save,
        "0100AAAAAAAAAAAA.device.tar",
        &from,
        &[],
        &tmp.path().join("bk"),
    )
    .unwrap();
    assert_eq!(s.outcome, StepOutcome::Skipped);
}

fn switch_index(rows: &[(u64, [u8; 16], u8, u64)]) -> Vec<u8> {
    let mut b = b"IMKV".to_vec();
    b.extend(0u32.to_le_bytes());
    b.extend((rows.len() as u32).to_le_bytes());
    for (title, user, kind, save) in rows {
        b.extend(b"IMEN");
        b.extend(64u32.to_le_bytes());
        b.extend(64u32.to_le_bytes());
        let mut key = vec![0u8; 64];
        key[..8].copy_from_slice(&title.to_le_bytes());
        key[8..24].copy_from_slice(user);
        key[0x20] = *kind;
        b.extend(key);
        let mut value = vec![0u8; 64];
        value[..8].copy_from_slice(&save.to_le_bytes());
        b.extend(value);
    }
    b
}

#[test]
fn the_registry_names_what_each_emulator_saves_and_installs() {
    let c = Catalog::embedded().unwrap();
    let r = c.registry(Os::Linux);
    let ryu = r.emulators.iter().find(|e| e.id == "ryujinx").unwrap();
    assert_eq!(ryu.saves, ["switch"]);
    assert_eq!(ryu.content["switch"], ["update", "dlc"]);
    assert!(!ryu.offered);
    let eden = r.emulators.iter().find(|e| e.id == "eden").unwrap();
    assert!(
        eden.content.is_empty(),
        "Eden has no way, and says so in the catalog"
    );
    let ra = r.emulators.iter().find(|e| e.id == "retroarch").unwrap();
    assert!(ra.archives && ra.saves.contains(&"snes".to_string()));
    assert!(ra.firmware.contains_key("psx"));
    // Every name a source uses finds its platform, old ROM Manager ids included.
    for (name, id) in [
        ("gamecube", "ngc"),
        ("ps1", "psx"),
        ("dreamcast", "dc"),
        ("pcengine", "tg16"),
        ("vita", "psvita"),
        ("genesis-slash-megadrive", "genesis"),
        ("Nintendo - GameCube", "ngc"),
        ("gc", "ngc"),
        ("PS2", "ps2"),
    ] {
        assert_eq!(
            c.find_platform(name).map(|p| p.id.as_str()),
            Some(id),
            "{name}"
        );
    }
}
