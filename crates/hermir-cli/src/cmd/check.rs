//! `hermir check <emulator>`: an emulator set up, configured and run on this machine, start to
//! finish, without touching the player's own copy or settings. The thing a contributor runs on
//! a real box before saying a catalog entry works (docs/design.md §8, "on glass").
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hermir::{
    Aspect, Exe, Hermir, Install, KnobValue, LaunchRequest, LaunchSpec, Options, Os, Patch,
    RealEnv, Result, StepOutcome, Support, Video,
};
use serde::Serialize;

use super::Outcome;
use crate::render::Stderr;

#[derive(Serialize)]
struct Step {
    step: &'static str,
    ok: bool,
    note: String,
}

/// What `check` writes and reads: fullscreen, 3×, vsync, 16:9.
fn session() -> Patch {
    Patch {
        video: Some(Video {
            fullscreen: Some(true),
            scale: Some(3),
            vsync: Some(true),
            aspect: Some(Aspect::SixteenNine),
        }),
        ..Default::default()
    }
}

/// The check's directory, gone when the check is over however it ended, unless kept.
struct Scratch {
    dir: PathBuf,
    keep: bool,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

pub fn check(h: &Hermir, emulator: &str, seconds: u64, keep: bool) -> Result<Outcome> {
    let os = h.os();
    let scratch = Scratch {
        dir: std::env::temp_dir().join(format!("hermir-check-{emulator}-{}", std::process::id())),
        keep,
    };
    let tmp = &scratch.dir;
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).map_err(|e| hermir::Error::Io {
        op: "create",
        path: home.clone(),
        source: e,
    })?;
    // A prefix and a home of its own: the copy, its settings and its saves are all in there.
    let env = RealEnv::with_home(os, home.clone());
    let vars = env.home_vars();
    let hc = Hermir::open(Options {
        prefix: Some(tmp.join("prefix")),
        os: Some(os),
        env: Some(Box::new(env)),
        ..Options::default()
    })?;
    let e = hc.emulator(emulator)?;
    let mut steps = Vec::new();
    let mut push = |step, ok, note: String| steps.push(Step { step, ok, note });

    let row = e.install(&Stderr)?;
    let installed = match &row.exe {
        Exe::FlatpakRun(id) => format!(
            "{id} from Flathub, into the user installation, where it stays; its settings go to \
             the check's own home"
        ),
        Exe::Path(p) => format!("{} (a throwaway prefix)", p.display()),
        other => other.to_string(),
    };
    push("install", true, installed);
    let copy = e
        .copies()?
        .into_iter()
        .find(|i| i.exe == row.exe)
        .ok_or_else(|| hermir::Error::NotInstalled(emulator.into()))?;

    let spec = e.launch(&copy, &LaunchRequest::default())?;
    let (ok, note) = run_for(&spec, &vars, os, seconds);
    push("first start", ok, note);

    let prepared = e.prepare(&copy, None, &[])?;
    let failed: Vec<String> = prepared
        .steps
        .iter()
        .filter(|s| s.outcome == StepOutcome::Failed)
        .map(|s| {
            format!(
                "{}: {}",
                s.target.display(),
                s.note.as_deref().unwrap_or("")
            )
        })
        .collect();
    push(
        "prepare",
        failed.is_empty(),
        if failed.is_empty() {
            format!("{} step(s)", prepared.steps.len())
        } else {
            failed.join("; ")
        },
    );

    let patch = session();
    let applied = e.apply(&copy, &patch)?;
    let marks: Vec<String> = applied
        .knobs
        .iter()
        .map(|k| format!("{} {}", crate::render::support_mark(k.support), k.knob))
        .collect();
    push("apply", !applied.failed(), marks.join(", "));

    let (ok, note) = run_for(&spec_again(&e, &copy)?, &vars, os, seconds);
    push("start on the patched settings", ok, note);

    let read = e.get(&copy)?;
    let wrong = read_back(&applied.knobs, &read);
    push(
        "read back",
        wrong.is_empty(),
        if wrong.is_empty() {
            "every applied knob reads as written".into()
        } else {
            wrong.join("; ")
        },
    );

    let reverted = e.revert(false)?;
    let conflicts = reverted
        .iter()
        .filter(|s| s.outcome == StepOutcome::Conflict)
        .count();
    let failed = reverted.iter().any(|s| s.outcome == StepOutcome::Failed);
    push(
        "revert",
        !failed,
        if conflicts > 0 {
            format!(
                "{conflicts} file(s) the emulator rewrote while running were left as it wrote them"
            )
        } else {
            format!("{} file(s) restored", reverted.len())
        },
    );

    let ok = steps.iter().all(|s| s.ok);
    let human = steps
        .iter()
        .map(|s| {
            format!(
                "{} {:<30} {}",
                if s.ok { "ok  " } else { "FAIL" },
                s.step,
                s.note
            )
        })
        .chain(keep.then(|| format!("kept: {}", tmp.display())))
        .collect::<Vec<_>>()
        .join("\n");
    let out = Outcome::new(
        &serde_json::json!({ "emulator": emulator, "ok": ok, "steps": steps,
                             "kept": keep.then_some(&tmp) }),
        human,
    );
    Ok(if ok {
        out
    } else {
        Outcome {
            exit: 1,
            note: Some(format!("{emulator}: the check failed")),
            ..out
        }
    })
}

fn spec_again(e: &hermir::EmulatorHandle<'_>, copy: &Install) -> Result<LaunchSpec> {
    e.launch(copy, &LaunchRequest::default())
}

/// Every knob `apply` wrote, as `get` reads it back.
fn read_back(applied: &[hermir::KnobChange], read: &[KnobValue]) -> Vec<String> {
    let want = |knob: &str| match knob {
        "video.fullscreen" | "video.vsync" => Some("true"),
        "video.scale" => Some("3"),
        "video.aspect" => Some("16:9"),
        _ => None,
    };
    applied
        .iter()
        .filter(|k| k.support == Support::Applied && k.file.is_some())
        .filter_map(|k| {
            let w = want(&k.knob)?;
            let got = read.iter().find(|r| r.knob == k.knob)?;
            (got.value.as_deref() != Some(w))
                .then(|| format!("{} reads {:?}, {w} was written", k.knob, got.value))
        })
        .collect()
}

/// Starts the command and lets it run `seconds`: still running then is a good start. It runs
/// with the check's home (`vars`); a Flatpak keeps its installation where it is.
fn run_for(spec: &LaunchSpec, vars: &[(&str, PathBuf)], os: Os, seconds: u64) -> (bool, String) {
    let argv = spec.argv();
    let Some((program, args)) = argv.split_first() else {
        return (false, "nothing to run".into());
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .envs(&spec.env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if os == Os::Linux {
        let installation = std::env::var_os("FLATPAK_USER_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let data = std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share"))
                    });
                data.map(|d| d.join("flatpak"))
            });
        if let Some(dir) = installation {
            cmd.env("FLATPAK_USER_DIR", dir);
        }
    }
    cmd.envs(vars.iter().map(|(k, v)| (k, v)));
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (false, format!("{program}: {e}")),
    };
    let until = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < until {
        match child.try_wait() {
            Ok(Some(status)) => {
                return (
                    false,
                    format!("it quit by itself within {seconds}s ({status})"),
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return (false, e.to_string()),
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    (true, format!("still running after {seconds}s"))
}
