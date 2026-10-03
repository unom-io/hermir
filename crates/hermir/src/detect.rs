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
    /// `path` with its links resolved, so two names for one program are one copy. As given
    /// when it cannot be resolved.
    fn canonical(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }
}

/// The real machine.
pub struct RealEnv {
    os: Os,
    home: Option<PathBuf>,
}

impl RealEnv {
    /// This machine, detected as `os`.
    pub fn new(os: Os) -> RealEnv {
        RealEnv { os, home: None }
    }

    /// This machine with another home: config roots under `~`, and the per-user variables
    /// (`HOME`, `%USERPROFILE%`, `%APPDATA%`, `%LOCALAPPDATA%`, the `XDG_*_HOME`s) resolve there,
    /// so a copy can be set up and run without touching the player's own (`hermir check`, a
    /// host's sandbox). [`RealEnv::home_vars`] are the same values, for the child process.
    pub fn with_home(os: Os, home: PathBuf) -> RealEnv {
        RealEnv {
            os,
            home: Some(home),
        }
    }
}

impl RealEnv {
    /// The per-user variables a process started in this env's home needs, so it finds its
    /// settings where this env looks for them: empty for the real home.
    pub fn home_vars(&self) -> Vec<(&'static str, PathBuf)> {
        self.home
            .as_deref()
            .map(|h| home_vars(self.os, h))
            .unwrap_or_default()
    }
}

/// Where each per-user variable points for a home of `home`.
fn home_vars(os: Os, home: &Path) -> Vec<(&'static str, PathBuf)> {
    match os {
        Os::Windows => vec![
            ("USERPROFILE", home.to_path_buf()),
            ("APPDATA", home.join("AppData").join("Roaming")),
            ("LOCALAPPDATA", home.join("AppData").join("Local")),
        ],
        _ => vec![
            ("HOME", home.to_path_buf()),
            ("XDG_CONFIG_HOME", home.join(".config")),
            ("XDG_DATA_HOME", home.join(".local/share")),
            ("XDG_CACHE_HOME", home.join(".cache")),
            ("XDG_STATE_HOME", home.join(".local/state")),
        ],
    }
}

