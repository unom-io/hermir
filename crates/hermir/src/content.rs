//! A game's add-ons: updates and DLC put or registered where the emulator looks for them. Each
//! emulator does it its own way, so each has an adapter; one that has none says why, in the
//! catalog, and a consumer shows that sentence instead of guessing.
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::channel::flatpak::Runner;
use crate::config::{self, ini};
use crate::model::{ContentMethod, ContentStep, Entry, Exe, Install, Os, StepOutcome};

/// The adapters a catalog entry may name.
pub(crate) const ADAPTERS: &[&str] = &["cemu", "eden", "ryujinx", "rpcs3"];

/// Platform → the add-on kinds the entry installs there.
pub fn kinds(e: &Entry) -> BTreeMap<String, Vec<String>> {
    e.content
        .iter()
        .filter_map(|(p, c)| {
            let mut k = Vec::new();
            if !matches!(c.update, ContentMethod::Unsupported { .. }) {
                k.push("update".to_string());
            }
            if !matches!(c.dlc, ContentMethod::Unsupported { .. }) {
                k.push("dlc".to_string());
            }
            (!k.is_empty()).then(|| (p.clone(), k))
        })
        .collect()
}

fn step(source: &Path, target: &Path, outcome: StepOutcome, note: Option<String>) -> ContentStep {
    ContentStep {
        source: source.to_path_buf(),
        target: target.to_path_buf(),
        outcome,
        note,
    }
}

/// Installs each of `files` as `kind` (`update` or `dlc`) of a `platform` game into `install`.
/// One step per add-on; one that fails says why and the rest go on.
pub fn install(
    entry: &Entry,
    os: Os,
    install: &Install,
    platform: &str,
    kind: &str,
    files: &[PathBuf],
    runner: &dyn Runner,
) -> Result<Vec<ContentStep>, String> {
    let c = entry
        .content
        .get(platform)
        .ok_or_else(|| format!("{} installs no add-ons for {platform}", entry.name))?;
    let method = match kind {
        "update" => &c.update,
        "dlc" => &c.dlc,
        _ => return Err(format!("{kind} is not an add-on kind; update or dlc")),
    };
    let root = install
        .config_root
        .as_deref()
        .ok_or("this copy's config root is unknown")?;
    match method {
        ContentMethod::Unsupported { unsupported } => Err(unsupported.clone()),
        ContentMethod::Installer { install: args } => Ok(files
            .iter()
            .map(|f| run(install, args, f, runner))
            .collect()),
        ContentMethod::Adapter { adapter } => match adapter.as_str() {
            "cemu" => {
                // Each title folder once, however many of its files were handed in.
                let mut titles: Vec<PathBuf> = Vec::new();
                let mut missing = Vec::new();
                for f in files {
                    let found = title_folders(f);
                    if found.is_empty() {
                        missing.push(f);
                    }
                    for t in found {
                        if !titles.contains(&t) {
                            titles.push(t);
                        }
                    }
                }
                let mut steps: Vec<ContentStep> = missing
                    .into_iter()
                    .map(|f| {
                        step(
                            f,
                            f,
                            StepOutcome::Failed,
                            Some("no title folder (with meta/meta.xml) in it".into()),
                        )
                    })
                    .collect();
                steps.extend(cemu(entry, os, install, root, &titles));
                Ok(steps)
            }
            "eden" => Ok(eden(entry, os, root, files)),
            "ryujinx" => Ok(files.iter().map(|f| ryujinx(root, kind, f)).collect()),
            "rpcs3" => Ok(files
                .iter()
                .map(|f| rpcs3(install, root, f, runner))
                .collect()),
            other => Err(format!("no content adapter {other}")),
        },
    }
}

