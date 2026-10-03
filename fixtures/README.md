# Fixtures

Real config files, written by each emulator on its first start, that the golden tests
(`crates/hermir-golden`) patch through hermir's public API and revert. They are what keeps the
catalog's bindings honest: a key hermir writes is checked against a file the emulator itself wrote,
not against a string someone typed into a test.

```
fixtures/<emulator>/<version>/<os>/
  meta.json                   how and when it was captured, which catalog file is which
  before/…                    what the emulator wrote on its first start (paths under its config root)
  sessions/<name>.json        a Patch, and the support each knob should report
  hermir/<name>/…             what hermir wrote for that session: the changed files, in full
  after/<name>/…              optional: what the emulator wrote when a person set the same things
                              in its UI (`cargo xtask capture --interactive`)
```

## The checks

`cargo test -p hermir-golden` runs one test per fixture and session (`pcsx2/2.8.2/linux/video-all`):

| | |
|---|---|
| A | each knob reports the support the session's `expect` names |
| B | what hermir wrote equals `hermir/<session>/` byte for byte |
| C | every file hermir wrote is well-formed, by a reader that shares no code with hermir (`toml`, `yaml-rust2`, `roxmltree`, `serde_json`, a small INI reader) |
| D | the keys that changed are the ones the catalog binds for the session's knobs, plus what `may_touch` gives an adapter |
| E | with `after/<session>/`: hermir changed what the emulator changed, to the same values, and nothing else (minus `meta.ignore_after`) |
| G | a second apply changes nothing |
| F | revert gives `before/` back byte for byte, and leaves nothing outstanding |
| H | `get` on what hermir wrote reads back the session's values |
| P | where the emulator has profiles, the session applied to a profile lands in the profile and leaves `before/` alone |

The standard sessions every fixture starts with: `video-all`, `video-off`, `region-eu`,
`audio-device`, `audio-latency`, `players-1-xbox`, `players-2-xbox`, and `players-2-mixed` (an
Xbox pad and a DualSense seated out of SDL's order, with the `connected` list). A new standard
session goes into `hermir_golden::fixture::standard_sessions`; `cargo xtask sessions` then adds
it to every fixture, and a bless records it.

A fixture of another OS runs everywhere: `apply` takes the OS as a parameter, so Windows fixtures
are checked on Linux CI and the other way round.

## Capturing

```sh
cargo xtask capture pcsx2            # in the capture image (ci/capture/Dockerfile), --privileged
cargo xtask capture pcsx2 --host     # the same script on this machine: flatpak, xvfb-run, dbus
cargo xtask capture --all            # every catalog entry with a Flathub id and config files
```

The emulator is installed from Flathub, started once under a virtual display with a fresh home,
and stopped as soon as the files the catalog names exist (90 s at most). What it wrote becomes
`before/`; `meta.created` lists every file the first start made, which is where saves and
firmware folders show up too. A fixture that already exists is left alone when its `before/` is
unchanged; `--force` replaces it.

`ci/capture/recipes/<emulator>.json` holds what an emulator needs beyond that: `masks` for values
that differ on every first start (Dolphin's analytics id), `full_writer` for emulators that write
every setting (every key the catalog binds must then already be in `before/`), `ignore_after`,
`args`, a longer `settle`, `close_after` for emulators that write only on a clean quit,
`checklists` for `--interactive`, and `saves_later` for save folders that appear only at the
first save.

Then bless and review:

```sh
HERMIR_BLESS=1 cargo test -p hermir-golden    # writes expect, hermir/<session>/, may_touch
git diff fixtures/                            # this is the review
```

Blessing writes what hermir does today. The review is where a wrong key shows up: a knob reported
`applied` that the emulator has no setting for, a value the emulator does not spell that way, a
section an adapter should not have touched. `may_touch` is filled once for players sessions and
then kept: an adapter that starts touching more fails D.

## Synthetic fixtures

Edge cases nobody has captured yet (a BOM, CRLF, a compact JSON file) may be written by hand.
Their `meta.captured.by` is `hand`, so they are never mistaken for an emulator's own files.

## Where the files come from

Every `before/` and `after/` file was written by the emulator named in `meta.json`, from the
release `meta.source` names (a Flathub commit, or a release URL). They are configuration the
programs generate, kept here as test data. A project that would rather not have its defaults here
can ask, and its fixtures go.

Two captured versions per emulator and OS are kept; the weekly drift job (`cargo xtask drift`)
adds the new one and drops the oldest. For each emulator it also starts the new release on the
files hermir wrote for `video-all` (it must still start, and a file it rewrites must keep what
hermir set) and checks that its first start made the save folders the catalog names.
