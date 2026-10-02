# hermir — implementation plan

| | |
|---|---|
| **Status** | Proposed |
| **Date** | 2026-10-02 |
| **Scope** | From today's tree (P0, `prepare`, `apply`/`revert`/`support`) to the rest of [`design.md`](design.md) §10: hardening for 0.1.0, the test setup, `launch`, profiles, pads, saves, `config get` and audio, the catalog's gaps |
| **Inputs** | A review of the whole tree on 2026-10-02, and a capture probe the same day (§2.5) |

This plan orders the work and says, for each step, what changes where, how it is tested, and when it is
done. The design stays the reference for *what* hermir is; this is *how we get there*. Sizes are
relative: **S** about a day, **M** a few days, **L** one to two weeks.

## 0. Order

| # | Milestone | Needs | Size | Ships in |
|---|---|---|---|---|
| T0 | Unit and integration tests for the risky paths (fake release server, hostile archives, property tests, CLI) | — | M | 0.1.0 |
| T1 | Golden fixtures in `cargo test`: real config files, every PR, every OS | T2a | M | 0.1.0 |
| T2a | Capture in Docker: an emulator writes its own defaults, headless | — | M | 0.1.0 |
| M0 | Hardening: the review's findings, fixed and tested | T0, T1 | L | 0.1.0 |
| T2b | Weekly drift job: re-capture current releases, check keys, smoke-launch | T2a | M | 0.2.0 |
| M1 | `launch` → `LaunchSpec`, `run`, launch-only knobs, the `Adapter` trait | M0 | M | 0.2.0 |
| M2 | Profiles: an isolated config root where the emulator has a flag | M1 | M–L | 0.2.0 |
| T3 | Human capture of `after/` files, and `hermir check` | T1; M1 for `check`; M3 for pads | M | 0.2.0 → 0.3.0 |
| M3 | Pads: `enumerate` (SDL3), `hermir pads`, per-name and per-GUID ordinals | M0 | L | 0.3.0 |
| M4 | Saves view | M5 `get` for paths set in settings | M | 0.3.0 |
| M5 | `config get`, audio knobs | M1 for the env fallback | M | 0.3.0 |
| M6 | Catalog gaps: detection, channels, firmware, settings | M0 (release id) for rolling releases | S each, ongoing | any |

Why this order:

- **Tests first.** Every fix in M0 and every later milestone needs real files to be checked against. T2a makes those files in
  about 25 seconds per emulator (§2.5), so T1 can land with real fixtures, not hand-written strings.
- **Launch before profiles.** A profile is mostly a launch flag (`-u`, `--config`, `--appendconfig`).
- **Pads after launch.** Enumeration matters most just before a launch: SDL's index is the order at that moment.
- **Catalog work runs alongside.** Each gap is a small data PR, and each one lands with a fixture where it can be captured.

The first four PRs, in order:

