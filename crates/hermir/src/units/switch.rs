//! Switch saves: one title's data for one profile, or for the whole console. Eden files a save
//! under `<profile>/<title>`, Ryujinx under a save id its own index maps to a title and a
//! profile. Both name a unit the same, `<TITLE>.account.<user>` or `<TITLE>.device`, so either
//! emulator restores the other's.
use std::path::{Path, PathBuf};

/// Names the units of a folder that is not named for its saves.
pub(super) trait Naming {
    /// The unit's name (without `.tar`) for the folder `rel` under the save folder, or `None`
    /// when that folder is no save.
    fn name_of(&self, rel: &str) -> Option<String>;
    /// Where a unit named `name` goes under the save folder, or `None` when this copy has no
    /// place for it yet. `others` are the server's names for the same game.
    fn rel_of(&self, name: &str, others: &[String]) -> Option<String>;
    /// A working copy of the unit beside it, rewritten with a restore.
    fn mirror(&self, _rel: &str) -> Option<String> {
        None
    }
}

pub(super) fn naming(name: &str, root: &Path) -> Option<Box<dyn Naming>> {
    match name {
        "eden" => Some(Box::new(Eden {
            root: root.to_path_buf(),
        })),
        "ryujinx" => Some(Box::new(Ryujinx {
            root: root.to_path_buf(),
        })),
        _ => None,
    }
}

/// The adapters a catalog entry may name.
pub(crate) const ADAPTERS: &[&str] = &["eden", "ryujinx"];

/// The profile a fresh Ryujinx starts with; a host's only profile takes over its saves.
const DEFAULT_USER: &str = "00000000000000010000000000000000";
const ZERO_USER: &str = "00000000000000000000000000000000";

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A user id as Ryujinx spells it: the 16 raw bytes as two little-endian u64, in order.
fn user_of_bytes(id: &[u8]) -> String {
    let half = |b: &[u8]| {
        format!(
            "{:016x}",
            u64::from_le_bytes(b.try_into().unwrap_or([0; 8]))
        )
    };
    format!("{}{}", half(&id[..8]), half(&id[8..16]))
}

/// Eden names a profile's folder with the halves the other way round, in capitals.
fn eden_dir_of(user: &str) -> String {
    format!("{}{}", &user[16..], &user[..16]).to_ascii_uppercase()
}

fn user_of_eden_dir(dir: &str) -> Option<String> {
    is_hex(dir, 32).then(|| format!("{}{}", &dir[16..], &dir[..16]).to_ascii_lowercase())
}

fn name_of_save(title: &str, user: Option<&str>) -> String {
    match user {
        None => format!("{}.device", title.to_ascii_uppercase()),
        Some(u) => format!(
            "{}.account.{}",
            title.to_ascii_uppercase(),
            u.to_ascii_lowercase()
        ),
    }
}

/// `(title, user)` from a unit name, the user absent for a device save.
fn parse_name(name: &str) -> Option<(String, Option<String>)> {
    let (title, rest) = name.split_once('.')?;
    if !is_hex(title, 16) {
        return None;
    }
    let title = title.to_ascii_uppercase();
    if rest.eq_ignore_ascii_case("device") {
        return Some((title, None));
    }
    let user = rest.strip_prefix("account.")?;
    is_hex(user, 32).then(|| (title, Some(user.to_ascii_lowercase())))
}

/// Which of this host's profiles takes a server save made for `wanted`: the one it names;
/// failing that, a host with one profile takes the one save no local profile matches, or the
/// default user's when several are left, so a player's first profile follows them.
fn local_user_for(wanted: &str, siblings: &[String], locals: &[String]) -> Option<String> {
    if locals.iter().any(|l| l == wanted) {
        return Some(wanted.to_string());
    }
    let [only] = locals else { return None };
    if siblings.iter().any(|u| locals.contains(u)) {
        return None;
    }
    let unmatched: Vec<&String> = siblings.iter().filter(|u| !locals.contains(u)).collect();
    let chosen = match unmatched.as_slice() {
        [one] => Some(one.as_str()),
        _ => unmatched
            .iter()
            .find(|u| u.as_str() == DEFAULT_USER)
            .map(|u| u.as_str()),
    };
    (chosen == Some(wanted)).then(|| only.clone())
}

