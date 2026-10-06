//! Flatpak is the Linux channel: `flatpak` does the fetching, hermir asks it. Behind a
//! `Runner`, so the argv is tested and the CLI runs the real thing.
use std::process::Command;

use crate::error::{Error, Result};

/// How a [`Runner`]'s program went.
pub struct Output {
    /// Whether it exited with success.
    pub ok: bool,
    /// What it wrote to stdout; empty when the runner can't capture it.
    pub stdout: String,
    /// What it wrote to stderr; empty when the runner can't capture it. Shown when a step fails.
    pub stderr: String,
}

/// How hermir runs a program (`flatpak`, an emulator's own firmware installer): [`Process`]
/// runs it, a test answers instead.
pub trait Runner: Send + Sync {
    /// Runs `program` with `args`, no shell, and says how it went.
    fn run(&self, program: &str, args: &[&str]) -> Result<Output>;
}

/// Runs processes.
pub struct Process;

impl Runner for Process {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
        let out = Command::new(program)
            .args(args)
            .output()
            .map_err(|e| Error::Place {
                what: program.into(),
                why: if e.kind() == std::io::ErrorKind::NotFound {
                    "not installed".into()
                } else {
                    e.to_string()
                },
            })?;
        Ok(Output {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

const FLATHUB: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

fn check(what: &str, out: Output) -> Result<Output> {
    if out.ok {
        Ok(out)
    } else {
        let why = out
            .stderr
            .lines()
            .last()
            .unwrap_or("failed")
            .trim()
            .to_string();
        Err(Error::Place {
            what: what.into(),
            why,
        })
    }
}

/// `flatpak install --user`, with the Flathub remote added for the user if it is missing.
pub fn install(runner: &dyn Runner, id: &str) -> Result<()> {
    check(
        "flatpak remote-add",
        runner.run(
            "flatpak",
            &[
                "remote-add",
                "--user",
                "--if-not-exists",
                "flathub",
                FLATHUB,
            ],
        )?,
    )?;
    check(
        &format!("flatpak install {id}"),
        runner.run(
            "flatpak",
            &["install", "--user", "-y", "--noninteractive", "flathub", id],
        )?,
    )?;
    Ok(())
}

pub fn update(runner: &dyn Runner, id: &str) -> Result<()> {
    check(
        &format!("flatpak update {id}"),
        runner.run(
            "flatpak",
            &["update", "--user", "-y", "--noninteractive", id],
        )?,
    )?;
    Ok(())
}

/// `flatpak uninstall --user`; with `purge`, the app's data in `~/.var/app/<id>` too.
pub fn remove(runner: &dyn Runner, id: &str, purge: bool) -> Result<()> {
    let mut args = vec!["uninstall", "--user", "-y", "--noninteractive"];
    if purge {
        args.push("--delete-data");
    }
    args.push(id);
    check(
        &format!("flatpak uninstall {id}"),
        runner.run("flatpak", &args)?,
    )?;
    Ok(())
}

/// The `Version:` line of `flatpak info`, when the app answers.
pub fn version(runner: &dyn Runner, id: &str) -> Option<String> {
    let out = runner.run("flatpak", &["info", "--user", id]).ok()?;
    if !out.ok {
        return None;
    }
    out.stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("Version:"))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    /// Records every argv; answers ok, with canned stdout for `info`.
    #[derive(Default)]
    pub struct FakeRunner {
        pub calls: Mutex<Vec<Vec<String>>>,
        pub info_version: Option<String>,
        pub fail_install: bool,
    }

    impl Runner for FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
            let mut argv = vec![program.to_string()];
            argv.extend(args.iter().map(|s| s.to_string()));
            self.calls.lock().unwrap().push(argv);
            if args.first() == Some(&"install") && self.fail_install {
                return Ok(Output {
                    ok: false,
                    stdout: String::new(),
                    stderr: "error: No remote refs found".into(),
                });
            }
            let stdout = if args.first() == Some(&"info") {
                self.info_version
                    .as_ref()
                    .map(|v| format!("Name: X\nVersion: {v}\n"))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            Ok(Output {
                ok: true,
                stdout,
                stderr: String::new(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeRunner;
    use super::*;

    #[test]
    fn install_adds_the_remote_then_installs() {
        let r = FakeRunner::default();
        install(&r, "net.pcsx2.PCSX2").unwrap();
        let calls = r.calls.lock().unwrap();
        assert_eq!(calls[0][1..4], ["remote-add", "--user", "--if-not-exists"]);
        assert_eq!(
            calls[1],
            [
                "flatpak",
                "install",
                "--user",
                "-y",
                "--noninteractive",
                "flathub",
                "net.pcsx2.PCSX2"
            ]
        );
    }

    #[test]
    fn a_purge_deletes_the_data_too() {
        let r = FakeRunner::default();
        remove(&r, "info.cemu.Cemu", false).unwrap();
        remove(&r, "info.cemu.Cemu", true).unwrap();
        let calls = r.calls.lock().unwrap();
        assert_eq!(
            calls[0],
            [
                "flatpak",
                "uninstall",
                "--user",
                "-y",
                "--noninteractive",
                "info.cemu.Cemu"
            ]
        );
        assert_eq!(
            calls[1],
            [
                "flatpak",
                "uninstall",
                "--user",
                "-y",
                "--noninteractive",
                "--delete-data",
                "info.cemu.Cemu"
            ]
        );
    }

    #[test]
    fn failure_surfaces_the_last_stderr_line() {
        let r = FakeRunner {
            fail_install: true,
            ..Default::default()
        };
        let e = install(&r, "x").unwrap_err();
        assert!(e.to_string().contains("No remote refs"));
    }

    #[test]
    fn version_parses_info() {
        let r = FakeRunner {
            info_version: Some("2.8.2".into()),
            ..Default::default()
        };
        assert_eq!(version(&r, "x").as_deref(), Some("2.8.2"));
        assert_eq!(version(&FakeRunner::default(), "x"), None);
    }
}