1. **golden v1** — T2a's capture image and script, T1's harness, and fixtures for PCSX2, DuckStation and
   Dolphin on Linux (re-captured; the probe's files were in a scratch directory).
2. **hardening: downloads and store** — M0.1, with T0's fake server and archive tests.
3. **hardening: config writes** — M0.2, checked against golden v1.
4. **API and CLI for 0.1.0** — M0.3, then tag `v0.1.0`.

## 1. M0 — hardening for 0.1.0

Each item names its place in the tree and the test that pins it. ● blocks 0.1.0; ○ can follow in 0.1.x.

### 1.1 Downloads and the store

- ● **Timeouts.** `channel/http.rs` builds a ureq agent with no timeouts (ureq 3 leaves them unset by
  default), so a stalled connection hangs forever while holding the prefix lock. Set resolve, connect, send
  and response-header timeouts (30 s each). ureq's `timeout_recv_body` is a *total* budget, not an idle one.
  So for the body, set it from the `Content-Length` (for example, a floor of 10 min plus size ÷ 50 KB/s).
  Downloads resume, so a timeout costs a retry, not the bytes already fetched.
  *Test:* the T0 fake server stalls mid-body, and the download fails within the budget, then resumes.
- ● **System trust store, https only.** ureq trusts only its bundled Mozilla roots. Behind a TLS-inspecting
  proxy or a Windows antivirus that scans HTTPS, every request fails with `UnknownIssuer` (reproduced on
  2026-10-02). Enable ureq's `platform-verifier` feature and `https_only(true)`.
  *Test:* the fake server redirects to `http://`, and the download is refused.
- ● **Say what was verified.** The three `url` channels (Dolphin, RetroArch, ScummVM) carry no `sha256`,
  libretro cores are never checked, and a GitHub asset without a `digest` is accepted. Yet
  `installed.json` stores the computed hash as `digest` either way (`channel::install`), so verified and
  unverified installs look alike. Changes:
  - `Installed` gets `sha256` (what arrived) and `verified: Published | Pinned | None`.
  - A `url` entry must pin `sha256`: the entry already pins the version, so the checksum is bumped in the
    same reviewed PR, and catalog validation rejects a `url` channel without one.
  - The CLI prints `not verified: <why>` instead of "verifying".
  - Hosts like punktfunk get an `Options::require_verified`.
  - Fix the README and CHANGELOG claims to match.
- ● **Folder-style portable markers.** `channel::install` creates `Portable::Dir` in the extracted tree,
  but `Store::place` moves only files and then prunes empty folders. Cemu's `portable/` and Azahar's
  `user/` (and PPSSPP's `memstick/` and shadPS4's `user/`, unless their archives ship the folder) never
  arrive, so the copy runs non-portable while hermir patches a root it never reads. Create the marker after
  `place`, as the firmware folder already is.
  *Test:* install a fake Cemu archive, and `app/portable/` exists.
- ● **Never follow symlinks in the store.** `store::walk` and `prune_empty_dirs` use `is_dir()`, which
  follows links. If a user links a ROM folder into `app/`, every update or remove empties the folders inside
  it. And a tar archive with a link to outside makes `place` move outside files in. Changes:
  - Use `symlink_metadata` and never descend into a link.
  - In `extract::untar`, refuse a link entry whose target leaves the tree, and refuse hard links.
  - In `unzip`, keep only the executable bit of a mode.
  *Tests:* hostile tar/zip/7z fixtures in T0, and a linked folder that survives an update.
- ● **A remove without `--purge` forgets edited files.** `Store::remove_files` deletes the manifest, so the
  next install overwrites the very files the remove kept. Keep the manifest rows of kept files. Also treat a
  file that is present but absent from the old manifest as the user's: the new one lands beside it as `.new`.
  *Test:* edit, remove, reinstall, and the edit stays.
- ○ **A `place` that stops halfway.** `place` deletes, moves, then writes the manifest with a plain
  `fs::write`. Instead:
  1. write the new manifest first as `manifest.next.json`;
  2. move the files;
  3. delete what the new release dropped;
  4. rename the manifest into place.

  A leftover `manifest.next.json` means the next run finishes the job by hashing. On Windows, a running
  emulator's locked exe fails the move: say "close <emulator> and retry", not a bare I/O error.
- ○ **The release id is the publish date** (`github.rs`). Two builds on the same day look identical, which
  matters for rolling assets like Vita3K's `windows-latest.zip`. Use the asset id plus its `updated_at` or
  digest. Name the `.part` after the id and size, send `If-Range`, and check `Content-Range` before
  appending. A 416 counts as complete only when the part's size equals the asset size.
- ○ **Smaller fixes:**
  - `remove --purge` on a Flatpak runs `flatpak uninstall --delete-data`.
  - `Placed.kept_modified` reaches `Installed` and the CLI ("3 files you edited were kept; the new ones are
    beside them as .new").
  - `lib.rs` `update`/`remove` read the row after taking the lock.
  - `store::sha256_of` and `http::sha256_file` become one function.

### 1.2 Config writes

- ● **Validate and escape every value.** Values, keys and sections are spliced into files raw. A newline in a
  pad name or a `--set` value adds keys or sections (reproduced on Dolphin's `GCPadNew.ini` and
  `retroarch.cfg`). A `"` breaks RPCS3's YAML, and `&` or `<` breaks Cemu's XML profile. Two layers:
  - **At the boundary:** `Patch::validate()` refuses control characters in `PadRef.name`, `Native` keys,
    sections and values, and the CLI's parsers call it.
  - **In each format:** XML text is entity-escaped. A JSON value must parse as one JSON scalar
    (`serde_json`). YAML strings are quoted when they need to be. Qt values containing `,` or `\` are
    quoted the way QSettings does.

  *Tests:* T0 properties — no value ever adds a key — and T1 parses every output with an independent parser.
- ● **The JSON editor reads one key per line** (`config/json.rs`). On compact JSON (`{"GPU": {"full_screen":
  false}}`) it appends text after the closing brace and reports Applied. Replace the line scan with a
  byte scanner that tracks strings, escapes and depth and yields `(key, value span, depth)`. Inserts take
  the indentation of their siblings.
  *Tests:* compact, pretty and mixed files; T1 parses the output with `serde_json`.
- ● **Report a knob after its write, not before.** `knobs::plan` decides `Support` before
  `txn::apply_edits` runs, so a knob whose file step failed still reads Applied. Join each knob to its
  file's step, and turn a failed step into `Failed` with the reason. (`Support` gains `Failed`; see 1.3 on
  `#[non_exhaustive]`.)
- ● **Seats.** `eden.rs` and `cemu.rs` compute `u32::from(seat) - 1`: seat 0 panics in debug builds and wraps
  to `player_4294967295` in release. Elsewhere seat 0 writes `[Pad0]`, and a duplicate seat silently wins.
  `Patch::validate()` requires seats to be unique, at least 1, and at most the emulator's port count. Seats
  past the ports stay a note, as today.
- ● **Lock every write.** `apply`, `revert`, `revert_all` and `prepare` write config, snapshots and firmware
  without `Store::lock`. Two concurrent applies can both snapshot hermir's own text as "the player's". The
  facade takes the lock once per call.
  *Test:* a second apply while the lock is held fails with exit code 8.
- ● **`write_atomic` keeps what it replaces** (`prepare.rs`; it moves to `config/txn.rs`, the layer that
  owns writes). Today:
  - it replaces a symlinked config with a regular file;
  - it resets 0600 to the default permissions (RetroArch's config holds a RetroAchievements password);
  - it never fsyncs;
  - its temp name (`with_extension`) collides for same-stem files;
  - on Windows the rename fails while the emulator has the file open.

  Instead: resolve the link and write to its target; copy the original's permissions; use a unique temp
  file in the same directory; fsync the file, then the directory; retry a Windows sharing violation briefly,
  then fail with "close <emulator>".
- ○ **Revert only what hermir changed, and don't clobber later edits.** Today a file is snapshotted before
  anyone knows whether it will change, and revert restores it even if the player edited it since. Instead:
  - Snapshot only the files whose text actually changes.
  - Record the hash of the text hermir wrote. On revert, a file whose current hash differs is reported as
    `Conflict`, with the snapshot kept, unless `--force`.
  - Record folders that apply created, and remove them on revert if empty (RPCS3's
    `input_configs/global`).
  - Manifest keys are raw `OsString` bytes, not `to_string_lossy`.
- ○ **Format edge cases**, each with a T0 case and, once one shows up in a real file, a T1 fixture:
  - **YAML:** match a section only at its own indent; allow a header with a trailing comment or `{}`; never
    insert into a key's block list.
  - **XML:** `find` takes a child, not any descendant.
  - **INI:** don't treat a TOML array line (`  [1, 2]`) as a section; keep inline comments. Add a
    `case_insensitive` flag on `ConfigFile` for emulators that read keys that way (Dolphin, SimpleIni-based
    ones).
  - **`prepare`'s first-run INI step** ignores a BOM, so it appends a second section and overrides a value
    the user set. Strip the BOM before `ini::get`, as `txn` does.

### 1.3 Library API and CLI

- ● **`Hermir` is neither `Send` nor `Sync`.** `Http`, `flatpak::Runner` and `detect::Env` lack the bounds,
  so a tokio host can't use `spawn_blocking` or an `Arc` (design D10). Add `Send + Sync` to the three
  traits, plus a compile-time assertion test.
- ● **Freeze the surface before publishing.**
  - Public enums that will grow get `#[non_exhaustive]`: `Error`, `InstallKind`, `Support`,
    `StepOutcome`, `Exe`, `Channel`, `Format`.
  - Structs a consumer builds (`Patch`, `Video`, `Player`, `PadRef`, `Install`) stay exhaustive. Marking
    them would forbid `..Default::default()` outside the crate; adding a field is an accepted 0.x minor
    break.
  - Add `Install::new(emulator, kind, exe, config_root)`, so golden tests and hosts build one without
    naming every field.
  - Make internal modules private (`channel`, `store`, `detect`, the format editors) and re-export what
    consumers need from the root, including the traits a host may implement (`Http`, `Runner`, `Env`).
  - Turn on `#![warn(missing_docs)]` (206 items today; 123 in `model.rs`).
  - Remove `apply_players`, `revert_players` and `revert_all_players`; nothing published depends on them.
- ● **Exit codes and errors as the design says** (§2.4):
  - Add `Error::NotInstalled`, `Error::Invalid` (usage) and `Error::Config` (7).
  - `Error::Place` stops being a catch-all.
  - `Error::exit_code()` lives in the library, so hosts and the CLI agree.
  - Today usage errors and "not on this machine" exit 5 ("extract or place"), a network failure in `catalog
    resolve` exits 2, and 7 is never used.
- ● **`--json` is one document per run.**
  - `catalog resolve` prints its rows *and* an error object on failure. Errors belong inside the one
    document.
  - `update --all` stops at the first error and reports nothing; it should go on, and report every row with
    its outcome.
  - Output uses the library types (`Applied`, `Prepared`, `Install`), not ad-hoc `json!` objects (D13).
- ● **CLI parsing:**
  - `config apply --fullscreen pcsx2` fails because the optional bool swallows the emulator name; use
    `require_equals`.
  - `--pad` takes an explicit index and version (`vid:pid[:name][@index]`); today seat N always gets index
    N−1, and every pad gets the Xbox 360 GUID's version.
  - `players --revert` conflicts with `--pad`.
  - "No copies" exits the same way for `players`, `prepare` and `config apply`.
- ○ **Split `hermir-cli/src/main.rs`.** `run()` is about 590 lines, with step rendering copied three times and
  `process::exit(5)` four times. Split it into:
  - `cli.rs` (clap types);
  - `parse.rs` (pads, `--set`, natives — with unit tests);
  - `render.rs`;
  - `cmd/<verb>.rs`;
  - an `Outcome { exit, json }` that `main` turns into an exit code.
- ○ **Read-only verbs don't create the prefix** (`Store::open` lazily), so `doctor` can report a prefix it
  cannot create.
- ○ **Detection:**
  - Deduplicate by canonical path; today `pcsx2-qt` and `pcsx2` on `PATH` give two rows.
  - Honour `FLATPAK_USER_DIR`; the probe had to symlink around it.
  - Make `Env` injectable through `Options`, for T1 and the detection tests.
  - Fix `expand`'s `%A%B%` and `~user` cases.
  - `status --check` reports a resolve error on its row instead of "no update".
- ○ **Docs and CI:**
  - The design's §2.2 model and §3 example match the code (today the example fails strict parsing).
  - The CHANGELOG counts 23 entries.
  - `cargo-deny` in CI enforces the permissive-licence closure that commit 26f87cb keeps by hand, plus
    advisories.
  - The catalog dry run opens an issue when it fails (design §8), instead of only turning red.

**Done when:** every ● item is fixed with the test named above; the ○ items are fixed or filed as issues; 0.1.0
is tagged and published (`docs/publishing.md`).

## 2. The test setup

Four layers. The first two run on every PR without Docker or emulators. The other two are where real
emulators come in.

| Layer | Proves | Runs | When | Needs |
|---|---|---|---|---|
| T0 unit and integration | the risky code paths behave | `cargo test`, all OSes | every PR | nothing |
| T1 golden | hermir edits *real* files correctly, and reverts them byte for byte | `cargo test`, all OSes | every PR | checked-in fixtures |
| T2 capture and drift | the files are what current releases write; hermir's keys still exist there; a patched file still starts the emulator | Docker on Linux; a Windows runner (to be proven) | on demand; weekly | network, `--privileged` |
| T3 human capture and `hermir check` | the keys are what the emulator itself writes when a person sets the same thing; pads land in the right seat | a contributor's real machine | once per emulator per major version | a person, and pads |

### 2.1 T0 — unit and integration tests

- **A fake release server** (`crates/hermir/tests/http.rs`): a `std::net::TcpListener` in a thread, with no new
  dependency. It serves 200, 206 with `Content-Range`, 416, a redirect to `http://`, a body that stalls,
  and an asset that changes between requests. Covered: resume, `If-Range`, timeouts, https-only, and a
  release replaced the same day.
- **Hostile archives:**
  - built in-test: zip, tar and 7z with `../`, absolute paths, a symlink to outside, a hard link, a single
    top-level symlink;
  - an AppImage-like tree with directory links;
  - every case is refused, or placed without leaving the tree.
- **The store:**
  - edit → remove → reinstall keeps the edit;
  - `Portable::Dir` arrives;
  - a linked folder in `app/` survives update and remove;
  - an interrupted `place` (a leftover `manifest.next.json`) is finished;
  - a corrupt `installed.json` or manifest is an error, not a panic.
- **Property tests for the editors** (`proptest`, a dev-dependency; MIT/Apache). For each format editor
  `set`:
  - `get` then returns the value;
  - every line it doesn't own is unchanged;
  - it adds at most the one key it was asked for;
  - the output parses with an independent parser;
  - through `txn`, apply then revert is byte-identical, with BOM and CRLF too.
- **CLI tests** (`assert_cmd`; MIT/Apache), with a temp prefix and fake `HOME`:
  - exit codes per error kind;
  - with `--json`, exactly one JSON document on stdout, failures included;
  - `--fullscreen pcsx2` parses.
- **Static assertions:** `Hermir: Send + Sync`.

### 2.2 T1 — golden fixtures

**Where.** `fixtures/` at the repository root (design §9), tested by a workspace crate that is never
published: `crates/hermir-golden` (`publish = false`). The fixtures never ship in the crates.io package, and
the harness can use only hermir's public API — which keeps that API testable.

```
fixtures/
  README.md                         what a fixture is, how to capture one, the masks
  pcsx2/
    2.8.2/
      linux/
        meta.json                   emulator, version, OS, install kind, source, capture date and method
        before/inis/PCSX2.ini       what the emulator wrote on its first start, masked
        sessions/
          video-all.json            a Patch, and the support each knob should report
          players-1-xbox.json
        hermir/video-all/inis/PCSX2.ini     hermir's output for that session (blessed)
        after/video-all/inis/PCSX2.ini      optional: what the emulator wrote when a person set the same
                                            things in its UI (T3)
      windows/
        …
```

**`meta.json`:**

```jsonc
{
  "emulator": "pcsx2", "version": "2.8.2", "os": "linux", "kind": "flatpak",
  "source": { "flatpak": "net.pcsx2.PCSX2", "commit": "<ostree commit>" },
  "captured": { "on": "2026-10-02", "by": "xtask capture" },   // or "hand" for synthetic edge cases
  "files": { "main": "inis/PCSX2.ini" },                      // the catalog's names → paths captured
  "oracle": { "main": "ini" },                                // ini | toml | yaml | xml | json | none
  "masks": [],                                                // values replaced at capture time
  "ignore_after": [],                                         // what a UI session also saves (window geometry)
  "full_writer": true,                                        // writes every setting, not only changed ones
  "created": ["inis/PCSX2.ini", "logs/emulog.txt"]            // everything the first start created (for §6)
}
```

**A session** is a `Patch` plus the support each knob should report:

```jsonc
{
  "patch": { "video": { "fullscreen": true, "scale": 3, "vsync": true, "aspect": "16:9" } },
  "expect": { "video.fullscreen": "applied", "video.scale": "applied", "video.vsync": "applied",
              "video.aspect": "applied" },
  "may_touch": []        // sections an adapter owns whole ("Pad1", "InputSources") for players sessions
}
```

The standard sessions:
- `video-all` (fullscreen, 3×, vsync, 16:9);
- `video-off` (windowed, 1×, no vsync, auto);
- `region-<r>`;
- `players-1-xbox`;
- `players-2-mixed` (an Xbox pad and a DualSense; after M3, with the `connected` list);
- `native-<key>`.

**The harness** (`crates/hermir-golden/tests/golden.rs`) uses `libtest-mimic` (MIT/Apache), so each
`(fixture, session)` is its own named test (`pcsx2/2.8.2/linux/video-all`). Each test:

1. copies `before/` into a temp root;
2. builds `Install::new(emulator, kind, …, root)` with the fixture's OS. `apply` takes the OS as a
   parameter, so **Windows fixtures run on Linux CI too**, and the reverse;
3. opens `Hermir` on a temp prefix and runs `emulator(id).apply(&install, &patch)`.

Then it checks seven things:

| | Check | Catches |
|---|---|---|
| A | each knob reports the support `expect` names | an unsupported knob reported as applied, and the reverse |
| B | the output equals `hermir/<session>/` byte for byte (`HERMIR_BLESS=1` rewrites it; the PR diff shows the change) | any change in what hermir writes |
| C | every output parses with an independent parser (`toml`, `yaml-rust2`, `roxmltree`, `serde_json`; MIT/Apache; a small INI oracle in the crate) | corrupt files: the compact-JSON and YAML bugs |
| D | the keys that changed from `before/` are exactly the keys the catalog binds for the session's knobs (with `also`, Qt's `\default`) plus `may_touch` sections | stray writes |
| E | if `after/<session>/` exists: every key the emulator changed (minus `ignore_after`) has hermir's value, and hermir changed nothing else | wrong keys, wrong spellings, missing companions — the real ground truth |
| F | `revert` gives back `before/` byte for byte, and no snapshot is outstanding | snapshot and restore bugs on real files |
| G | a second `apply` changes no file | non-idempotent edits |

D and E compare parsed values, not bytes, because emulators rewrite whole files while hermir edits lines.

**Synthetic fixtures** stay allowed for edge cases nobody has captured yet (BOM, CRLF, compact JSON). They say
`"by": "hand"`, so real and synthetic files are never confused.

**Retention:** the latest two captured versions per emulator per OS; T2b's PRs drop the older ones.

**Licence:** default config files are output of the emulators, not their code. `fixtures/README.md`
says where each came from, and a project that objects gets its fixtures removed.

### 2.3 T2 — capture in Docker, and the weekly drift job

**The tool.** An `xtask` crate (`publish = false`; `cargo xtask …`), so the shipped CLI stays free of
developer verbs:

```
cargo xtask capture <emu> [--os linux] [--source flatpak|release]     T2a: before/ + meta.json
cargo xtask capture <emu> --interactive --session video-all           T3:  after/<session>/
cargo xtask drift [<emu> | --all]                                     T2b
```

**The image** (`ci/capture/Dockerfile`):
- `ubuntu:24.04` with `flatpak`, `xvfb`, `xauth`, `dbus`, `dbus-x11`, `xdotool`, Mesa (llvmpipe and
  lavapipe for GL and Vulkan without a GPU) and `ca-certificates`;
- Flatpak installs go in a named volume (`hermir-flatpak`), so runtimes are fetched once.

**Run it `--privileged`.** Flatpak's sandbox (bubblewrap) can't create its namespaces in a default
container; it also failed with seccomp and AppArmor unconfined plus `SYS_ADMIN`. With `--privileged` it
works (§2.5). GitHub's Linux runners allow privileged containers. Flatpak straight on the runner is the
fallback if that ever changes.

**The capture script**, as the probe ran it:

```sh
# flatpak run needs a system bus
dbus-daemon --system --fork
# a fresh home; a short runtime dir, because unix socket paths are limited to 108 bytes
# (p11-kit's socket broke under a long one)
export HOME=$(mktemp -d) XDG_RUNTIME_DIR=/run/user/0
mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
# exit 124 means it was still running when stopped: it started
dbus-run-session -- xvfb-run -a -s "-screen 0 1280x720x24" \
  timeout -s TERM "$SETTLE" flatpak run "$APP"
# then: copy what the catalog's config.files name from roots.flatpak, apply the masks,
# list every created path into meta.created, write fixtures/<emu>/<version>/linux/
```

**Per-emulator recipes** (`ci/capture/recipes/<emu>.json`) hold:
- how long to let it settle (25 s was enough for all three probed);
- extra arguments;
- masks;
- `full_writer`;
- whether it writes settings only on a clean quit, and how to quit it (`xdotool key ctrl+q`);
- the checklists T3 shows a person.

**Release builds** (RPCS3, Xenia Canary, shadPS4, Supermodel on Linux) are captured by installing them
*with hermir* into a temp prefix and running from `app/`. That exercises the install path at the same time.
The config format is the same as the Flatpak's, so one capture per version is enough.

**The drift job** (`.github/workflows/golden-drift.yml`; weekly and `workflow_dispatch`; one matrix job per
emulator):

1. Capture the current release. If the version and the masked files equal the newest fixture, the job is
   green.
2. Otherwise:
   - **Keys:** for a `full_writer` emulator, every section and key the catalog binds must exist in the new
     defaults. This catches upstream renames, which D9 ("current stable line") makes the main way bindings
     rot.
   - **Golden:** run T1's harness on the new fixture (checks A–D, F, G).
   - **Launch smoke:** apply `video-all`, start the emulator for the settle time, and require that it is
     still running (exit 124), not crashed. If the emulator rewrites its settings on load, also require
     hermir's values to survive.

     The probe showed PCSX2 and DuckStation do *not* re-save on start or on kill, so for them "it
     survived" proves nothing. The smoke's value is catching a file hermir corrupted. Where it can prove
     more, the recipe says so.
3. Open a PR with the new fixture directory when everything passes, or an issue naming the failed check.

**Cost.** Measure first. In the probe, three emulators with their runtimes took 91 s and 4.3 GB, with the KDE
runtime shared. If the weekly run grows past about 20 minutes, cache the volume per runtime with
`actions/cache`.

**Windows (T2w): a spike before any promise.** Docker can't run GUI apps in Windows containers, so the
capture runs straight on `windows-latest`:
1. `hermir install <emu>` into a temp prefix (portable, so nothing outside it is touched);
2. start the exe with a timeout, then `Stop-Process`;
3. collect the portable files.

It is unproven whether each emulator writes its defaults with no GPU and no desktop interaction there.
Try PCSX2 and DuckStation first; where it fails, Windows fixtures come from T3.

**Virtual pads (spike, for M3):**
- Find out whether `sudo modprobe uinput` works on `ubuntu-latest`, and whether a privileged container gets
  `/dev/uinput`. (The machine the probe ran on had no uinput.)
- If so, the `evdev` crate (MIT/Apache) creates pads with exact identities (Xbox 360 `045e:028e`, DualSense
  `054c:0ce6`, name and version), and `hermir pads --json` must report the GUIDs and indices SDL reports.

### 2.4 T3 — human capture, and `hermir check`

**Interactive capture** (`cargo xtask capture <emu> --interactive --session <s>`), on a contributor's real
machine:
1. Install into an isolated home (Linux: a fresh `HOME` as in the probe; Windows: a portable copy in a temp
   prefix), so the contributor's own settings are never read or touched.
2. Start the emulator once to capture `before/`.
3. Print the session's checklist from the recipe, for example: "Settings → Graphics → Internal Resolution:
   3x Native. Settings → Interface → Start Fullscreen: on. … Quit."
4. Start the emulator, wait for it to exit, and snapshot `after/<session>/`.
5. Show the diff, apply masks, and write the fixture.

The PR template asks for this. It is CONTRIBUTING.md's "fixture in the PR", made mechanical. Pad sessions
also record the `connected` list from `hermir pads --json` (M3), so the session's `Patch` describes the pads
that were really there.

**What to capture first:** the five emulators of design P1 and P2 (PCSX2, Dolphin, DuckStation, RPCS3,
RetroArch), on Linux and Windows, for the sessions `video-all`, `players-1-xbox` and `players-2-mixed`.
This settles the open items in design §12:
- the Windows pad forms (XInput slot or SDL index);
- Eden's button numbering (SDL GameController numbers or raw joystick numbers).

**`hermir check <emu>`** (a shipped verb; needs M1) is design §8's "on glass" check, for anyone with a real
machine:
1. install into a temp prefix;
2. `prepare`;
3. apply `video-all`;
4. launch without a game and let it settle;
5. revert.

It prints a JSON report per step. Contributors run it before claiming a new entry works, and attach the
report.

### 2.5 What the probe established (2026-10-02)

- From Flathub: PCSX2 2.8.2, DuckStation `0.1-9482-g0a53bc47c`, Dolphin 2609; 91 s, 4.3 GB with runtimes.
- **First start under Xvfb, fresh `HOME`, 25 s, no interaction:** each wrote its defaults.
  - PCSX2: `inis/PCSX2.ini`, 12.5 KB, every setting.
  - DuckStation: `settings.ini`, 8.5 KB, every setting.
  - Dolphin: mostly sparse files — `Dolphin.ini` 50 bytes, `GFX.ini` empty, `GCPadNew.ini` and
    `WiimoteNew.ini` with defaults.
- **`hermir config apply` on those files:** every PCSX2 and DuckStation key the catalog binds exists under
  exactly that spelling, and only those lines changed. For Dolphin, hermir added the sections it binds to
  its sparse files.
- **In Docker:** a default container fails (`bwrap: Creating new namespace failed`); `--privileged` works,
  and PCSX2's file was byte-identical to the run outside Docker.
- **Unstable values:** Dolphin writes a random `[Analytics] ID` on every first start, hence `masks`.
- **Relaunch on the patched files:** PCSX2 and DuckStation did not rewrite their settings on start or on
  `SIGTERM`, hence the limits of the launch smoke in §2.3.
- **A bug it hit:** detection ignores `FLATPAK_USER_DIR` (M0.3).

## 3. M1 — launch

**Model** (`model.rs`, matching design §2.2):

```rust
pub struct LaunchRequest { pub file: Option<PathBuf>, pub platform: Option<String>,
                           pub fullscreen: Option<bool>, pub profile: Option<String>,
                           pub patch: Option<Patch> }        // launch-only knobs ride along; no state
pub struct LaunchSpec    { pub exe: Exe, pub args: Vec<OsString>, pub env: Vec<(OsString, OsString)>,
                           pub cwd: Option<PathBuf>,
                           pub sandbox: Vec<String> }        // `flatpak run` options, e.g. --filesystem
```

**Templates.** The grammar is checked at catalog load, so an unknown placeholder is a validation error:
- `{file}`;
- `{fullscreen:<arg>}` — split at the *first* colon, so Flycast's `window:fullscreen=yes` keeps its colons;
- `{platform}`;
- `{core}` — RetroArch: the core's path from a `cores` map, platform → libretro core id, in the
  RetroArch entry, under its cores directory;
- `{profile}` (M2).

Today all 23 entries carry a template and no code reads one. A Flatpak copy can't read a game outside its
sandbox, so `launch` adds `--filesystem=<game's dir>:ro` to `sandbox` for Flatpak installs.

**Launch-only knobs.** A knob binding gains `"via": "launch"` for settings that exist only as a flag. The first
users are the four emulators whose `video.fullscreen` is unsupported or undescribed today, though their
templates already carry the flag: ares (`--fullscreen`), Cemu (`-f`), melonDS (`-f`) and RMG
(`--fullscreen`). The first three already say "a launch flag" in their catalog notes. Other flags take an `arg` template.

`apply` reports these knobs as `Applied` with the note "on launch". `launch` renders them from
`LaunchRequest.patch`. Nothing is stored between the two calls.

**The `Adapter` trait** (design §4.3) replaces the string `match` and the hand-kept `ADAPTERS` list in
`config/adapters/mod.rs`:

```rust
pub(crate) trait Adapter: Send + Sync {
    fn players(&self, cx: &Cx, players: &[Player]) -> Plan;
    fn launch_extra(&self, cx: &Cx, req: &LaunchRequest) -> LaunchExtra { LaunchExtra::default() }
}
```

**Facade and CLI:**
- `EmulatorHandle::launch(&Install, &LaunchRequest) -> Result<LaunchSpec>`.
- `hermir launch <emu> [<file>] [--platform p] [--fullscreen] [--profile x] [--json]` prints the spec.
- `hermir run …` spawns it (`Exe::FlatpakRun` → `flatpak run <sandbox…> <id> <args…>`), with stdio
  inherited, and forwards the exit code. Nothing is ever joined into a shell string (design §6).

**Tests:**
- A snapshot of every entry's rendered spec, per OS, for a fixed request (blessable, like T1's B).
- Grammar unit tests.
- T2b's launch smoke starts from `launch`'s own spec.
- `hermir check` uses it.

**Done when:**
- `hermir run pcsx2 <file> --fullscreen` starts PCSX2 fullscreen from the Flatpak on Linux and from a managed
  copy on Windows;
- every entry renders;
- catalog validation rejects unknown placeholders.

## 4. M2 — profiles

**Catalog.** A `profile` block per entry, describing the emulator's own mechanism. Each flag is verified
against the current release by T2 (the profile directory fills, and the user's root stays byte-identical):

| kind | emulators | flag | patched |
|---|---|---|---|
| `user_dir` | Dolphin | `-u {profile}` | a copy of the user directory |
| `config_file` | RPCS3 (`--config`), DuckStation (`-settings`) | the file | a copy of that file |
| `append` | RetroArch | `--appendconfig {profile}/override.cfg` | an override file only; no seeding |
| `portable_dir` | Cemu, and managed copies with a portable marker | none | a portable root of its own |
| absent | the rest | — | in place, through the transaction; `Applied.profile` says so |

**Where it lives:**
- `<prefix>/profiles/<emu>/<name>/`;
- for a Flatpak copy, `~/.var/app/<id>/data/hermir/profiles/<name>/`, which the app can always reach,
  so no extra sandbox permission is needed.

**Seeding:**
- `profile create` copies the files the catalog names (`config.files` and adapter-owned files) from the
  user's root, once;
- `--fresh` starts from the emulator's defaults;
- `profile reset` re-seeds.

**Apply** is the same code: an `Install` whose `config_root` is the profile. The transaction still guards
against crashes, but revert is optional: the profile is hermir's own.

**API and CLI:**
- `EmulatorHandle::profile(name) -> Profile` with `.apply()`, `.path()`, `.reset()` and `.remove()`, plus
  `LaunchRequest.profile`;
- `hermir profile list|create|reset|remove <emu> <name>`, `config apply --profile`, and `launch --profile`.

**Tests:**
- T1 runs every fixture's sessions against a seeded profile root too (checks A–E; G instead of F);
- unit tests for seeding;
- T2's profile smoke: launch with a profile, and the user's root is byte-identical afterwards (hashed
  before and after).

**Done when:** Dolphin, RPCS3, RetroArch and DuckStation run sessions in profiles; every other entry reports
`in place` with its note; the user's own files are unchanged by a profile session.

## 5. M3 — pads and `enumerate`

**Model:**
- `PadRef` gains `guid: Option<String>`, the GUID as SDL reports it. When present it is used verbatim
  instead of being computed.
- The computed GUID uses the pad's own `version` (today every pad gets the Xbox 360's) and SDL's CRC of the
  name.
- `Patch` gains `connected: Option<Vec<PadRef>>`: every pad present, in enumeration order. That lets
  adapters compute the identities emulators actually use:
  - RPCS3's `<name> N` counts pads with the same name;
  - Dolphin's `evdev/<id>/<name>` takes the lowest free id per name;
  - Eden's and Azahar's `port:` counts pads with the same GUID;
  - Cemu's `<index>_<guid>` (to verify).
- Without `connected`, adapters assume the seated pads are all the pads, as today, and the players knob
  says so (Partial).

**Feature `enumerate`:**
- uses the `sdl3` crate (SDL is zlib-licensed, so permissive): `hermir::pads::enumerate() ->
  Vec<PadRef>`;
- `hermir pads [--json] [--watch]`;
- `--pad auto` seats pads in SDL order;
- **static or dynamic SDL** (design §12): static for the release binaries (self-contained), dynamic for
  distribution packages. A CI job builds the feature on all three OSes; it needs CMake and a C toolchain,
  and only that job does.

**Windows forms:** XInput slot or SDL index, for PCSX2, DuckStation and Dolphin. This is decided by T3
fixtures from one Windows machine with both kinds of pad (design §12), then written into the adapters.

**Tests:**
- ordinal unit tests over mixed sets, including the review's RPCS3 case (with a DualSense at index 0 and
  an Xbox pad at index 1, the Xbox pad is number 1 among its name, not 2);
