//! Save units: the pieces of an emulator's save folders a sync client moves one at a time. A
//! unit is named for what it holds, so one save has one name on every machine; listing gives a
//! stamp that changes with the save, export packs one into a file, import puts one back. hermir
//! never decides which side wins: that is the sync client's call.
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use md5::{Digest, Md5};

use crate::detect::Env;
use crate::model::{
    Entry, ExportedUnit, Install, Os, PlatformSaves, PrepareStep, SaveKind, SaveUnit, StepOutcome,
    UnitShape, Units,
};
use crate::saves::locate;

mod switch;

/// The save adapters a catalog entry may name.
pub(crate) use switch::ADAPTERS;

/// What an emulator writes beside its saves that is never one.
const NOISE: [&str; 6] = [".png", ".jpg", ".log", ".tmp", ".bak", ".part"];

/// Platforms whose saves are listed as units, `*` spelled out.
pub fn platforms_with_units(e: &Entry) -> Vec<String> {
    e.platforms
        .iter()
        .filter(|p| {
            matches!(e.saves.get(*p).or_else(|| e.saves.get("*")),
                Some(PlatformSaves::Dirs(dirs)) if dirs.iter().any(|d| d.units.is_some()))
        })
        .cloned()
        .collect()
}

/// One save folder of a copy, resolved, with how it splits.
struct Root<'a> {
    kind: SaveKind,
    dir: PathBuf,
    units: &'a Units,
    pattern: Option<&'a str>,
    naming: Option<Box<dyn switch::Naming>>,
}

fn roots<'a>(
    entry: &'a Entry,
    os: Os,
    install: &Install,
    env: &dyn Env,
    platform: &str,
    game: Option<&Path>,
) -> Result<Vec<Root<'a>>, String> {
    if !entry.platforms.iter().any(|p| p == platform) {
        return Err(format!("{} does not run {platform}", entry.name));
    }
    let Some(PlatformSaves::Dirs(dirs)) =
        entry.saves.get(platform).or_else(|| entry.saves.get("*"))
    else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for d in dirs {
        let Some(units) = d.units.as_ref() else {
            continue;
        };
        let loc = locate(entry, os, install, env, d);
        let dir = match (loc.path, loc.beside_game, game) {
            (Some(p), _, _) => p,
            (None, true, Some(g)) => g.to_path_buf(),
            _ => continue,
        };
        let naming = match units.adapter.as_deref() {
            None => None,
            Some(name) => {
                let root = install
                    .config_root
                    .as_deref()
                    .ok_or("this copy's config root is unknown")?;
                Some(switch::naming(name, root).ok_or(format!("no save adapter {name}"))?)
            }
        };
        out.push(Root {
            kind: d.kind,
            dir,
            units,
            pattern: d.pattern.as_deref(),
            naming,
        });
    }
    Ok(out)
}

/// Every unit of `platform`'s saves on this copy. `game` is the game's folder, for an emulator
/// that saves beside the game.
pub fn list(
    entry: &Entry,
    os: Os,
    install: &Install,
    env: &dyn Env,
    platform: &str,
    game: Option<&Path>,
) -> Result<Vec<SaveUnit>, String> {
    let mut out = Vec::new();
    for root in roots(entry, os, install, env, platform, game)? {
        let mut found: BTreeMap<String, SaveUnit> = BTreeMap::new();
        for f in walk(&root.dir) {
            let Some((name, unit_rel)) = unit_of(&root, &f.rel) else {
                continue;
            };
            let u = found.entry(name.clone()).or_insert_with(|| SaveUnit {
                kind: root.kind,
                name,
                shared: matches_any(&root.units.shared, &f.rel),
                written_at_start: root.units.written_at_start,
                size: 0,
                files: 0,
                modified: 0,
                path: join_rel(&root.dir, &unit_rel),
            });
            u.size += f.size;
            u.files += 1;
            u.modified = u.modified.max(f.modified);
        }
        out.extend(found.into_values());
    }
    Ok(out)
}

