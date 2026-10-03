//! One test per fixture and session: `pcsx2/2.8.2/linux/video-all`. `HERMIR_BLESS=1` rewrites
//! the expectations instead of checking them.
use std::path::Path;

use hermir_golden::{check, fixture};
use libtest_mimic::{Arguments, Failed, Trial};

fn main() {
    let args = Arguments::from_args();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let bless = std::env::var_os("HERMIR_BLESS").is_some_and(|v| !v.is_empty() && v != "0");
    let fixtures = match fixture::discover(&root) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("fixtures: {e}");
            std::process::exit(1);
        }
    };
    let mut trials = Vec::new();
    for fx in fixtures {
        for session in fx.sessions.clone() {
            let fx = fx.clone();
            let name = format!("{}/{session}", fx.name());
            trials.push(Trial::test(name, move || {
                check::run(&fx, &session, bless).map_err(Failed::from)
            }));
        }
    }
    libtest_mimic::run(&args, trials).exit();
}