- the uinput spike (§2.3) if it works;
- T3's `players-2-mixed` fixtures.

**Done when** design P2's condition holds and is pinned by T3 fixtures: two different pads, in a chosen seat
order, land as players 1 and 2 in PCSX2, DuckStation, Dolphin, RPCS3 and RetroArch, on Linux and Windows.

## 6. M4 — saves

**Catalog.** `saves` per platform, or `*` for every platform:

```jsonc
"saves": { "ps2": [ { "kind": "memcard", "path": "memcards/", "pattern": "*.ps2", "per_game": false,
                      "setting": { "file": "main", "section": "Folders", "key": "MemoryCards" } },
                    { "kind": "state",   "path": "sstates/", "per_game": true } ] }
```

Paths are relative to the config root, or to `{data}`. `setting` names the key where the user can move the
folder; it is read first, which is why M4 waits for M5's `get`.

`Install` gains `data_root` (design §2.2) for emulators that keep saves outside their config root (a
Flatpak's `~/.var/app/<id>/data/…`, a native `~/.local/share/…`).

**API and CLI:**
- `EmulatorHandle::saves(&Install, platform) -> Vec<SaveLocation { kind, path, per_game }>`, resolved and
  absolute;
- `hermir saves where <emu> [<platform>]`.

hermir doesn't sync (design §7).

**Data and checks:**
- all 23 entries;
- an entry that doesn't know yet says `"unknown": "<why>"`, as `unsupported` knobs do;
- T2's `meta.created` lists what the first start created, so the drift job can check that the folders
  exist where the catalog says (some appear only at the first save; the recipe notes those);
- RetroArch's per-core sorting options make its paths depend on settings, so its entry uses `setting`.

**Done when:** every entry has saves for each of its platforms, or `unknown` with a reason.

## 7. M5 — `config get`, and audio

**Reading:**
- the YAML/BML, XML and JSON editors get a `get` (INI has one), held to T0's "set then get" property;
- a knob is read through its binding in reverse (the bool pair, the inverted `values` table, the inverted
  `scale`);
- a literal the table doesn't know comes back as `Unknown(literal)`, never a guess.

**API and CLI:**
- `EmulatorHandle::get(&Install) -> Current { video, region, unknown: Vec<(knob, literal)> }` and
  `get_native(file, section, key)`;
- `hermir config get <emu> [<knob> | <file>:<section>/<key>] [--json]`.

**Audio:**
- the knobs `audio.device` and `audio.latency_ms` join `KNOBS`, bound where emulators have them;
  `support` lists them like the rest;
- an emulator without a device setting gets the Linux fallback of design §4.2: `PULSE_SINK` in
  `LaunchSpec.env` (PipeWire's Pulse server honours it), reported as Partial with that note;
- the backend selection in design §2.2 waits until a consumer needs it.

**Tests:**
- T1 sessions `audio-device`, `audio-latency`;
- a T1 check H: `get` on hermir's output returns the session's values.

## 8. M6 — catalog gaps

Each lands as a small PR, with a T2 fixture where it can be captured. Items marked *verify* came from the
review but weren't confirmed against the projects:

- **Detection on Windows** for Eden, Flycast, shadPS4, Supermodel and Vita3K (install paths and portable
  markers).
- **Rules that can never fire.**
  - melonDS and Xenia Canary have only a `portable_marker`, which never fires without a path hit.
  - Catalog validation should reject a detect block whose only rule is a marker.
  - Xenia Canary also needs a Linux rule.
- **Vita3K on Linux.** Check whether its rolling release carries a Linux build, and under which asset name
  (*verify*). Add the channel, or say why not in `notes`. This needs M0's release-id fix, because the assets are rebuilt in place.
- **Firmware:**

  | Emulator | Needs | Shape |
  |---|---|---|
  | xemu | MCPX boot ROM, flash BIOS, HDD image | set by paths in `xemu.toml`, not a folder; a new `Firmware.keys` form |
  | Vita3K | firmware PUP | through its own installer, like RPCS3 |
  | Ryujinx | `prod.keys` | like Eden |
  | ares | per-system BIOS files | *verify* |

- **Settings:** describe Snes9x's and RMG's (`config` blocks with no files or knobs today).
- **To verify:** PPSSPP's Windows asset on GitHub releases; whether Xenia Canary's binaries live in a
  separate releases repository; the Ryujinx Flatpak id.
- **README:** mark the non-free licences (DuckStation's PolyForm Strict, Snes9x's) in the emulator table.

## 9. Releases

| Version | Contains | Gate |
|---|---|---|
| 0.1.0 | today's features, hardened (M0); T0, T1, T2a | the M0 ● items done; golden fixtures for at least PCSX2, DuckStation and Dolphin; published, trusted publishing on |
| 0.2.0 | `launch`/`run`, profiles, the drift job, `hermir check`, T3 video fixtures for the P1 five | punktfunk can install, apply, launch and revert through the crate |
| 0.3.0 | pads and `enumerate`, saves, `config get`, audio, T3 pad fixtures | design P2's "done when", proven by fixtures |
| 1.0 | the API frozen | design P4's case, end to end: a PS2 title from a RomM server plays through punktfunk with pads in seat order and no hand-edited file |

## 10. Decisions to take

| # | Question | Recommendation |
|---|---|---|
| 1 | Privileged containers in CI for T2 | Yes; GitHub's Linux runners allow them. Fall back to Flatpak on the runner if that changes. |
| 2 | Capture tooling in an `xtask` crate or a hidden `hermir dev` verb | `xtask`: developer verbs stay out of the shipped CLI. |
| 3 | Unverified downloads: refuse or warn | Warn and record `verified: none`; hosts opt into `require_verified`. A `url` entry must pin its sha256. |
| 4 | `#[non_exhaustive]` on consumer-built structs | No; enums only (see 1.3). |
| 5 | SDL3 linked statically or dynamically | Static in the release binaries; dynamic for distribution packages. |
| 6 | Where a Flatpak copy's profiles live | Under the app's own data directory, so no sandbox permission is needed. |
| 7 | Checking in emulator-written default files | Yes, with their source in `fixtures/README.md`, removed on request. |