/// The account users of `title`'s saves among `names` (with or without `.tar`).
fn users_of(names: &[String], title: &str) -> Vec<String> {
    names
        .iter()
        .filter_map(|n| parse_name(n.strip_suffix(".tar").unwrap_or(n)))
        .filter(|(t, _)| t == title)
        .filter_map(|(_, u)| u)
        .collect()
}

fn siblings(name: &str, others: &[String], title: &str) -> Vec<String> {
    let mut all: Vec<String> = others.to_vec();
    all.push(name.to_string());
    let mut users = users_of(&all, title);
    users.sort();
    users.dedup();
    users
}

struct Eden {
    root: PathBuf,
}

impl Eden {
    /// The profile ids in `profiles.dat`: a 16-byte header, then eight 200-byte user slots.
    fn profiles(&self) -> Vec<String> {
        let path = self
            .root
            .join("nand/system/save/8000000000000010/su/avators/profiles.dat");
        let Ok(b) = std::fs::read(path) else {
            return Vec::new();
        };
        eden_profiles(&b)
    }
}

fn eden_profiles(b: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0x10;
    while at + 16 <= b.len() && out.len() < 8 {
        let id = &b[at..at + 16];
        if id.iter().any(|x| *x != 0) {
            out.push(user_of_bytes(id));
        }
        at += 0xc8;
    }
    out
}

impl Naming for Eden {
    fn name_of(&self, rel: &str) -> Option<String> {
        let (dir, title) = rel.split_once('/')?;
        if title.contains('/') || !is_hex(title, 16) {
            return None;
        }
        if dir == ZERO_USER {
            return Some(name_of_save(title, None));
        }
        Some(name_of_save(title, Some(&user_of_eden_dir(dir)?)))
    }

    fn rel_of(&self, name: &str, others: &[String]) -> Option<String> {
        let (title, user) = parse_name(name)?;
        let Some(wanted) = user else {
            return Some(format!("{ZERO_USER}/{title}"));
        };
        let user = local_user_for(&wanted, &siblings(name, others, &title), &self.profiles())?;
        Some(format!("{}/{title}", eden_dir_of(&user)))
    }
}

struct Ryujinx {
    root: PathBuf,
}

struct RyuSave {
    title: String,
    user: String,
    device: bool,
    save_id: String,
}

impl Ryujinx {
    fn index(&self) -> Vec<RyuSave> {
        let path = self
            .root
            .join("bis/system/save/8000000000000000/0/imkvdb.arc");
        std::fs::read(path)
            .map(|b| ryujinx_index(&b))
            .unwrap_or_default()
    }

    fn profiles(&self) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(self.root.join("system/Profiles.json")) else {
            return Vec::new();
        };
        let v: serde_json::Value =
            serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default();
        v["profiles"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["user_id"].as_str())
            .filter(|id| is_hex(id, 32))
            .map(str::to_ascii_lowercase)
            .collect()
    }
}

