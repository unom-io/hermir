//! Finding emulators the user installed: `PATH` names, Flatpak app ids, known paths, portable
//! markers. Pure over an `Env`, so both operating systems are tested from fake trees.
use std::path::{Path, PathBuf};

use crate::catalog::Catalog;
use crate::model::{Detect, Entry, Exe, Install, InstallKind, Os, Roots};

/// What detection may ask of the machine.
pub trait Env: Send + Sync {
    /// The OS detection runs for.
    fn os(&self) -> Os;
    /// The user's home directory.
    fn home(&self) -> Option<PathBuf>;
    /// An environment variable.
    fn var(&self, name: &str) -> Option<String>;
    /// Whether `path` exists.
    fn exists(&self, path: &Path) -> bool;
    /// `name` resolved on `PATH`, with `PATHEXT` on Windows.
    fn which(&self, name: &str) -> Option<PathBuf>;
}

/// The real machine.
pub struct RealEnv {
    os: Os,
}

impl RealEnv {
    /// This machine, detected as `os`.
    pub fn new(os: Os) -> RealEnv {
        RealEnv { os }
    }
}

impl Env for RealEnv {
    fn os(&self) -> Os {
        self.os
    }
    fn home(&self) -> Option<PathBuf> {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    }
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
    fn exists(&self, path: &Path) -> bool {
        path.is_file() || path.is_dir()
    }
    fn which(&self, name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        let exts: Vec<String> = if self.os == Os::Windows {
            std::env::var("PATHEXT")
                .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
                .split(';')
                .map(|s| s.to_ascii_lowercase())
                .collect()
        } else {
            vec![String::new()]
        };
        for dir in std::env::split_paths(&path) {
            for ext in &exts {
                let p = dir.join(format!("{name}{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    }
}

/// `~`, `$VAR`, `%VAR%` and `<app>` (given) expanded. Unknown variables stay as written, so a
/// missing `%LOCALAPPDATA%` is a path that does not exist rather than a crash.
pub fn expand(s: &str, env: &dyn Env, app_dir: Option<&Path>) -> PathBuf {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    if let Some(r) = rest.strip_prefix("~") {
        out.push_str(&env.home().unwrap_or_default().to_string_lossy());
        rest = r;
    }
    if let Some(r) = rest.strip_prefix("<app>") {
        out.push_str(&app_dir.unwrap_or(Path::new(".")).to_string_lossy());
        rest = r;
    }
    let mut chars = rest.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '%' => {
                if let Some(end) = rest[i + 1..].find('%') {
                    let name = &rest[i + 1..i + 1 + end];
                    if let Some(v) = env.var(name) {
                        out.push_str(&v);
                        for _ in 0..end + 1 {
                            chars.next();
                        }
                        continue;
                    }
                }
                out.push(c);
            }
            '$' => {
                let name: String = rest[i + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty()
                    && let Some(v) = env.var(&name)
                {
                    out.push_str(&v);
                    for _ in 0..name.len() {
                        chars.next();
                    }
                    continue;
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    PathBuf::from(out)
}

/// Every copy of every catalog emulator this machine has.
pub fn detect(catalog: &Catalog, env: &dyn Env) -> Vec<Install> {
    catalog
        .entries()
        .iter()
        .flat_map(|e| detect_entry(e, env))
        .collect()
}

pub fn detect_entry(entry: &Entry, env: &dyn Env) -> Vec<Install> {
    let os = env.os();
    let Some(rules) = entry.detect.get(os) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    if let Some(id) = &rules.flatpak
        && flatpak_present(id, env)
    {
        found.push(Install {
            emulator: entry.id.clone(),
            kind: InstallKind::Flatpak,
            exe: Exe::FlatpakRun(id.clone()),
            version: None,
            config_root: entry.roots.flatpak.as_deref().map(|r| expand(r, env, None)),
        });
    }
    for name in &rules.path {
        if let Some(exe) = env.which(name) {
            found.push(native(entry, rules, exe, env));
        }
    }
    for p in &rules.paths {
        let exe = expand(p, env, None);
        if env.exists(&exe) {
            found.push(native(entry, rules, exe, env));
        }
    }
    found.dedup_by(|a, b| a.exe == b.exe);
    found
}

fn flatpak_present(id: &str, env: &dyn Env) -> bool {
    let user = env
        .home()
        .map(|h| h.join(".local/share/flatpak/app").join(id))
        .is_some_and(|p| env.exists(&p));
    user || env.exists(&Path::new("/var/lib/flatpak/app").join(id))
}

/// The directory of `exe` under `os` path rules, so a Windows path parses on any host.
fn dir_of(os: Os, exe: &Path) -> Option<PathBuf> {
    if os == Os::Windows {
        let s = exe.to_string_lossy();
        let cut = s.rfind(['\\', '/'])?;
        Some(PathBuf::from(&s[..cut]))
    } else {
        exe.parent().map(Path::to_path_buf)
    }
}

fn join_for(os: Os, dir: &Path, name: &str) -> PathBuf {
    if os == Os::Windows {
        PathBuf::from(format!("{}\\{name}", dir.to_string_lossy()))
    } else {
        dir.join(name)
    }
}

fn native(entry: &Entry, rules: &Detect, exe: PathBuf, env: &dyn Env) -> Install {
    let app_dir = dir_of(env.os(), &exe);
    let portable = rules
        .portable_marker
        .as_ref()
        .zip(app_dir.as_ref())
        .is_some_and(|(m, d)| env.exists(&join_for(env.os(), d, m)));
    let kind = if portable {
        InstallKind::Portable
    } else {
        InstallKind::Native
    };
    Install {
        emulator: entry.id.clone(),
        kind,
        config_root: config_root(&entry.roots, kind, env, app_dir.as_deref()),
        exe: Exe::Path(exe),
        version: None,
    }
}

fn config_root(
    roots: &Roots,
    kind: InstallKind,
    env: &dyn Env,
    app_dir: Option<&Path>,
) -> Option<PathBuf> {
    let template = match (kind, env.os()) {
        (InstallKind::Portable, _) | (InstallKind::Managed, _) => roots.portable.as_deref(),
        (InstallKind::Flatpak, _) => roots.flatpak.as_deref(),
        (InstallKind::Native, Os::Windows) => roots.windows.as_deref(),
        (InstallKind::Native, _) => roots.native.as_deref(),
    }?;
    Some(expand(template, env, app_dir))
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    /// A machine described as a set of paths and variables.
    pub struct FakeEnv {
        pub os: Os,
        pub home: PathBuf,
        pub vars: BTreeMap<String, String>,
        pub files: BTreeSet<PathBuf>,
        pub on_path: BTreeMap<String, PathBuf>,
    }

    impl FakeEnv {
        pub fn new(os: Os, home: &str) -> FakeEnv {
            FakeEnv {
                os,
                home: PathBuf::from(home),
                vars: BTreeMap::new(),
                files: BTreeSet::new(),
                on_path: BTreeMap::new(),
            }
        }
        pub fn file(mut self, p: &str) -> Self {
            self.files.insert(PathBuf::from(p));
            self
        }
        pub fn var(mut self, k: &str, v: &str) -> Self {
            self.vars.insert(k.into(), v.into());
            self
        }
        pub fn bin(mut self, name: &str, p: &str) -> Self {
            self.on_path.insert(name.into(), PathBuf::from(p));
            self
        }
    }

    impl Env for FakeEnv {
        fn os(&self) -> Os {
            self.os
        }
        fn home(&self) -> Option<PathBuf> {
            Some(self.home.clone())
        }
        fn var(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned()
        }
        fn exists(&self, path: &Path) -> bool {
            self.files.contains(path)
        }
        fn which(&self, name: &str) -> Option<PathBuf> {
            self.on_path.get(name).cloned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeEnv;
    use super::*;

    #[test]
    fn expands_home_and_variables() {
        let env = FakeEnv::new(Os::Windows, "/home/u")
            .var("LOCALAPPDATA", "C:\\Users\\u\\AppData\\Local");
        assert_eq!(
            expand("%LOCALAPPDATA%\\Azahar\\azahar.exe", &env, None),
            PathBuf::from("C:\\Users\\u\\AppData\\Local\\Azahar\\azahar.exe")
        );
        assert_eq!(expand("%NOPE%\\x", &env, None), PathBuf::from("%NOPE%\\x"));
        let env = FakeEnv::new(Os::Linux, "/home/u").var("XDG_CONFIG_HOME", "/cfg");
        assert_eq!(
            expand("~/.config/PCSX2", &env, None),
            PathBuf::from("/home/u/.config/PCSX2")
        );
        assert_eq!(
            expand("$XDG_CONFIG_HOME/PCSX2", &env, None),
            PathBuf::from("/cfg/PCSX2")
        );
        assert_eq!(
            expand("<app>/inis", &env, Some(Path::new("/opt/p"))),
            PathBuf::from("/opt/p/inis")
        );
    }

    #[test]
    fn finds_flatpak_path_and_portable_copies() {
        let c = Catalog::embedded().unwrap();
        let pcsx2 = c.get("pcsx2").unwrap();
        let env = FakeEnv::new(Os::Linux, "/home/u")
            .file("/home/u/.local/share/flatpak/app/net.pcsx2.PCSX2")
            .bin("pcsx2-qt", "/usr/bin/pcsx2-qt");
        let found = detect_entry(pcsx2, &env);
        let kinds: Vec<_> = found.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, vec![InstallKind::Flatpak, InstallKind::Native]);
        assert_eq!(
            found[0].config_root.as_deref(),
            Some(Path::new("/home/u/.var/app/net.pcsx2.PCSX2/config/PCSX2"))
        );

        let env = FakeEnv::new(Os::Windows, "C:\\Users\\u")
            .var("ProgramFiles", "C:\\Program Files")
            .file("C:\\Program Files\\PCSX2\\pcsx2-qt.exe")
            .file("C:\\Program Files\\PCSX2\\portable.ini");
        let found = detect_entry(pcsx2, &env);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, InstallKind::Portable);
        assert_eq!(
            found[0].config_root.as_deref(),
            Some(Path::new("C:\\Program Files\\PCSX2"))
        );
    }

    #[test]
    fn nothing_on_a_bare_machine() {
        let c = Catalog::embedded().unwrap();
        assert!(detect(&c, &FakeEnv::new(Os::Linux, "/home/u")).is_empty());
    }
}