/// The unit a file under a root belongs to, and the unit's path under the root.
fn unit_of(root: &Root, rel: &str) -> Option<(String, String)> {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    if NOISE.iter().any(|n| base.to_ascii_lowercase().ends_with(n)) {
        return None;
    }
    let lead = root
        .units
        .under
        .as_deref()
        .map(|u| format!("{u}__"))
        .unwrap_or_default();
    match root.units.shape {
        UnitShape::Files => {
            if root.pattern.is_some_and(|p| !glob(p, base)) {
                return None;
            }
            Some((format!("{lead}{}", rel.replace('/', "__")), rel.to_string()))
        }
        UnitShape::Folders => {
            let depth = usize::from(root.units.depth.unwrap_or(1).max(1));
            let parts: Vec<&str> = rel.split('/').collect();
            if parts.len() <= depth {
                return None; // a loose file beside the units is not one
            }
            let top = parts[..depth].join("/");
            let inside = parts[depth..].join("/");
            if !root.units.keep.is_empty() && !matches_any(&root.units.keep, &inside) {
                return None;
            }
            if parts[..depth]
                .iter()
                .any(|p| p.to_ascii_lowercase().ends_with(".bak"))
            {
                return None;
            }
            let name = match &root.naming {
                Some(n) => n.name_of(&top)?,
                None => format!("{lead}{}", top.replace('/', "__")),
            };
            Some((format!("{name}.tar"), top))
        }
    }
}

/// Packs one unit into `out` (a folder) as `<name>`: the save file, or a tar of the folder's
/// kept files with no time, owner or mode in it.
#[allow(clippy::too_many_arguments)]
pub fn export(
    entry: &Entry,
    os: Os,
    install: &Install,
    env: &dyn Env,
    platform: &str,
    game: Option<&Path>,
    kind: SaveKind,
    name: &str,
    out: &Path,
) -> Result<ExportedUnit, String> {
    safe_name(name)?;
    let roots = roots(entry, os, install, env, platform, game)?;
    let (root, rel) = roots
        .iter()
        .filter(|r| r.kind == kind)
        .find_map(|r| {
            let rel = here(r, name)?;
            join_rel(&r.dir, &rel).exists().then_some((r, rel))
        })
        .ok_or_else(|| format!("no unit {name} on this copy"))?;
    fs::create_dir_all(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let file = out.join(name);
    let src = join_rel(&root.dir, &rel);
    match root.units.shape {
        UnitShape::Files => {
            fs::copy(&src, &file).map_err(|e| format!("copy {}: {e}", src.display()))?;
        }
        UnitShape::Folders => {
            let bytes = pack(&src, &root.units.keep)?;
            fs::write(&file, bytes).map_err(|e| format!("write {}: {e}", file.display()))?;
        }
    }
    let (size, md5) = md5_file(&file)?;
    Ok(ExportedUnit {
        name: name.to_string(),
        file,
        size,
        md5,
    })
}

/// The unit's path under its root on this machine, from its name; `None` when this machine
/// has no place for it yet (a Switch title that never ran here).
fn here(root: &Root, name: &str) -> Option<String> {
    let bare = match root.units.shape {
        UnitShape::Files => name,
        UnitShape::Folders => name.strip_suffix(".tar")?,
    };
    let rel = match (&root.naming, root.units.under.as_deref()) {
        (Some(n), _) => n.rel_of(bare, &[])?,
        (None, Some(u)) => bare.strip_prefix(&format!("{u}__"))?.replace("__", "/"),
        (None, None) => bare.replace("__", "/"),
    };
    rel_ok(root, &rel).then_some(rel)
}

fn rel_ok(root: &Root, rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    let depth_ok = match root.units.shape {
        UnitShape::Files => true,
        UnitShape::Folders => parts.len() == usize::from(root.units.depth.unwrap_or(1).max(1)),
    };
    depth_ok
        && parts.iter().all(|p| {
            !p.is_empty() && *p != "." && *p != ".." && !p.contains('\\') && !p.contains(':')
        })
}

/// A unit name never carries a path.
fn safe_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 255
        || name.contains('/')
        || name.contains('\\')
        || name.starts_with('.')
        || name.chars().any(char::is_control)
    {
        return Err(format!("{name:?} is not a unit name"));
    }
    Ok(())
}