/// The records of Ryujinx's save index (`imkvdb.arc`): `IMKV`, a count, then `IMEN` entries of
/// a 64-byte key (title, user, type at 0x20) and a value that begins with the save id. Account
/// (1) and device (3) saves only.
fn ryujinx_index(b: &[u8]) -> Vec<RyuSave> {
    let u32_at = |at: usize| -> Option<u32> {
        b.get(at..at + 4)
            .map(|x| u32::from_le_bytes(x.try_into().unwrap_or([0; 4])))
    };
    let u64_at = |at: usize| -> Option<u64> {
        b.get(at..at + 8)
            .map(|x| u64::from_le_bytes(x.try_into().unwrap_or([0; 8])))
    };
    if b.get(..4) != Some(b"IMKV") {
        return Vec::new();
    }
    let Some(count) = u32_at(8) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut at = 12usize;
    for _ in 0..count {
        if b.get(at..at + 4) != Some(b"IMEN") {
            break;
        }
        let (Some(key_size), Some(value_size)) = (u32_at(at + 4), u32_at(at + 8)) else {
            break;
        };
        let key = at + 12;
        let value = key + key_size as usize;
        at = value + value_size as usize;
        if key_size < 0x22 || value_size < 8 || at > b.len() {
            break;
        }
        let kind = b[key + 0x20];
        if kind != 1 && kind != 3 {
            continue;
        }
        let (Some(title), Some(save_id)) = (u64_at(key), u64_at(value)) else {
            break;
        };
        out.push(RyuSave {
            title: format!("{title:016X}"),
            user: user_of_bytes(&b[key + 8..key + 24]),
            device: kind == 3,
            save_id: format!("{save_id:016x}"),
        });
    }
    out
}

impl Naming for Ryujinx {
    fn name_of(&self, rel: &str) -> Option<String> {
        let save_id = rel.strip_suffix("/0")?;
        if !is_hex(save_id, 16) {
            return None;
        }
        let s = self.index().into_iter().find(|s| s.save_id == save_id)?;
        Some(name_of_save(
            &s.title,
            (!s.device).then_some(s.user.as_str()),
        ))
    }

    fn rel_of(&self, name: &str, others: &[String]) -> Option<String> {
        let (title, user) = parse_name(name)?;
        let saves: Vec<RyuSave> = self
            .index()
            .into_iter()
            .filter(|s| s.title == title)
            .collect();
        let save = match user {
            None => saves.iter().find(|s| s.device),
            Some(wanted) => {
                let user =
                    local_user_for(&wanted, &siblings(name, others, &title), &self.profiles())?;
                saves.iter().find(|s| !s.device && s.user == user)
            }
        }?;
        Some(format!("{}/0", save.save_id))
    }

