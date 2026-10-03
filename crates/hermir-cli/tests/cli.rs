//! The CLI as a script sees it: exit codes, and exactly one JSON document per run.
use std::path::Path;

use assert_cmd::Command;

/// `hermir` with a prefix and a home of its own, so nothing on this machine is detected.
fn hermir(tmp: &Path) -> Command {
    let mut c = Command::cargo_bin("hermir").unwrap();
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    c.env("HERMIR_PREFIX", tmp.join("prefix"))
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("APPDATA", &home)
        .env("LOCALAPPDATA", &home)
        .env("PATH", tmp.join("no-bin"))
        .env_remove("GITHUB_TOKEN");
    c
}

/// Runs and returns the exit code and stdout parsed as one JSON document.
fn json(tmp: &Path, args: &[&str]) -> (i32, serde_json::Value) {
    let out = hermir(tmp).arg("--json").args(args).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let doc = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("{args:?}: stdout is not one JSON document ({e}): {stdout}"));
    (out.status.code().unwrap(), doc)
}

#[test]
fn every_kind_of_failure_has_its_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let (code, doc) = json(t, &["install", "nosuch"]);
    assert_eq!(code, 2);
    assert!(doc["error"].as_str().unwrap().contains("nosuch"));
    // Switch emulators are never installed: policy.
    assert_eq!(json(t, &["install", "ryujinx"]).0, 2);
    // Nothing installed to act on.
    assert_eq!(json(t, &["where", "pcsx2"]).0, 9);
    assert_eq!(json(t, &["config", "apply", "pcsx2", "--scale", "2"]).0, 9);
    assert_eq!(json(t, &["prepare", "pcsx2"]).0, 9);
    // `check` refuses what `install` refuses, and leaves nothing behind.
    assert_eq!(json(t, &["check", "nosuch"]).0, 2);
    assert_eq!(json(t, &["check", "ryujinx"]).0, 2);
    // An impossible request.
    assert_eq!(json(t, &["config", "apply", "pcsx2"]).0, 2);
    assert_eq!(
        json(t, &["config", "apply", "pcsx2", "--pad", "054c:0ce6"]).0,
        2,
        "a pad that is not the Xbox one needs its name"
    );
}

#[test]
fn usage_errors_are_one_json_document_too() {
    let tmp = tempfile::tempdir().unwrap();
    let (code, doc) = json(
        tmp.path(),
        &["players", "pcsx2", "--revert", "--pad", "045e:028e"],
    );
    assert_eq!(code, 2);
    assert!(doc["error"].is_string());
    let (code, _) = json(tmp.path(), &["no-such-verb"]);
    assert_eq!(code, 2);
}

#[test]
fn a_flag_with_an_optional_value_does_not_swallow_the_emulator() {
    let tmp = tempfile::tempdir().unwrap();
    // Parsed: `pcsx2` is the emulator, so the run gets as far as "not on this machine".
    assert_eq!(
        json(tmp.path(), &["config", "apply", "--fullscreen", "pcsx2"]).0,
        9
    );
    assert_eq!(
        json(tmp.path(), &["config", "apply", "--vsync=no", "pcsx2"]).0,
        9
    );
}

#[test]
fn reading_never_creates_the_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let (code, doc) = json(tmp.path(), &["status"]);
    assert_eq!(code, 0);
    assert!(doc.is_array());
    assert_eq!(json(tmp.path(), &["catalog", "list"]).0, 0);
    assert_eq!(json(tmp.path(), &["detect"]).0, 0);
    let (code, doc) = json(tmp.path(), &["doctor"]);
    assert_eq!(code, 0);
    assert_eq!(doc["prefix_writable"], true);
    assert!(!tmp.path().join("prefix").exists());
}

#[test]
fn the_catalog_validates_and_reverting_nothing_is_fine() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../hermir/catalog");
    let (code, doc) = json(tmp.path(), &["catalog", "validate", dir.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(doc["entries"].as_u64().unwrap() >= 20);
    let (code, doc) = json(tmp.path(), &["config", "revert", "--all"]);
    assert_eq!(code, 0);
    assert_eq!(doc, serde_json::json!([]));
    let (code, doc) = json(tmp.path(), &["config", "support", "pcsx2"]);
    assert_eq!(code, 0);
    assert_eq!(doc[0]["emulator"], "pcsx2");
}

#[test]
#[cfg(not(feature = "enumerate"))]
fn without_sdl_listing_pads_says_how_to_name_them_instead() {
    let tmp = tempfile::tempdir().unwrap();
    let (code, doc) = json(tmp.path(), &["pads"]);
    assert_eq!(code, 2);
    assert!(doc["error"].as_str().unwrap().contains("--pad VID:PID"));
    assert_eq!(
        json(tmp.path(), &["players", "pcsx2", "--pad", "auto"]).0,
        2
    );
}

/// SDL's own signal handlers would turn SIGTERM into an event nobody reads.
#[test]
#[cfg(all(unix, feature = "enumerate"))]
fn watching_pads_ends_on_sigterm() {
    let tmp = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("hermir"))
        .env("HOME", tmp.path())
        .args(["--json", "pads", "--watch"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let killed = std::process::Command::new("kill")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(killed.success());
    for _ in 0..50 {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    child.kill().unwrap();
    panic!("hermir pads --watch kept running after SIGTERM");
}
