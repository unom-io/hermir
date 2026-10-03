//! Developer tasks that are not part of the shipped CLI.
//!
//! ```text
//! cargo xtask capture <emulator>… | --all [--host] [--settle <s>] [--force]
//! ```
//!
//! `capture` starts each emulator once, headless, with a fresh home, and turns what it wrote into
//! a fixture under `fixtures/` (see `fixtures/README.md`). By default it runs in the capture image
//! (`ci/capture/Dockerfile`, built on first use, run `--privileged`); `--host` runs the same script
//! on this machine, which needs `flatpak`, `xvfb-run` and `dbus-run-session`.
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod capture;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("capture") => capture::main(&args[1..]),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str =
    "usage: cargo xtask capture <emulator>… | --all [--host] [--settle <s>] [--force]";

/// The workspace root.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root exists")
}

/// Today as `YYYY-MM-DD`, in UTC.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Howard Hinnant's days-to-civil.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}
