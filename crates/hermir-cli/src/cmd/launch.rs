//! `hermir launch` and `run`: the command that starts a game, printed or run.
use hermir::{Error, Hermir, LaunchRequest, LaunchSpec, Result};

use super::Outcome;
use crate::cli::LaunchArgs;

fn spec(h: &Hermir, a: &LaunchArgs) -> Result<LaunchSpec> {
    let e = h.emulator(&a.emulator)?;
    let copy = e
        .best()?
        .ok_or_else(|| Error::NotInstalled(a.emulator.clone()))?;
    let req = LaunchRequest {
        file: a.file.clone(),
        platform: a.platform.clone(),
        fullscreen: a.fullscreen,
        core: a.core.clone(),
        patch: None,
    };
    e.launch(&copy, &req)
}

/// The command a shell would read the same way: what a person copies, never what runs.
fn shell_line(spec: &LaunchSpec) -> String {
    let quote = |s: &str| {
        if !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:=+,@%".contains(c))
        {
            s.to_string()
        } else {
            format!("'{}'", s.replace('\'', "'\\''"))
        }
    };
    let env = spec.env.iter().map(|(k, v)| format!("{k}={} ", quote(v)));
    env.chain(std::iter::once(
        spec.argv()
            .iter()
            .map(|a| quote(a))
            .collect::<Vec<_>>()
            .join(" "),
    ))
    .collect()
}

pub fn launch(h: &Hermir, a: &LaunchArgs) -> Result<Outcome> {
    let spec = spec(h, a)?;
    let line = shell_line(&spec);
    Ok(Outcome::new(&spec, line))
}

/// Runs the command with the terminal's stdin, stdout and stderr, and exits as it did.
pub fn run(h: &Hermir, a: &LaunchArgs) -> Result<Outcome> {
    let spec = spec(h, a)?;
    let argv = spec.argv();
    let (program, args) = argv.split_first().expect("a command has a program");
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).envs(&spec.env);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    let status = cmd.status().map_err(|e| Error::Io {
        op: "run",
        path: program.into(),
        source: e,
    })?;
    let code = status.code().unwrap_or_else(|| signal(&status));
    let mut out = Outcome::new(
        &serde_json::json!({ "launched": spec, "exit": code }),
        String::new(),
    );
    out.exit = code;
    Ok(out)
}

/// A child killed by a signal exits the way a shell reports it: 128 + the signal.
#[cfg(unix)]
fn signal(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map_or(1, |s| 128 + s)
}

#[cfg(not(unix))]
fn signal(_: &std::process::ExitStatus) -> i32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermir::Exe;

    #[test]
    fn the_printed_line_quotes_what_a_shell_would_split() {
        let spec = LaunchSpec {
            exe: Exe::FlatpakRun("net.pcsx2.PCSX2".into()),
            args: vec!["--".into(), "/roms/ps2/It's a Game.iso".into()],
            env: [("PULSE_SINK".to_string(), "hdmi out".to_string())].into(),
            cwd: None,
            sandbox: vec!["--filesystem=/roms/ps2".into()],
        };
        assert_eq!(
            shell_line(&spec),
            "PULSE_SINK='hdmi out' flatpak run --filesystem=/roms/ps2 net.pcsx2.PCSX2 -- \
             '/roms/ps2/It'\\''s a Game.iso'"
        );
    }
}