/// Puts the unit in `from` (a file export wrote, or a server's copy) where this copy keeps it,
/// in the first folder of `kinds` that has a place for its name.
/// What was there is moved to `backups` first; a folder unit's files beside the save (`keep`)
/// stay. `others` are the names a server holds for the same game, which a Switch emulator with
/// one profile uses to take over a save made under another. Nothing is written when this copy
/// has no place for the unit yet; the step says why.
#[allow(clippy::too_many_arguments)]
pub fn import(
    entry: &Entry,
    os: Os,
    install: &Install,
    env: &dyn Env,
    platform: &str,
    game: Option<&Path>,
    kinds: &[SaveKind],
    name: &str,
    from: &Path,
    others: &[String],
    backups: &Path,
) -> Result<PrepareStep, String> {
    safe_name(name)?;
    let roots = roots(entry, os, install, env, platform, game)?;
    // In the order asked: a server slot does not say whether it was a save or a game's card.
    let candidates: Vec<&Root> = kinds
        .iter()
        .flat_map(|k| roots.iter().filter(move |r| r.kind == *k))
        .collect();
    let shaped = |r: &Root| match r.units.shape {
        UnitShape::Folders => name.ends_with(".tar"),
        UnitShape::Files => !name.ends_with(".tar"),
    };
    let placed = candidates.iter().filter(|r| shaped(r)).find_map(|r| {
        let bare = name.strip_suffix(".tar").unwrap_or(name);
        let rel = match (&r.naming, r.units.under.as_deref()) {
            (Some(n), _) => n.rel_of(bare, others),
            (None, Some(u)) => bare
                .strip_prefix(&format!("{u}__"))
                .map(|x| x.replace("__", "/")),
            (None, None) => {
                // A name led by another folder's `under` is that folder's.
                let led = candidates
                    .iter()
                    .filter_map(|o| o.units.under.as_deref())
                    .any(|u| bare.starts_with(&format!("{u}__")));
                (!led).then(|| bare.replace("__", "/"))
            }
        }?;
        rel_ok(r, &rel).then_some((*r, rel))
    });
    let Some((root, rel)) = placed else {
        return Ok(PrepareStep {
            kind: "save".into(),
            target: PathBuf::from(name),
            outcome: StepOutcome::Skipped,
            note: Some(
                "this copy has no place for it yet: start the game once, and it is restored \
                 the next time"
                    .into(),
            ),
        });
    };
    let dest = join_rel(&root.dir, &rel);
    if dest.exists() {
        backup(&dest, &root.units.keep, &backups.join(stamp()).join(name))?;
    }
    match root.units.shape {
        UnitShape::Files => {
            let parent = dest.parent().ok_or("no folder to write into")?;
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
            let tmp = parent.join(format!(
                ".{}.hermir",
                rel.rsplit('/').next().unwrap_or("save")
            ));
            fs::copy(from, &tmp).map_err(|e| format!("copy {}: {e}", from.display()))?;
            fs::rename(&tmp, &dest).map_err(|e| format!("place {}: {e}", dest.display()))?;
        }
        UnitShape::Folders => {
            let bytes = fs::read(from).map_err(|e| format!("read {}: {e}", from.display()))?;
            replace_folder(&dest, &root.units.keep, &bytes)?;
            // A working copy beside the save (Ryujinx's `1`) is rewritten to match, as a commit
            // would leave it.
            if let Some(m) = root.naming.as_ref().and_then(|n| n.mirror(&rel)) {
                let mirror = join_rel(&root.dir, &m);
                if mirror.exists() {
                    replace_folder(&mirror, &root.units.keep, &bytes)?;
                }
            }
        }
    }
    Ok(PrepareStep {
        kind: "save".into(),
        target: dest,
        outcome: StepOutcome::Applied,
        note: None,
    })
}

fn replace_folder(dest: &Path, keep: &[String], tar: &[u8]) -> Result<(), String> {
    if keep.is_empty() {
        if dest.exists() {
            fs::remove_dir_all(dest).map_err(|e| format!("clear {}: {e}", dest.display()))?;
        }
    } else {
        for f in walk(dest).into_iter().filter(|f| matches_any(keep, &f.rel)) {
            let p = join_rel(dest, &f.rel);
            fs::remove_file(&p).map_err(|e| format!("clear {}: {e}", p.display()))?;
        }
    }
    fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    unpack(tar, dest)
}

fn backup(src: &Path, keep: &[String], to: &Path) -> Result<(), String> {
    if src.is_file() {
        if let Some(p) = to.parent() {
            fs::create_dir_all(p).map_err(|e| format!("create {}: {e}", p.display()))?;
        }
        fs::copy(src, to).map_err(|e| format!("back up {}: {e}", src.display()))?;
        return Ok(());
    }
    for f in walk(src)
        .into_iter()
        .filter(|f| keep.is_empty() || matches_any(keep, &f.rel))
    {
        let dest = join_rel(to, &f.rel);
        if let Some(p) = dest.parent() {
            fs::create_dir_all(p).map_err(|e| format!("create {}: {e}", p.display()))?;
        }
        let from = join_rel(src, &f.rel);
        fs::copy(&from, &dest).map_err(|e| format!("back up {}: {e}", from.display()))?;
    }
    Ok(())
}

fn stamp() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    ms.to_string()
}