    /// Ryujinx works in `1` and commits into `0`; after a commit the two match.
    fn mirror(&self, rel: &str) -> Option<String> {
        rel.strip_suffix("/0").map(|s| format!("{s}/1"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An index with one record: title, user bytes, type, save id.
    pub(crate) fn index_with(rows: &[(u64, [u8; 16], u8, u64)]) -> Vec<u8> {
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
    fn names_round_trip_and_users_read_as_ryujinx_spells_them() {
        let mut id = [0u8; 16];
        id[0] = 1;
        assert_eq!(user_of_bytes(&id), DEFAULT_USER);
        assert_eq!(
            name_of_save("01006b601380e000", Some(DEFAULT_USER)),
            "01006B601380E000.account.00000000000000010000000000000000"
        );
        assert_eq!(
            parse_name("01006B601380E000.device"),
            Some(("01006B601380E000".into(), None))
        );
        assert!(parse_name("01006B601380E000.account.xyz").is_none());
        assert!(parse_name("../etc.device").is_none());
        let dir = eden_dir_of(DEFAULT_USER);
        assert_eq!(dir, "00000000000000000000000000000001");
        assert_eq!(user_of_eden_dir(&dir).as_deref(), Some(DEFAULT_USER));
    }

    #[test]
    fn a_hosts_only_profile_takes_over_the_default_users_save() {
        let local = "aaaaaaaaaaaaaaaabbbbbbbbbbbbbbbb".to_string();
        // The server holds the default user's save only: the one local profile takes it.
        assert_eq!(
            local_user_for(
                DEFAULT_USER,
                &[DEFAULT_USER.into()],
                std::slice::from_ref(&local)
            ),
            Some(local.clone())
        );
        // A server save made by this very profile is its own.
        assert_eq!(
            local_user_for(
                &local,
                std::slice::from_ref(&local),
                std::slice::from_ref(&local)
            ),
            Some(local.clone())
        );
        // Two profiles here: nothing is guessed.
        let other = "cccccccccccccccccccccccccccccccc".to_string();
        assert_eq!(
            local_user_for(
                DEFAULT_USER,
                &[DEFAULT_USER.into()],
                &[local.clone(), other]
            ),
            None
        );
        // The server also has this profile's own save: the default user's stays where it is.
        assert_eq!(
            local_user_for(
                DEFAULT_USER,
                &[DEFAULT_USER.into(), local.clone()],
                std::slice::from_ref(&local)
            ),
            None
        );
    }

    #[test]
    fn ryujinx_folders_are_named_through_its_index() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut user = [0u8; 16];
        user[0] = 1;
        let idx = index_with(&[
            (0x01006B601380E000, user, 1, 0x10),
            (0x01006B601380E000, [0; 16], 3, 0x11),
            (0x0100000000010000, user, 2, 0x12),
        ]);
        let p = root.join("bis/system/save/8000000000000000/0");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("imkvdb.arc"), idx).unwrap();
        std::fs::create_dir_all(root.join("system")).unwrap();
        std::fs::write(
            root.join("system/Profiles.json"),
            format!("\u{feff}{{\"profiles\":[{{\"user_id\":\"{DEFAULT_USER}\"}}]}}"),
        )
        .unwrap();
        let n = Ryujinx {
            root: root.to_path_buf(),
        };
        assert_eq!(
            n.name_of("0000000000000010/0").as_deref(),
            Some("01006B601380E000.account.00000000000000010000000000000000")
        );
        assert_eq!(
            n.name_of("0000000000000011/0").as_deref(),
            Some("01006B601380E000.device")
        );
        assert_eq!(n.name_of("0000000000000010/1"), None);
        assert_eq!(n.name_of("0000000000000012/0"), None, "a system save");
        assert_eq!(
            n.rel_of(
                "01006B601380E000.account.00000000000000010000000000000000",
                &[]
            )
            .as_deref(),
            Some("0000000000000010/0")
        );
        assert_eq!(
            n.rel_of("01006B601380E000.device", &[]).as_deref(),
            Some("0000000000000011/0")
        );
        assert_eq!(n.rel_of("0100AAAAAAAAAAAA.device", &[]), None);
        assert_eq!(
            n.mirror("0000000000000010/0").as_deref(),
            Some("0000000000000010/1")
        );
    }

    #[test]
    fn eden_reads_its_profiles_and_files_saves_under_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = root.join("nand/system/save/8000000000000010/su/avators");
        std::fs::create_dir_all(&p).unwrap();
        let mut dat = vec![0u8; 0x10 + 8 * 0xc8];
        let mut id = [0u8; 16];
        id[0] = 0xaa;
        id[8] = 0xbb;
        dat[0x10..0x20].copy_from_slice(&id);
        std::fs::write(p.join("profiles.dat"), dat).unwrap();
        let n = Eden {
            root: root.to_path_buf(),
        };
        let local = user_of_bytes(&id);
        assert_eq!(n.profiles(), std::slice::from_ref(&local));
        let dir_name = eden_dir_of(&local);
        assert_eq!(
            n.name_of(&format!("{dir_name}/01006B601380E000")),
            Some(format!("01006B601380E000.account.{local}"))
        );
        assert_eq!(
            n.name_of(&format!("{ZERO_USER}/01006B601380E000"))
                .as_deref(),
            Some("01006B601380E000.device")
        );
        // A Ryujinx save of the default user lands on Eden's only profile.
        assert_eq!(
            n.rel_of(
                "01006B601380E000.account.00000000000000010000000000000000",
                &[]
            ),
            Some(format!("{dir_name}/01006B601380E000"))
        );
    }
}