/// The emulator's own installer on `file`; its exit says how it went.
fn run(install: &Install, args: &[String], file: &Path, runner: &dyn Runner) -> ContentStep {
    let file_arg = file.to_string_lossy().into_owned();
    let rendered: Vec<String> = args
        .iter()
        .map(|a| a.replace("{file}", &file_arg))
        .collect();
    let (program, mut argv) = match &install.exe {
        // The sandbox sees the folder the file sits in, read-only, and nothing else of it.
        Exe::FlatpakRun(id) => (
            "flatpak".to_string(),
            vec![
                "run".to_string(),
                format!(
                    "--filesystem={}:ro",
                    file.parent().unwrap_or(Path::new("/")).display()
                ),
                id.clone(),
            ],
        ),
        Exe::Path(p) => (p.to_string_lossy().into_owned(), Vec::new()),
    };
    argv.extend(rendered);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    match runner.run(&program, &refs) {
        Ok(o) if o.ok => step(file, file, StepOutcome::Applied, None),
        Ok(o) => {
            let last = o
                .stderr
                .lines()
                .chain(o.stdout.lines())
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("the installer failed")
                .trim()
                .to_string();
            step(file, file, StepOutcome::Failed, Some(last))
        }
        Err(e) => step(file, file, StepOutcome::Failed, Some(e.to_string())),
    }
}

// ── Cemu: an update or DLC is a folder (code/, content/, meta/) copied into the MLC under its
// title id, as Cemu's own "Install game title, update or DLC" lays it out. ──

fn cemu(
    entry: &Entry,
    os: Os,
    install: &Install,
    root: &Path,
    titles: &[PathBuf],
) -> Vec<ContentStep> {
    let mlc = config::get_native(entry, os, install, "main", "", "mlc_path")
        .ok()
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| config::data_dir(root).join("mlc01"));
    titles
        .iter()
        .map(|dir| {
            let Some(id) = fs::read_to_string(dir.join("meta/meta.xml"))
                .ok()
                .and_then(|x| cemu_title_id(&x))
            else {
                return step(
                    dir,
                    dir,
                    StepOutcome::Failed,
                    Some("its meta.xml names no title id".into()),
                );
            };
            let target = mlc
                .join("usr/title")
                .join(id[..8].to_ascii_lowercase())
                .join(id[8..].to_ascii_lowercase());
            match copy_tree(dir, &target) {
                Ok(()) => step(dir, &target, StepOutcome::Applied, None),
                Err(e) => step(dir, &target, StepOutcome::Failed, Some(e)),
            }
        })
        .collect()
}

/// The title folders a path is or holds: below a folder, or above one of its files.
fn title_folders(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        return find_meta(path, 3);
    }
    path.ancestors()
        .skip(1)
        .take(4)
        .find(|a| a.join("meta/meta.xml").is_file())
        .map(|a| vec![a.to_path_buf()])
        .unwrap_or_default()
}

/// Folders under `path` (itself included) that hold `meta/meta.xml`, at most `depth` down.
fn find_meta(path: &Path, depth: u8) -> Vec<PathBuf> {
    if path.join("meta/meta.xml").is_file() {
        return vec![path.to_path_buf()];
    }
    if depth == 0 || !path.is_dir() {
        return Vec::new();
    }
    let Ok(rd) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .flat_map(|e| find_meta(&e.path(), depth - 1))
        .collect();
    out.sort();
    out
}

/// `<title_id type="hexBinary" length="8">0005000E101C9500</title_id>` → `0005000E101C9500`.
fn cemu_title_id(xml: &str) -> Option<String> {
    let at = xml.find("<title_id")?;
    let open = xml[at..].find('>')? + at + 1;
    let close = xml[open..].find('<')? + open;
    let id = xml[open..close].trim();
    (id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| id.to_string())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("create {}: {e}", to.display()))?;
    let rd = fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))?;
    for e in rd.flatten() {
        let Ok(t) = e.file_type() else { continue };
        let dest = to.join(e.file_name());
        if t.is_dir() {
            copy_tree(&e.path(), &dest)?;
        } else if t.is_file() {
            fs::copy(e.path(), &dest).map_err(|e| format!("copy {}: {e}", dest.display()))?;
        }
    }
    Ok(())
}