/// A tar of `dir`'s files (the `keep` ones, when given), paths relative to it, sorted, every
/// header without time, owner or mode: the same save is the same bytes everywhere.
pub fn pack(dir: &Path, keep: &[String]) -> Result<Vec<u8>, String> {
    let mut b = tar::Builder::new(Vec::new());
    let mut files = walk(dir);
    files.retain(|f| keep.is_empty() || matches_any(keep, &f.rel));
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    for f in files {
        let path = join_rel(dir, &f.rel);
        let mut data = Vec::new();
        fs::File::open(&path)
            .and_then(|mut h| h.read_to_end(&mut data))
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        let mut h = tar::Header::new_ustar();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(0);
        h.set_uid(0);
        h.set_gid(0);
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, &f.rel, data.as_slice())
            .map_err(|e| format!("pack {}: {e}", f.rel))?;
    }
    b.into_inner().map_err(|e| format!("pack: {e}"))
}

/// Writes a tar's regular files under `dir`. An entry that would land outside it is refused
/// before anything is written.
pub fn unpack(bytes: &[u8], dir: &Path) -> Result<(), String> {
    let mut entries = Vec::new();
    let mut a = tar::Archive::new(bytes);
    for e in a.entries().map_err(|e| format!("read the save: {e}"))? {
        let mut e = e.map_err(|e| format!("read the save: {e}"))?;
        let kind = e.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err("the save holds something other than files".into());
        }
        let path = e
            .path()
            .map_err(|e| format!("read the save: {e}"))?
            .into_owned();
        let rel = path.to_string_lossy().replace('\\', "/");
        let parts: Vec<&str> = rel
            .split('/')
            .filter(|p| !p.is_empty() && *p != ".")
            .collect();
        if parts.is_empty() || parts.contains(&"..") || rel.starts_with('/') || rel.contains(':') {
            return Err(format!("the save's entry {rel} leaves its folder"));
        }
        let mut data = Vec::new();
        e.read_to_end(&mut data)
            .map_err(|e| format!("read the save: {e}"))?;
        entries.push((parts.join("/"), data));
    }
    for (rel, data) in entries {
        let dest = join_rel(dir, &rel);
        if let Some(p) = dest.parent() {
            fs::create_dir_all(p).map_err(|e| format!("create {}: {e}", p.display()))?;
        }
        fs::File::create(&dest)
            .and_then(|mut f| f.write_all(&data))
            .map_err(|e| format!("write {}: {e}", dest.display()))?;
    }
    Ok(())
}

pub(crate) fn md5_file(path: &Path) -> Result<(u64, String), String> {
    let mut f = fs::File::open(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut h = Md5::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        size += n as u64;
        h.update(&buf[..n]);
    }
    let out = h.finalize();
    Ok((size, out.iter().map(|b| format!("{b:02x}")).collect()))
}

struct Found {
    rel: String,
    size: u64,
    modified: u64,
}

/// Every regular file under `dir`, `/`-separated relative paths; links are not followed.
fn walk(dir: &Path) -> Vec<Found> {
    fn visit(d: &Path, prefix: &str, out: &mut Vec<Found>) {
        let Ok(rd) = fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if t.is_dir() {
                visit(&e.path(), &rel, out);
            } else if t.is_file()
                && let Ok(m) = e.metadata()
            {
                let modified = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as u64);
                out.push(Found {
                    rel,
                    size: m.len(),
                    modified,
                });
            }
        }
    }
    let mut out = Vec::new();
    visit(dir, "", &mut out);
    out
}

fn join_rel(dir: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(dir.to_path_buf(), |p, s| p.join(s))
}

fn matches_any(globs: &[String], rel: &str) -> bool {
    globs.iter().any(|g| glob(g, rel))
}

/// `*` and `?` within a segment, `**` across segments, `[AB]` one of. Case-sensitive, as the
/// emulators' own file names are.
pub(crate) fn glob(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            // `**/` is zero or more whole segments; a bare `**` anything at all.
            Some('*') if p.get(1) == Some(&'*') => match p.get(2) {
                Some('/') => {
                    (0..=t.len()).any(|i| (i == 0 || t[i - 1] == '/') && go(&p[3..], &t[i..]))
                }
                _ => (0..=t.len()).any(|i| go(&p[2..], &t[i..])),
            },
            Some('*') => (0..=t.len())
                .take_while(|&i| i == 0 || t[i - 1] != '/')
                .any(|i| go(&p[1..], &t[i..])),
            Some('?') => t.first().is_some_and(|c| *c != '/') && go(&p[1..], &t[1..]),
            Some('[') => {
                let Some(end) = p.iter().position(|c| *c == ']') else {
                    return t.first() == Some(&'[') && go(&p[1..], &t[1..]);
                };
                t.first().is_some_and(|c| p[1..end].contains(c)) && go(&p[end + 1..], &t[1..])
            }
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    go(&p, &t)
}

#[cfg(test)]
mod tests;