impl Env for RealEnv {
    fn os(&self) -> Os {
        self.os
    }
    fn home(&self) -> Option<PathBuf> {
        self.home.clone().or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
        })
    }
    fn var(&self, name: &str) -> Option<String> {
        if let Some(home) = &self.home {
            let moved = home_vars(self.os, home);
            if let Some((_, v)) = moved.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
                return Some(v.to_string_lossy().into_owned());
            }
        }
        std::env::var(name).ok()
    }
    fn exists(&self, path: &Path) -> bool {
        path.is_file() || path.is_dir()
    }
    fn canonical(&self, path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }
    fn which(&self, name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        // On Windows the name as given first (it may carry its extension), then with each of
        // PATHEXT's.
        let mut exts = vec![String::new()];
        if self.os == Os::Windows {
            exts.extend(
                std::env::var("PATHEXT")
                    .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_ascii_lowercase()),
            );
        }
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
/// missing `%LOCALAPPDATA%` is a path that does not exist rather than a crash. `~` is the home
/// only on its own or before a separator; `~user` stays as written.
pub fn expand(s: &str, env: &dyn Env, app_dir: Option<&Path>) -> PathBuf {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    if let Some(r) = rest.strip_prefix('~')
        && (r.is_empty() || r.starts_with(['/', '\\']))
    {
        out.push_str(&env.home().unwrap_or_default().to_string_lossy());
        rest = r;
    }
    if let Some(r) = rest.strip_prefix("<app>") {
        out.push_str(&app_dir.unwrap_or(Path::new(".")).to_string_lossy());
        rest = r;
    }
    let var_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut i = 0;
    while let Some(c) = rest[i..].chars().next() {
        match c {
            // `%NAME%`: known, its value; unknown, as written, closing `%` included.
            '%' => {
                if let Some(end) = rest[i + 1..].find('%') {
                    let name = &rest[i + 1..i + 1 + end];
                    if !name.is_empty() && name.chars().all(|c| var_char(c) || c == '(' || c == ')')
                    {
                        match env.var(name) {
                            Some(v) => out.push_str(&v),
                            None => out.push_str(&rest[i..i + end + 2]),
                        }
                        i += end + 2;
                        continue;
                    }
                }
            }
            '$' => {
                let name: String = rest[i + 1..].chars().take_while(|c| var_char(*c)).collect();
                if !name.is_empty()
                    && let Some(v) = env.var(&name)
                {
                    out.push_str(&v);
                    i += 1 + name.len();
                    continue;
                }
            }
            _ => {}
        }
        out.push(c);
        i += c.len_utf8();
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
    // One row per program: two `PATH` names, or a `PATH` name and a known path, for one file
    // are one copy.
    let mut seen = Vec::new();
    found.retain(|i| match &i.exe {
        Exe::Path(p) => {
            let c = env.canonical(p);
            let new = !seen.contains(&c);
            seen.push(c);
            new
        }
        Exe::FlatpakRun(_) => true,
    });
    found
}

/// Whether the app is in the user installation (`$FLATPAK_USER_DIR`, by default
/// `~/.local/share/flatpak`) or the system one (`$FLATPAK_SYSTEM_DIR`, by default
/// `/var/lib/flatpak`).
fn flatpak_present(id: &str, env: &dyn Env) -> bool {
    let user = env
        .var("FLATPAK_USER_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env.home().map(|h| h.join(".local/share/flatpak")));
    let system = env
        .var("FLATPAK_SYSTEM_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from("/var/lib/flatpak"), PathBuf::from);
    user.into_iter()
        .chain(std::iter::once(system))
        .any(|dir| env.exists(&dir.join("app").join(id)))
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
        /// A link → what it points at.
        pub links: BTreeMap<PathBuf, PathBuf>,
    }

    impl FakeEnv {
        pub fn new(os: Os, home: &str) -> FakeEnv {
            FakeEnv {
                os,
                home: PathBuf::from(home),
                vars: BTreeMap::new(),
                files: BTreeSet::new(),
                on_path: BTreeMap::new(),
                links: BTreeMap::new(),
            }
        }
        pub fn link(mut self, from: &str, to: &str) -> Self {
            self.links.insert(PathBuf::from(from), PathBuf::from(to));
            self
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
        fn canonical(&self, path: &Path) -> PathBuf {
            self.links
                .get(path)
                .cloned()
                .unwrap_or_else(|| path.to_path_buf())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeEnv;
    use super::*;

    #[test]
    fn another_home_moves_the_per_user_variables_with_it() {
        let home = PathBuf::from("/tmp/check/home");
        let env = RealEnv::with_home(Os::Windows, home.clone());
        assert_eq!(env.home(), Some(home.clone()));
        assert_eq!(
            expand("%APPDATA%/Dolphin Emulator", &env, None),
            home.join("AppData")
                .join("Roaming")
                .join("Dolphin Emulator")
        );
        assert_eq!(
            env.var("localappdata"),
            Some(
                home.join("AppData")
                    .join("Local")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        let env = RealEnv::with_home(Os::Linux, home.clone());
        assert_eq!(
            env.var("XDG_CONFIG_HOME"),
            Some("/tmp/check/home/.config".into())
        );
        assert!(
            env.home_vars()
                .iter()
                .any(|(k, v)| *k == "HOME" && *v == home)
        );
        assert!(RealEnv::new(Os::Linux).home_vars().is_empty());
    }

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
        // `~user` is someone else's home: left as written.
        assert_eq!(expand("~foo/x", &env, None), PathBuf::from("~foo/x"));
        assert_eq!(expand("~", &env, None), PathBuf::from("/home/u"));
    }

    #[test]
    fn unknown_windows_variables_stay_whole_and_names_may_be_anything_unicode_around() {
        let env = FakeEnv::new(Os::Windows, "C:\\Users\\u").var("B", "XX");
        // `%A%` is unknown and stays as written; `B` after it is not taken for `%B%`.
        assert_eq!(expand("%A%B%", &env, None), PathBuf::from("%A%B%"));
        assert_eq!(expand("%A%%B%", &env, None), PathBuf::from("%A%XX"));
        assert_eq!(
            expand("C:\\Spiele\\Émulateurs\\%B%\\x", &env, None),
            PathBuf::from("C:\\Spiele\\Émulateurs\\XX\\x")
        );
        assert_eq!(expand("50% off", &env, None), PathBuf::from("50% off"));
        let env = FakeEnv::new(Os::Linux, "/home/u").var("Ü", "nope");
        assert_eq!(expand("/ä/$Ü/ö", &env, None), PathBuf::from("/ä/$Ü/ö"));
    }

    #[test]
    fn two_names_for_one_program_are_one_copy() {
        let c = Catalog::embedded().unwrap();
        let env = FakeEnv::new(Os::Linux, "/home/u")
            .bin("pcsx2-qt", "/usr/bin/pcsx2-qt")
            .bin("pcsx2", "/usr/bin/pcsx2")
            .link("/usr/bin/pcsx2", "/usr/bin/pcsx2-qt");
        let found = detect_entry(c.get("pcsx2").unwrap(), &env);
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn flatpak_installations_are_where_flatpak_says() {
        let c = Catalog::embedded().unwrap();
        let pcsx2 = c.get("pcsx2").unwrap();
        let env = FakeEnv::new(Os::Linux, "/home/u")
            .var("FLATPAK_USER_DIR", "/data/flatpak")
            .file("/data/flatpak/app/net.pcsx2.PCSX2");
        assert_eq!(detect_entry(pcsx2, &env).len(), 1);
        let env = FakeEnv::new(Os::Linux, "/home/u")
            .var("FLATPAK_SYSTEM_DIR", "/sys-flatpak")
            .file("/sys-flatpak/app/net.pcsx2.PCSX2");
        assert_eq!(detect_entry(pcsx2, &env).len(), 1);
        let env = FakeEnv::new(Os::Linux, "/home/u").file("/var/lib/flatpak/app/net.pcsx2.PCSX2");
        assert_eq!(detect_entry(pcsx2, &env).len(), 1);
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
    fn scoop_installs_are_found_on_windows_and_a_user_folder_makes_them_portable() {
        let c = Catalog::embedded().unwrap();
        let apps = "C:\\Users\\u\\scoop\\apps";
        let env = FakeEnv::new(Os::Windows, "C:\\Users\\u")
            .var("USERPROFILE", "C:\\Users\\u")
            .var("APPDATA", "C:\\Users\\u\\AppData\\Roaming")
            .file(&format!("{apps}\\flycast\\current\\flycast.exe"))
            .file(&format!("{apps}\\eden\\current\\eden.exe"))
            .file(&format!("{apps}\\eden\\current\\user"));
        let flycast = detect_entry(c.get("flycast").unwrap(), &env);
        assert_eq!(flycast.len(), 1);
        assert_eq!(flycast[0].kind, InstallKind::Native);
        assert_eq!(
            flycast[0].config_root.as_deref(),
            Some(Path::new("C:\\Users\\u\\AppData\\Roaming\\flycast"))
        );
        let eden = detect_entry(c.get("eden").unwrap(), &env);
        assert_eq!(eden[0].kind, InstallKind::Portable);
        assert_eq!(
            eden[0].config_root.as_deref(),
            Some(Path::new(&format!("{apps}\\eden\\current/user")))
        );
    }

    #[test]
    fn nothing_on_a_bare_machine() {
        let c = Catalog::embedded().unwrap();
        assert!(detect(&c, &FakeEnv::new(Os::Linux, "/home/u")).is_empty());
    }
}