// ── Ryujinx: an NSP is registered, not unpacked. `games/<base title>/updates.json` names the
// update files, `dlc.json` each DLC file with its data NCAs. Title ids come from the package's
// ticket, so no console keys are needed. ──

/// One file in a PFS0 (an NSP).
struct PfsFile {
    name: String,
    offset: u64,
    size: u64,
}

fn pfs0(f: &mut fs::File) -> Result<Vec<PfsFile>, String> {
    let mut head = [0u8; 16];
    f.read_exact(&mut head)
        .map_err(|_| "not an NSP (too short)".to_string())?;
    if &head[..4] != b"PFS0" {
        return Err("not an NSP (no PFS0 header)".into());
    }
    let le32 = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap_or([0; 4]));
    let count = le32(&head[4..8]) as usize;
    let strings = le32(&head[8..12]) as usize;
    if count == 0 || count > 4096 || strings > 1 << 20 {
        return Err("not an NSP (bad header)".into());
    }
    let mut table = vec![0u8; count * 0x18 + strings];
    f.read_exact(&mut table)
        .map_err(|_| "not an NSP (cut short)".to_string())?;
    let data = 16 + table.len() as u64;
    let (entries, names) = table.split_at(count * 0x18);
    let le64 = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
    entries
        .chunks_exact(0x18)
        .map(|e| {
            let at = le32(&e[16..20]) as usize;
            let end = names[at.min(names.len())..]
                .iter()
                .position(|b| *b == 0)
                .map_or(names.len(), |p| p + at);
            let name = String::from_utf8_lossy(names.get(at..end).unwrap_or_default()).into_owned();
            Ok(PfsFile {
                name,
                offset: data + le64(&e[0..8]),
                size: le64(&e[8..16]),
            })
        })
        .collect()
}

/// The title id a ticket's rights id names (its first eight bytes, as Nintendo writes them).
fn ticket_title(f: &mut fs::File, t: &PfsFile) -> Option<u64> {
    if t.size < 0x2B0 {
        return None;
    }
    f.seek(SeekFrom::Start(t.offset + 0x2A0)).ok()?;
    let mut id = [0u8; 8];
    f.read_exact(&mut id).ok()?;
    Some(u64::from_be_bytes(id))
}

fn ryujinx(root: &Path, kind: &str, file: &Path) -> ContentStep {
    match ryujinx_register(root, kind, file) {
        Ok(target) => step(file, &target, StepOutcome::Applied, None),
        Err(e) => step(file, file, StepOutcome::Failed, Some(e)),
    }
}

fn ryujinx_register(root: &Path, kind: &str, file: &Path) -> Result<PathBuf, String> {
    let mut f = fs::File::open(file).map_err(|e| format!("read {}: {e}", file.display()))?;
    let files = pfs0(&mut f)?;
    let tickets: Vec<u64> = files
        .iter()
        .filter(|x| x.name.ends_with(".tik"))
        .filter_map(|t| ticket_title(&mut f, t))
        .collect();
    let [title] = tickets[..] else {
        return Err(if tickets.is_empty() {
            "it carries no ticket, so its title is unknown; add it in Ryujinx".into()
        } else {
            "it bundles several titles; add it in Ryujinx".into()
        });
    };
    let path = file.to_string_lossy().into_owned();
    let read = |p: &Path| -> serde_json::Value {
        fs::read_to_string(p)
            .ok()
            .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
            .unwrap_or(serde_json::Value::Null)
    };
    let (base, name, value) = match kind {
        "update" => {
            if title & 0xFFF != 0x800 {
                return Err(format!("{title:016X} is not an update's title id"));
            }
            let base = title & !0xFFF;
            let target = root.join(format!("games/{base:016x}/updates.json"));
            let mut v = read(&target);
            let mut paths: Vec<String> = v["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_str().map(str::to_string))
                .filter(|p| *p != path)
                .collect();
            paths.push(path.clone());
            v = serde_json::json!({ "selected": path, "paths": paths });
            (base, "updates.json", v)
        }
        _ => {
            if title & 0xFFF == 0 || title & 0xFFF == 0x800 {
                return Err(format!("{title:016X} is not a DLC's title id"));
            }
            let base = (title ^ 0x1000) & !0xFFF;
            let target = root.join(format!("games/{base:016x}/dlc.json"));
            let ncas: Vec<serde_json::Value> = files
                .iter()
                .filter(|x| x.name.ends_with(".nca") && !x.name.ends_with(".cnmt.nca"))
                .map(|x| {
                    serde_json::json!({
                        "path": format!("/{}", x.name),
                        "title_id": title,
                        "is_enabled": true,
                    })
                })
                .collect();
            if ncas.is_empty() {
                return Err("it holds no content to register".into());
            }
            let mut rows: Vec<serde_json::Value> = read(&target)
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|r| r["path"].as_str() != Some(path.as_str()))
                .collect();
            rows.push(serde_json::json!({ "path": path, "dlc_nca_list": ncas }));
            (base, "dlc.json", serde_json::Value::Array(rows))
        }
    };
    let target = root.join(format!("games/{base:016x}/{name}"));
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    if let Some(p) = target.parent() {
        fs::create_dir_all(p).map_err(|e| format!("create {}: {e}", p.display()))?;
    }
    config::write_atomic(&target, text.as_bytes())
        .map_err(|e| format!("write {}: {e}", target.display()))?;
    Ok(target)
}

// ── Eden and Lichii: a package is read where it lies. At every start they scan each folder of
// `[UI] Paths\external_content_dirs`, subfolders included, for NSP, NSZ, XCI and XCZ files (Eden
// 0.2.1 only NSP and XCI) and take the updates and DLC in them. ──

const EDEN_DIRS: &str = "Paths\\external_content_dirs";

/// What makes Eden quote a path it writes; its reader drops every `"`.
const EDEN_QUOTE: &[char] = &[
    '!', '#', '$', '%', '^', '&', '*', '|', ';', '\'', '"', ',', '<', '>', '?', '`', '~', '=',
];

fn eden(entry: &Entry, os: Os, root: &Path, files: &[PathBuf]) -> Vec<ContentStep> {
    let Some(ini) = eden_ini(entry, os, root) else {
        let why = || Some("the catalog names no settings file".to_string());
        return files
            .iter()
            .map(|f| step(f, f, StepOutcome::Failed, why()))
            .collect();
    };
    files
        .iter()
        .map(|f| match eden_register(&entry.name, &ini, os, f) {
            Ok(outcome) => step(f, &ini, outcome, None),
            Err(e) => step(f, f, StepOutcome::Failed, Some(e)),
        })
        .collect()
}

/// Its `qt-config.ini`: the catalog's `main` file.
fn eden_ini(entry: &Entry, os: Os, root: &Path) -> Option<PathBuf> {
    let rel = entry.config.as_ref()?.files.get("main")?.path.get(os)?;
    Some(config::resolve(root, rel))
}

/// The folders this copy scans for add-ons, as its ini spells them; none for an emulator that
/// takes its add-ons another way. A Flatpak copy is granted them at launch.
pub(crate) fn scanned_dirs(entry: &Entry, os: Os, install: &Install) -> Vec<String> {
    let eden =
        |m: &ContentMethod| matches!(m, ContentMethod::Adapter { adapter } if adapter == "eden");
    if !entry
        .content
        .values()
        .any(|c| eden(&c.update) || eden(&c.dlc))
    {
        return Vec::new();
    }
    install
        .config_root
        .as_deref()
        .and_then(|root| eden_ini(entry, os, root))
        .and_then(|ini| fs::read_to_string(ini).ok())
        .map(|text| eden_dirs(&text).1)
        .unwrap_or_default()
}

/// `(size, folders)` of the list in `text`. The size is what Eden numbers the next one by.
fn eden_dirs(text: &str) -> (usize, Vec<String>) {
    // ponytail: capped so a corrupt size cannot loop for long; Eden's own UI lists a handful.
    let size = ini::get(text, "UI", &format!("{EDEN_DIRS}\\size"))
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .min(4096);
    let dirs = (1..=size)
        .filter_map(|i| ini::get(text, "UI", &format!("{EDEN_DIRS}\\{i}\\path")))
        .map(|p| p.replace('"', ""))
        .filter(|p| !p.is_empty())
        .collect();
    (size, dirs)
}

/// `dir` as Eden writes a path: `/` for `\`, no doubled `/` but a network path's leading two.
fn eden_spelling(dir: &Path) -> String {
    let s = dir.to_string_lossy().replace('\\', "/");
    let mut out = String::with_capacity(s.len() + 1);
    for c in s.chars() {
        if !(c == '/' && out.ends_with('/')) {
            out.push(c);
        }
    }
    if s.starts_with("//") {
        out.insert(0, '/');
    }
    out
}

/// Whether Eden's scan of `listed` reaches `dir`: the folder itself, or one below it.
fn covers(listed: &str, dir: &str, os: Os) -> bool {
    let fold = |s: &str| {
        let s = s.trim_end_matches('/');
        if os == Os::Windows {
            s.to_ascii_lowercase()
        } else {
            s.to_string()
        }
    };
    let (l, d) = (fold(listed), fold(dir));
    d == l || d.starts_with(&format!("{l}/"))
}

/// Lists the folder `file` lies in, unless a listed folder already reaches it.
fn eden_register(name: &str, ini_path: &Path, os: Os, file: &Path) -> Result<StepOutcome, String> {
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "nsp" | "nsz" | "xci" | "xcz") {
        return Err(format!(
            "{name} reads updates and DLC from NSP, NSZ, XCI or XCZ files"
        ));
    }
    if !file.is_file() {
        return Err(format!("read {}: no such file", file.display()));
    }
    let dir = eden_spelling(file.parent().ok_or("the file has no folder")?);
    let text = match fs::read_to_string(ini_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read {}: {e}", ini_path.display())),
    };
    let (size, listed) = eden_dirs(&text);
    if listed.iter().any(|l| covers(l, &dir, os)) {
        return Ok(StepOutcome::Present);
    }
    let value = if dir.contains(EDEN_QUOTE) {
        format!("\"{dir}\"")
    } else {
        dir
    };
    let opts = ini::IniOpts {
        sep: "=",
        case_insensitive: false,
    };
    let n = size + 1;
    let mut text = text;
    for (key, value) in [
        (format!("{EDEN_DIRS}\\{n}\\path"), value),
        (format!("{EDEN_DIRS}\\size"), n.to_string()),
    ] {
        if let Some(next) = ini::set_with(&text, "UI", &key, &value, opts)? {
            text = next;
        }
    }
    if let Some(p) = ini_path.parent() {
        fs::create_dir_all(p).map_err(|e| format!("create {}: {e}", p.display()))?;
    }
    config::write_atomic(ini_path, text.as_bytes())
        .map_err(|e| format!("write {}: {e}", ini_path.display()))?;
    Ok(StepOutcome::Applied)
}

// ── RPCS3: a `.pkg` goes through its own installer; a licence (`.rap`, `.edat`) into the
// user's `exdata`, where the installed content looks for it. ──

fn rpcs3(install: &Install, root: &Path, file: &Path, runner: &dyn Runner) -> ContentStep {
    let lower = file.to_string_lossy().to_ascii_lowercase();
    if lower.ends_with(".rap") || lower.ends_with(".edat") {
        let dir = root.join("dev_hdd0/home/00000001/exdata");
        let target = dir.join(file.file_name().unwrap_or_default());
        return match fs::create_dir_all(&dir).and_then(|()| fs::copy(file, &target)) {
            Ok(_) => step(file, &target, StepOutcome::Applied, None),
            Err(e) => step(file, &target, StepOutcome::Failed, Some(e.to_string())),
        };
    }
    if !lower.ends_with(".pkg") {
        return step(
            file,
            file,
            StepOutcome::Skipped,
            Some("not a package or a licence".into()),
        );
    }
    let mut done = run(
        install,
        &[
            "--headless".to_string(),
            "--installpkg".to_string(),
            "{file}".to_string(),
        ],
        file,
        runner,
    );
    if let Some(title) = pkg_title(file) {
        done.target = root.join("dev_hdd0/game").join(title);
    }
    done
}

/// The title id in a PKG's content id (`UP0001-BLUS30443_00-…` → `BLUS30443`).
fn pkg_title(file: &Path) -> Option<String> {
    let mut f = fs::File::open(file).ok()?;
    let mut head = [0u8; 0x60];
    f.read_exact(&mut head).ok()?;
    if &head[..4] != b"\x7fPKG" {
        return None;
    }
    let id = String::from_utf8_lossy(&head[0x30..0x54]).into_owned();
    let title = id.get(7..16)?;
    title
        .bytes()
        .all(|b| b.is_ascii_alphanumeric())
        .then(|| title.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::channel::flatpak::fake::FakeRunner;
    use crate::model::InstallKind;

    /// An NSP with these files; a `.tik` file's rights id names `title`.
    fn nsp(files: &[(&str, Option<u64>)]) -> Vec<u8> {
        let mut names = Vec::new();
        let mut offsets = Vec::new();
        for (n, _) in files {
            offsets.push(names.len() as u32);
            names.extend(n.as_bytes());
            names.push(0);
        }
        while names.len() % 16 != 0 {
            names.push(0);
        }
        let bodies: Vec<Vec<u8>> = files
            .iter()
            .map(|(_, t)| match t {
                Some(title) => {
                    let mut b = vec![0u8; 0x2C0];
                    b[0x2A0..0x2A8].copy_from_slice(&title.to_be_bytes());
                    b
                }
                None => vec![1u8; 32],
            })
            .collect();
        let mut out = b"PFS0".to_vec();
        out.extend((files.len() as u32).to_le_bytes());
        out.extend((names.len() as u32).to_le_bytes());
        out.extend(0u32.to_le_bytes());
        let mut at = 0u64;
        for (i, b) in bodies.iter().enumerate() {
            out.extend(at.to_le_bytes());
            out.extend((b.len() as u64).to_le_bytes());
            out.extend(offsets[i].to_le_bytes());
            out.extend(0u32.to_le_bytes());
            at += b.len() as u64;
        }
        out.extend(&names);
        for b in bodies {
            out.extend(b);
        }
        out
    }

    #[test]
    fn ryujinx_registers_an_update_and_a_dlc_under_the_base_title() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Ryujinx");
        let upd = tmp.path().join("Game [v65536].nsp");
        std::fs::write(
            &upd,
            nsp(&[
                ("a.nca", None),
                ("b.cnmt.nca", None),
                ("t.tik", Some(0x0100F2C0115B6800)),
            ]),
        )
        .unwrap();
        let s = ryujinx(&root, "update", &upd);
        assert_eq!(s.outcome, StepOutcome::Applied, "{s:?}");
        let v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("games/0100f2c0115b6000/updates.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["selected"], upd.to_string_lossy().as_ref());
        assert_eq!(v["paths"].as_array().unwrap().len(), 1);
        // Again: still one path.
        ryujinx(&root, "update", &upd);
        let v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("games/0100f2c0115b6000/updates.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["paths"].as_array().unwrap().len(), 1);

        let dlc = tmp.path().join("Game DLC.nsp");
        std::fs::write(
            &dlc,
            nsp(&[
                ("d.nca", None),
                ("m.cnmt.nca", None),
                ("t.tik", Some(0x0100F2C0115B7001)),
            ]),
        )
        .unwrap();
        assert_eq!(ryujinx(&root, "dlc", &dlc).outcome, StepOutcome::Applied);
        let v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("games/0100f2c0115b6000/dlc.json")).unwrap(),
        )
        .unwrap();
        let list = &v[0]["dlc_nca_list"];
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["path"], "/d.nca");
        assert_eq!(list[0]["title_id"], 0x0100F2C0115B7001u64);

        // A DLC handed in as an update is refused, and so is a file that isn't an NSP.
        assert_eq!(ryujinx(&root, "update", &dlc).outcome, StepOutcome::Failed);
        let junk = tmp.path().join("x.nsp");
        std::fs::write(&junk, b"nope").unwrap();
        assert_eq!(ryujinx(&root, "dlc", &junk).outcome, StepOutcome::Failed);
    }

    #[test]
    fn cemu_copies_a_title_folder_into_the_mlc_under_its_id() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("config/Cemu");
        std::fs::create_dir_all(&root).unwrap();
        let upd = tmp.path().join("update/Game Update");
        std::fs::create_dir_all(upd.join("meta")).unwrap();
        std::fs::create_dir_all(upd.join("code")).unwrap();
        std::fs::write(
            upd.join("meta/meta.xml"),
            "<?xml version=\"1.0\"?><menu><title_id type=\"hexBinary\" length=\"8\">0005000E101C9500</title_id></menu>",
        )
        .unwrap();
        std::fs::write(upd.join("code/app.xml"), "x").unwrap();
        let copy = Install::new(
            "cemu",
            InstallKind::Native,
            Exe::Path("/usr/bin/Cemu".into()),
            Some(root.clone()),
        );
        let steps = install(
            c.get("cemu").unwrap(),
            Os::Linux,
            &copy,
            "wiiu",
            "update",
            &[tmp.path().join("update")],
            &FakeRunner::default(),
        )
        .unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].outcome, StepOutcome::Applied, "{steps:?}");
        assert!(
            tmp.path()
                .join("data/Cemu/mlc01/usr/title/0005000e/101c9500/code/app.xml")
                .is_file()
        );
        // Its files handed in one by one: the title is copied once.
        let steps = install(
            c.get("cemu").unwrap(),
            Os::Linux,
            &copy,
            "wiiu",
            "update",
            &[upd.join("code/app.xml"), upd.join("meta/meta.xml")],
            &FakeRunner::default(),
        )
        .unwrap();
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert_eq!(steps[0].outcome, StepOutcome::Applied);
    }

    #[test]
    fn an_emulator_without_a_way_says_why() {
        let c = Catalog::embedded().unwrap();
        let copy = Install::new(
            "azahar",
            InstallKind::Native,
            Exe::Path("/usr/bin/azahar".into()),
            Some("/tmp/azahar".into()),
        );
        let err = install(
            c.get("azahar").unwrap(),
            Os::Linux,
            &copy,
            "3ds",
            "dlc",
            &[],
            &FakeRunner::default(),
        )
        .unwrap_err();
        assert!(err.contains("Azahar"), "{err}");
    }

    #[test]
    fn eden_and_lichii_scan_the_folder_an_add_on_lies_in() {
        let c = Catalog::embedded().unwrap();
        let lichii = c.get("lichii").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("home/.local/share/lichii");
        let copy = Install::new(
            "lichii",
            InstallKind::Native,
            Exe::Path("/usr/bin/lichii".into()),
            Some(root),
        );
        // A comma makes Eden quote the path.
        let game = tmp.path().join("games, all/Game");
        std::fs::create_dir_all(game.join("update")).unwrap();
        std::fs::create_dir_all(game.join("dlc/more")).unwrap();
        let upd = game.join("update/Game [v65536].nsp");
        let dlc = game.join("dlc/Game DLC.nsz");
        let deeper = game.join("dlc/more/More.xci");
        let zip = game.join("update/Game.zip");
        for f in [&upd, &dlc, &deeper, &zip] {
            std::fs::write(f, b"PFS0").unwrap();
        }
        let run = |kind: &str, files: &[PathBuf]| {
            let steps = install(
                lichii,
                Os::Linux,
                &copy,
                "switch2",
                kind,
                files,
                &FakeRunner::default(),
            )
            .unwrap();
            steps.into_iter().map(|s| s.outcome).collect::<Vec<_>>()
        };
        assert_eq!(
            run("update", std::slice::from_ref(&upd)),
            [StepOutcome::Applied]
        );
        // A second folder; then files the listed folders already reach.
        assert_eq!(
            run("dlc", &[dlc, deeper, upd]),
            [
                StepOutcome::Applied,
                StepOutcome::Present,
                StepOutcome::Present
            ]
        );
        assert_eq!(run("update", &[zip]), [StepOutcome::Failed]);
        let ini =
            std::fs::read_to_string(tmp.path().join("home/.config/lichii/qt-config.ini")).unwrap();
        let g = eden_spelling(&game);
        assert!(
            ini.starts_with(&format!(
                "[UI]\nPaths\\external_content_dirs\\1\\path=\"{g}/update\"\n"
            )),
            "{ini}"
        );
        let want = vec![format!("{g}/update"), format!("{g}/dlc")];
        assert_eq!(eden_dirs(&ini), (2, want.clone()));
        assert_eq!(scanned_dirs(lichii, Os::Linux, &copy), want);
        assert!(lichii.content["switch2"].dlc.keeps_path());
        assert!(!c.get("cemu").unwrap().content["wiiu"].dlc.keeps_path());
    }

    #[test]
    fn a_path_is_spelled_as_eden_writes_it() {
        assert_eq!(
            eden_spelling(Path::new(r"C:\Games\\Zelda")),
            "C:/Games/Zelda"
        );
        assert_eq!(eden_spelling(Path::new(r"\\nas\roms")), "//nas/roms");
        assert!(covers("C:/games/", "c:/Games/Zelda/update", Os::Windows));
        assert!(!covers("/games/Zel", "/games/Zelda", Os::Linux));
    }

    #[test]
    fn rpcs3_installs_a_package_headless_and_files_a_licence() {
        let c = Catalog::embedded().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("rpcs3");
        let pkg = tmp.path().join("dlc.pkg");
        let mut head = vec![0u8; 0x80];
        head[..4].copy_from_slice(b"\x7fPKG");
        head[0x30..0x54].copy_from_slice(b"UP0001-BLUS30443_00-DLCPACK000000000");
        std::fs::write(&pkg, head).unwrap();
        let rap = tmp.path().join("UP0001-BLUS30443_00-DLCPACK000000000.rap");
        std::fs::write(&rap, [0u8; 16]).unwrap();
        let copy = Install::new(
            "rpcs3",
            InstallKind::Flatpak,
            Exe::FlatpakRun("net.rpcs3.RPCS3".into()),
            Some(root.clone()),
        );
        let runner = FakeRunner::default();
        let steps = install(
            c.get("rpcs3").unwrap(),
            Os::Linux,
            &copy,
            "ps3",
            "dlc",
            &[pkg.clone(), rap.clone()],
            &runner,
        )
        .unwrap();
        assert_eq!(steps[0].outcome, StepOutcome::Applied, "{steps:?}");
        assert_eq!(steps[0].target, root.join("dev_hdd0/game/BLUS30443"));
        assert!(
            root.join("dev_hdd0/home/00000001/exdata")
                .join(rap.file_name().unwrap())
                .is_file()
        );
        let calls = runner.calls.lock().unwrap();
        let call = calls.first().unwrap();
        assert_eq!(call[0], "flatpak");
        assert!(call.iter().any(|a| a == "--installpkg"));
        assert!(call.iter().any(|a| a == "--headless"));
    }
}
