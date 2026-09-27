# hermir — design

| | |
|---|---|
| **Status** | Design — nothing implemented |
| **Date** | 2026-09-27 |
| **Name** | Icelandic *hermir*: emulator, simulator; from *herma*, "to mimic". |
| **One line** | One multi-platform interface for managing emulators: install them, find them, configure them (controllers, video, audio, paths), build their launch, know where their firmware and saves live. A Rust library, and a CLI that is the same thing for every other language. |

hermir is what a launcher, a frontend or a streaming host calls instead of learning fifteen
config formats. It manages emulators; it never scans ROMs, scrapes metadata, or draws a UI.
Its first consumer is [punktfunk](https://github.com/unom-io/punktfunk) (host-side installs on
an operator's click, controller order and resolution written before each launch); it is
designed so that a RomM client, an ES-DE-style frontend or a shell script can use it the same
way.

## 0. Decisions

| # | Decision | Recommendation |
|---|---|---|
| D1 | Scope boundary | **Manage emulators, nothing around them.** No ROM scanning, no metadata, no library, no UI. A consumer brings the file to play, the players at the table, the display it wants; hermir turns that into an installed, configured, launchable emulator. |
| D2 | Two ways to configure | **A neutral model for what every emulator has in common, and native passthrough for everything else.** `apply(Patch { players, video, audio })` covers 90 % of what a consumer wants; `set("pcsx2", "EmuCore/GS.upscale_multiplier", 3)` covers the rest without waiting for a model. |
| D3 | Data or code per emulator | **Data by default, code where the format has logic.** Identity, channels, detection, paths, launch template and the simple knobs (fullscreen, scale, vsync) are catalog entries. Controller bindings and anything structural are a Rust adapter. A new emulator with only simple needs is a JSON file and a PR. |
| D4 | How config files are touched | **Patches, never rewrites. Round-trip safe. Transactional.** Unknown keys and comments survive; `apply` snapshots the files it changes and `revert` restores them byte for byte. Where the emulator has a profile flag (Dolphin `-u`, RPCS3 `--config`, RetroArch `--appendconfig`, Cemu `portable/`), a **profile** is an isolated directory instead and the user's own settings are never touched. |
| D5 | Pad identity | **The consumer describes the pads; hermir writes the bindings.** A `PadRef` carries every identity an emulator might key on (SDL GUID, VID/PID, name, enumeration index, evdev path / XInput slot). Enumeration is an optional feature (`enumerate`, SDL3) for the CLI and consumers without their own input stack. |
| D6 | Canonical layout | **SDL game-controller semantics** (A/B/X/Y, bumpers, triggers, sticks, d-pad, start/back/guide, paddles, touchpad, motion) are the neutral buttons. Each platform's emulated pad (DualShock 2, GameCube pad, Wii U GamePad…) has a mapping table from that layout, in the catalog, overridable per call. |
| D7 | Launching | **hermir builds a `LaunchSpec { exe, args, env, cwd }`; the library never spawns.** The CLI's `run` spawns for convenience and testing. A host with its own execution rules (punktfunk's exec templates) stays in charge of what runs. |
| D8 | Operating systems | **Linux and Windows in v1; macOS in the schema.** Every catalog entry has a slot per OS; a `null` slot is an honest "not offered here", never a crash. |
| D9 | Versions | **Current stable line per emulator.** Where a format split matters (melonDS `.ini` → `.toml` at 1.0), `detect` reads the version and the adapter branches. Ancient versions are out of scope. |
| D10 | Language, runtime, deps | **Rust 2024, blocking I/O, progress by callback.** Small dependency set (§9). No async in the library — a CLI doesn't need it and a tokio host wraps it in `spawn_blocking`. |
| D11 | Licence | **MIT OR Apache-2.0**, as punktfunk. The catalog is data under the same licence; it points at each emulator's own release channel and rehosts nothing. |
| D12 | Switch emulators | **DECIDED 2026-09-27: supported, never installed.** Detect-and-configure entries (Ryujinx and its Ryubing fork, Eden) ship like any other emulator; no catalog entry for them carries an install channel — policy of the catalog, not a code path. |
| D13 | The CLI is the FFI | **Every library type is `serde` and every CLI verb has `--json`.** A JSON Schema is generated from the types (`schemars`) and shipped, so a TypeScript consumer gets typed results without bindings. |

## 1. What a consumer wants

Four questions, in the order a launcher asks them:

1. **Is there an emulator for this platform on this machine?** — `detect`, `status`. If not: **can I get one?** — `install`, on the operator's say-so.
2. **Before I start it, make it right for this session.** Player 1 is this pad, player 2 that one; 1440p, fullscreen, v-sync on; audio into this device; BIOS in place. — `apply`, `firmware`.
3. **Start this file.** — `launch` → a command the consumer runs its own way.
4. **Afterwards, put things back** (a streaming host that borrowed the emulator for a session) and **where did the saves go** (a client that syncs them). — `revert`, `saves`.

Everything hermir exposes is one of those four; anything that isn't, it doesn't do.

## 2. Structure

### 2.1 Layers

```
                       ┌────────────────────────── hermir (library) ───────────────────────────┐
  consumer ──────────► │ facade  Hermir::open(prefix) ─► .emulator("pcsx2") ─► EmulatorHandle │
  (host, frontend,     │                                                                        │
   CLI, script)        │ model    Emulator · Platform · Install · Player/PadRef · Video · Audio  │
                       │          Patch · Applied · LaunchSpec · Firmware · SaveLocation         │
                       │                                                                        │
                       │ catalog  emulators/*.json · platforms.json · pads.json  (embedded,      │
                       │          refreshable, schema-validated)                                 │
                       │                                                                        │
                       │ store    the prefix on disk: <id>/app, <id>/data, installed.json, lock  │
                       │ channel  flatpak · github · url · libretro-core   (fetch, verify, place) │
                       │ detect   PATH · Flatpak · known paths · portable markers · version      │
                       │ config   format/{ini,toml,yaml,xml,racfg}  +  adapters/{dolphin,…}      │
                       │ launch   template → LaunchSpec (exe, args, env, cwd)                    │
                       │ firmware where each emulator wants which files, per platform            │
                       │ saves    where each emulator keeps saves and states, per platform        │
                       └────────────────────────────────────────────────────────────────────────┘
                                                        │
                                            hermir-cli: hermir <noun> <verb> [--json]
```

Dependencies point downward only: `facade → model + everything`, `config → model + format +
catalog`, `channel → store`, nothing imports `facade`. `model` has no I/O. `catalog` is data
plus a loader. Each adapter is one file that depends on `model`, `format` and its own catalog
entry — never on another adapter.

### 2.2 The model

```rust
pub struct Emulator { pub id: EmulatorId, pub name: String, pub platforms: Vec<PlatformId>,
                      pub channels: PerOs<Option<Channel>>, pub license: License, /* … catalog entry */ }

pub struct Platform { pub id: PlatformId, pub name: String, pub aliases: Aliases /* romm, esde, libretro, igdb */,
                      pub extensions: Vec<String>, pub emulated_pad: EmulatedPad, pub firmware: Vec<FirmwareFile>,
                      pub default_emulators: Vec<EmulatorId> /* ranked */ }

/// An emulator that exists on this machine, however it got there.
pub struct Install { pub emulator: EmulatorId, pub version: Option<Version>, pub kind: InstallKind,
                     pub exe: Exe /* Path | FlatpakRun(id) */, pub data_root: PathBuf, pub config_root: PathBuf }
pub enum InstallKind { Managed /* ours, in the prefix */, Flatpak, Native, Portable, Detected }

pub struct Player { pub seat: u8 /* 1-based */, pub pad: PadRef, pub layout: Option<Layout> }
pub struct PadRef { pub name: String, pub kind: PadKind, pub sdl_guid: Option<SdlGuid>,
                    pub vid_pid: Option<(u16, u16)>, pub index: Option<u32>, pub os: OsHandle,
                    pub motion: bool, pub touchpad: bool, pub rumble: bool }
pub enum PadKind { Xbox, DualShock4, DualSense, SwitchPro, Generic }
pub enum OsHandle { Linux { evdev: Option<PathBuf>, uniq: Option<String> },
                    Windows { xinput: Option<u8>, hid_path: Option<String> }, None }

pub struct Video { pub fullscreen: Option<bool>, pub scale: Option<Scale>, pub vsync: Option<bool>, pub aspect: Option<Aspect> }
pub enum Scale { Native(u8) /* 1×–8× the console's resolution */, Output(u32, u32), Auto }
pub struct Audio { pub device: Option<String>, pub backend: Option<AudioBackend>, pub latency_ms: Option<u16> }

pub struct Patch { pub players: Option<Vec<Player>>, pub emulated_pad: Option<EmulatedPad>,
                   pub video: Option<Video>, pub audio: Option<Audio>, pub native: Vec<(Key, Value)> }

/// What `apply` did, knob by knob. A consumer shows this; it never guesses.
pub struct Applied { pub changes: Vec<Change>, pub snapshot: SnapshotId }
pub struct Change { pub knob: Knob, pub support: Support /* Applied | Unsupported | Partial(note) */,
                    pub file: Option<PathBuf> }

pub struct LaunchRequest { pub file: PathBuf, pub platform: PlatformId, pub profile: Option<ProfileId>, pub fullscreen: bool }
pub struct LaunchSpec { pub exe: Exe, pub args: Vec<OsString>, pub env: Vec<(OsString, OsString)>, pub cwd: Option<PathBuf> }
```

Three properties hold everywhere: every type is `Serialize + Deserialize + JsonSchema`; every
optional field means "leave as is"; every result says what it did (`Applied`) rather than
claiming success.

### 2.3 The facade

```rust
let h = Hermir::open(Options { prefix, catalog: CatalogSource::Embedded, os: Os::current() })?;

h.catalog().emulators();                       // what exists
h.installs()?;                                 // Vec<Install>: managed + detected, deduplicated
h.detect()?;                                   // just the detected ones, fresh

let e = h.emulator("pcsx2")?;                  // EmulatorHandle: catalog entry + best Install
e.install(Progress::stderr())?;                // channel for this OS; no-op if current
e.update()?;  e.remove(Purge::KeepData)?;
e.config().get("EmuCore/GS.upscale_multiplier")?;
let done = e.config().apply(&Patch { players: Some(seats), video: Some(Video { scale: Some(Scale::Native(3)), fullscreen: Some(true), ..Default::default() }), ..Default::default() })?;
e.config().revert(done.snapshot)?;             // byte-identical restore
e.config().profile("punktfunk")?.apply(&patch)?; // isolated where the emulator allows it
e.launch(&LaunchRequest { file, platform: "ps2".into(), profile: None, fullscreen: true })?;
e.firmware().status("ps2")?;  e.firmware().place("ps2", &[pup])?;
e.saves().locations("ps2")?;
```

A handle is cheap and stateless; the prefix holds a file lock for the duration of any write.

### 2.4 The CLI

Nouns first, verbs second, `--json` everywhere, exit codes that mean something:

```
hermir catalog   list | show <emu> | refresh | validate [<file>]
hermir status    [<emu>]                       managed + detected, versions, what's outdated
hermir detect    [--fresh]
hermir install   <emu> [--prefix P] [--channel C]
hermir update    <emu> | --all
hermir remove    <emu> [--purge]
hermir where     <emu>                         exe, data root, config root
hermir config    get <emu> [<key>]
                 set <emu> <key>=<value>…
                 apply <emu> [--players spec] [--video …] [--audio …] [--profile name]
                 revert <emu> [<snapshot>]
                 support [<emu>]               the knob × emulator matrix, from the adapters themselves
hermir pads      [--watch]                     (feature `enumerate`) what SDL3 sees, as PadRef JSON
hermir firmware  status <emu|platform> | place <emu> <platform> <file>…
hermir saves     where <emu> [<platform>]
hermir launch    <emu> <file> [--platform p] [--profile x] [--fullscreen]   → LaunchSpec
hermir run       …same…                        spawns it, forwards exit code
hermir doctor                                  flatpak? 7z? FUSE? prefix writable? catalog fresh?
```

Exit: 0 ok · 2 not in catalog / policy · 3 network · 4 verification · 5 extract or place · 6
unsupported on this OS · 7 config format · 8 locked. `--json` on stdout, human text on stderr,
progress on stderr only.

`--players` on the CLI takes the same JSON `PadRef` list the library takes, or `auto`
(feature `enumerate`: seats in SDL order). Nothing on the CLI is a second model.

## 3. Catalog

`catalog/emulators/<id>.json`, one file per emulator, validated by `catalog/schema/emulator.json`
in CI. The shape, on the emulator that exercises most of it:

```jsonc
{
  "id": "pcsx2", "name": "PCSX2", "platforms": ["ps2"], "license": "GPL-3.0",
  "channels": {
    "linux":   { "flatpak": "net.pcsx2.PCSX2" },
    "windows": { "github": "PCSX2/pcsx2", "asset": { "all": ["windows", "x64", "Qt", ".7z"] },
                 "exe": "pcsx2-qt.exe", "portable": { "file": "portable.ini" } },
    "macos":   null
  },
  "detect": {
    "linux":   { "path": ["pcsx2-qt", "pcsx2"], "flatpak": "net.pcsx2.PCSX2" },
    "windows": { "paths": ["%ProgramFiles%\\PCSX2\\pcsx2-qt.exe"], "portable_marker": "portable.ini" }
  },
  "roots": { "flatpak": "~/.var/app/net.pcsx2.PCSX2/config/PCSX2", "native": "~/.config/PCSX2",
             "portable": "<app>", "managed": "<data>" },
  "launch": { "args": ["-batch", "-nogui", "{fullscreen:-fullscreen}", "--", "{file}"] },
  "config": {
    "files": { "main": "inis/PCSX2.ini" },
    "format": "ini",
    "knobs": {
      "video.scale":      { "file": "main", "key": "EmuCore/GS.upscale_multiplier", "type": "native_x" },
      "video.fullscreen": { "file": "main", "key": "UI.StartFullscreen", "type": "bool" },
      "video.vsync":      { "file": "main", "key": "EmuCore/GS.VsyncEnable", "type": "bool" },
      "audio.device":     { "unsupported": "picks the default device; use env on Linux" }
    },
    "adapter": "pcsx2"                       // present ⇒ a Rust adapter owns players + anything structural
  },
  "firmware": { "dir": "bios", "platforms": { "ps2": { "any_of": ["*.bin"], "note": "a dumped PS2 BIOS" } } },
  "saves":    { "ps2": { "memcards": "memcards/*.ps2", "states": "sstates/" } }
}
```

`catalog/platforms.json` carries the platform ids and their **aliases** (RomM slug, ES-DE folder,
libretro system name, IGDB slug), extensions, firmware needs, the emulated pad and a ranked default
emulator list — so a consumer maps its own vocabulary once, at the edge. `catalog/pads.json`
carries pad kinds (VID/PID → kind, glyph family) and the canonical → emulated-pad button tables.

**Channels** (§2.1): `flatpak` shells out to `flatpak install --user -y --noninteractive
flathub <id>`; `github` resolves the latest non-prerelease, picks the asset by `all`/`none`
substring filters (regex invites mistakes), verifies the API's `digest` (sha256, since 2025-06),
uses `releases/latest/download/<asset>` when the name is fixed, and falls back to the Atom feed on
a 403; `url` takes a fixed URL plus a checksum URL (RetroArch's buildbot, ScummVM); `libretro-core`
fetches `buildbot.libretro.com/nightly/<os>/x86_64/latest/<core>_libretro.<so|dll>.zip` into the
cores dir — the URL RetroArch's own updater uses. Every download lands as `.part`, is verified,
then extracted into `<prefix>/<id>/app/`, the emulator's home: a portable emulator keeps its
config, saves and firmware beside its exe, so they live in there too. `<id>/manifest.json` lists
what the release shipped (path → sha256) and is what makes the rest safe: an **update** replaces
shipped files, keeps a shipped file the user modified (the new one lands beside it as `.new`),
deletes unmodified files the new release dropped, and touches nothing else; a **remove** takes
the unmodified shipped files away and leaves the data, unless purged. `installed.json` records
id, version, channel, digest and time. The whole store is idempotent and resumable.

**Policy lives here.** No Switch emulator has a channel. Every source is the emulator's own
release channel. A catalog entry is a reviewed PR, and a weekly CI job dry-resolves every entry so
a renamed asset is a PR, not a support thread.

## 4. Configuration

### 4.1 Formats

Five, each a small round-trip-safe editor that patches keys in place and leaves everything
else — comments, order, unknown keys — untouched:

| format | used by | editor |
|---|---|---|
| `ini` (with `A/B.key` sections and Dolphin's `[Section]` + backtick values) | PCSX2, DuckStation, Dolphin, PPSSPP, Azahar, mGBA | own, line-based |
| `toml` | xemu, Xenia, melonDS ≥ 1.0 | `toml_edit` |
| `yaml` | RPCS3, Vita3K | own, line-based, key-path patch (no reflow) |
| `xml` | Cemu (`settings.xml`, `controllerProfiles/*.xml`) | `quick-xml`, node patch |
| `racfg` (`key = "value"`) | RetroArch, its remaps and autoconfigs | own, line-based |

A format editor never knows what an emulator is; an adapter never parses text.

### 4.2 Knobs (data)

A knob is a neutral name (`video.scale`, `video.fullscreen`, `video.vsync`, `video.aspect`,
`audio.device`, `audio.backend`, `audio.latency_ms`) bound in the catalog to a file, a key and a
type converter (`bool` in the emulator's spelling, `native_x` for scale multipliers, `enum` with
a value map). `apply` walks the patch, resolves each knob, patches the file, and records a
`Change` — `Unsupported` with the catalog's note when the emulator has no such setting. That
note is the honest answer a UI shows; the matrix `hermir config support` is generated from it.

Where a setting exists only as a launch flag (Dolphin's `-C Dolphin.Display.Fullscreen=True`,
Flycast's `-config window:fullscreen=yes`), the knob's binding says `"via": "launch"` and the
value travels in the `LaunchSpec` instead of a file. Audio routing on Linux has a universal
fallback no emulator needs to support: `audio.device` becomes `PULSE_SINK` / `PIPEWIRE_NODE` in
`LaunchSpec.env` when the emulator has no device setting of its own.

### 4.3 Players (code)

Controllers are where the formats stop being flat. Each emulator keys a player's device
differently, and that is the whole reason `PadRef` carries every identity:

| emulator | device identity in its config | bindings | file |
|---|---|---|---|
| PCSX2, DuckStation | `SDL-<index>/<Button>` (shared input code) | per emulated button, `[Pad1]` | `PCSX2.ini` / `settings.ini` |
| Dolphin | `SDL/<index>/<name>` (Linux, Windows), `XInput/<n>/Gamepad`, `evdev/0/<name>` | `Buttons/A = `Button S`` | `GCPadNew.ini`, `WiimoteNew.ini` |
| RPCS3 | handler + `Device: <name> <n>` or `evdev` path | per PS3 button | `InputConfigs/global/Default.yml` |
| RetroArch | autoconfig by VID/PID + driver, `input_player1_joypad_index` | autoconfig profile + per-core remap | `autoconfig/<driver>/*.cfg`, `retroarch.cfg` |
| Cemu | `<uuid>` = `<index>_<sdl guid>`, `<api>SDLController</api>` | XML profile per controller | `controllerProfiles/controllerN.xml` |
| xemu | `port1 = "<sdl guid>"` | fixed Xbox layout | `xemu.toml` |
| Azahar | `engine:sdl,guid:<guid>,port:<n>,button:<b>` | per 3DS button | `qt-config.ini` |
| PPSSPP | SDL device id ranges | per PSP button | `controls.ini` |
| melonDS | joystick index | per DS button | `melonDS.toml` |
| Flycast | mapping file by device name, `maple_sdl_joystick_<n>` | per DC button | `emu.cfg`, `mappings/` |
| Vita3K | SDL index | per Vita button | `config.yml` |
| Xenia | XInput slot order | fixed 360 layout | `xenia-canary.config.toml` |

(From each emulator's documented config; every row is pinned by a fixture file in `fixtures/` when
its adapter lands — §8.)

An adapter's contract:

```rust
pub trait Adapter: Send + Sync {
    fn id(&self) -> EmulatorId;
    /// Write players in seat order; return one Change per seat, Unsupported if the seat count exceeds the emulator's.
    fn apply_players(&self, install: &Install, players: &[Player], emulated: EmulatedPad, tx: &mut Txn) -> Result<Vec<Change>>;
    /// Anything structural the data knobs cannot express (Dolphin's per-Wiimote extension, RetroArch's remap files).
    fn apply_extra(&self, install: &Install, patch: &Patch, tx: &mut Txn) -> Result<Vec<Change>> { Ok(vec![]) }
    /// Launch args the template cannot express (Cemu's `--mlc`, RPCS3's `--config <profile>`).
    fn launch_extra(&self, install: &Install, req: &LaunchRequest, profile: Option<&Profile>) -> Result<LaunchExtra> { Ok(default()) }
}
```

`Txn` is the transaction: every file an adapter touches is snapshotted first (content + mtime
into `<prefix>/.snapshots/<id>/<n>/`), and `revert` restores the set. A crash mid-apply leaves a
snapshot that `hermir config revert` finishes. Snapshots are capped (last 8 per emulator).

The **index trap** is documented rather than hidden: an SDL index is the enumeration order at
launch. Adapters prefer GUID or name forms where the emulator accepts them; where only an index
works, the consumer passes the index it observed, or lets `enumerate` observe it just before
`launch`.

### 4.4 Profiles

A profile is an isolated config root for one consumer's purposes. Where the emulator has a flag
(Dolphin `-u <dir>`, RPCS3 `--config <file>`, RetroArch `--appendconfig <cfg>`, Cemu's `portable/`
directory, DuckStation `-settings <file>`), `profile("x")` seeds it from the user's config once,
patches only the copy, and `launch` adds the flag. Where there is no flag, a profile degrades to
the transactional in-place patch and `Applied` says so. The user's own settings are never the
thing a consumer edits by accident.

## 5. Detection

`detect` runs the catalog's `detect` block per OS — `PATH` names, Flatpak app ids (user and system
installations), known install paths with `%VAR%` and `~` expansion, portable markers — and the
managed prefix, then reads a version where cheap (exe version resource on Windows, `--version`
where the emulator answers in under a second, Flatpak metadata). Results are `Install`s with
`kind` and both roots resolved from `roots`. Detection is pure over an injectable `Env`
(filesystem, `which`, env vars), so it is unit-tested against fake trees for both OSes.

## 6. Launch

`launch` renders the catalog template with the request (`{file}`, `{fullscreen:-flag}`,
`{platform}`), adds `launch_extra` from the adapter, the profile flag, and the env from
`via: launch` knobs, and returns the `LaunchSpec`. For a Flatpak, `exe` is
`FlatpakRun("net.pcsx2.PCSX2")` — the consumer decides whether that becomes `flatpak run …` or a
portal call. Files are single arguments; nothing is ever shell-joined.

## 7. Firmware and saves

Two read-mostly views of catalog knowledge, resolved against an `Install`:

- `firmware().status(platform)` → per file: present / missing / not needed, with the human note
  ("a dumped PS2 BIOS"). `place(platform, files)` copies into the emulator's expected dir, or runs
  the emulator's own installer where that is the only way (RPCS3 `--installfw <PUP>`).
- `saves().locations(platform)` → `SaveLocation { kind: Memcard | Save | State, path, per_game: bool }`,
  so a sync client knows what to watch. hermir does not sync.

## 8. Testing

- **Formats**: property tests — patch then revert is byte-identical; patch never changes an
  untouched line.
- **Adapters**: golden tests on real config files checked into `fixtures/<emu>/<version>/`
  (fresh-install defaults from each emulator). `apply` a fixed patch → diff equals the expected
  file. A new adapter lands with its fixtures or not at all.
- **Catalog**: schema validation on every PR; weekly dry-resolution of every channel (asset
  found, digest present) with an issue opened on failure.
- **Store**: install/update/remove/resume against a local fake release server.
- **Detection**: fake trees per OS.
- **On glass**: `hermir doctor` plus a `hermir check <emu>` that installs into a temp prefix,
  applies a patch, launches with `--version`-class arguments, and reports — the thing a
  contributor runs on a real Windows box before claiming a new entry works.

## 9. Repository

```
hermir/
  Cargo.toml                 workspace: hermir, hermir-cli
  crates/hermir/src/
    lib.rs facade.rs model.rs error.rs
    catalog/{mod,load,schema}.rs
    store/{mod,layout,lock,snapshot}.rs
    channel/{mod,flatpak,github,url,libretro}.rs
    detect/{mod,env}.rs
    config/{mod,knobs,txn,profile}.rs
    config/format/{ini,toml,yaml,xml,racfg}.rs
    config/adapters/{mod,retroarch,dolphin,pcsx2,duckstation,rpcs3,cemu,ppsspp,melonds,azahar,xemu,flycast,vita3k,xenia}.rs
    launch.rs firmware.rs saves.rs
  crates/hermir-cli/src/main.rs
  catalog/{emulators/*.json,platforms.json,pads.json,schema/*.json}
  fixtures/<emu>/<version>/…
  docs/design.md  docs/catalog.md  docs/consumers.md
  .github/workflows/{ci,catalog-dry-run,release}.yml     release: linux-x64, windows-x64, macos-arm64 binaries
```

Dependencies: `serde`, `serde_json`, `schemars`, `thiserror`, `toml_edit`, `quick-xml`, `zip`,
`sevenz-rust`, `sha2`, `ureq`, `fs2` (lock), `semver`; CLI adds `clap`; feature `enumerate`
adds `sdl3`. Nothing async, nothing that needs a C toolchain beyond what `sdl3` brings behind
its feature.

## 10. Phases

| # | Delivers | Done when |
|---|---|---|
| **P0 — get** | workspace, model, catalog + schema, store, channels, detect, `status/install/update/remove/where/doctor`, release binaries | `hermir install pcsx2` places a working PCSX2 on Windows and a Flatpak on Linux; `status --json` lists managed + detected; a `.part` resumes; weekly dry-run green |
| **P1 — set** | formats, data knobs, `Txn` + snapshots, `config get/set/apply/revert/support`, `launch`/`run` | `apply --video scale=3,fullscreen=true` on PCSX2, Dolphin, DuckStation, RPCS3, RetroArch; `revert` byte-identical; support matrix generated |
| **P2 — players** | `PadRef`, pads catalog, adapters for RetroArch, Dolphin, PCSX2, DuckStation, RPCS3; `enumerate` feature; `hermir pads` | two pads in a chosen seat order land as player 1 and 2 in all five, on both OSes, from a real machine |
| **P3 — the rest** | Cemu (Wii U GamePad + Pro), PPSSPP, melonDS, Azahar, xemu, Flycast, Vita3K, Xenia adapters; profiles; firmware + saves views | every catalog emulator has fixtures and a green `hermir check` on one real box per OS |
| **P4 — consumers** | punktfunk links the crate (installs on approval, `prepare` → `apply`/`revert`); ROM Manager's registry generated from `catalog/`; a `docs/consumers.md` recipe for a script and for a RomM client | the Discord case: a PS2 title on a RomM server plays through punktfunk with pads in seat order and no hand-edited file |

P0 is the [emu-get scope from the punktfunk design](https://git.unom.io/unom/punktfunk-planning)
and can ship alone. P1 and P2 are independent of each other.

## 11. Prior art

- **Batocera `configgen`** — the closest thing to §4.3: one controller model, a Python generator per
  emulator writing its native format before every launch. The reference for what each format needs;
  Python, Linux-only, coupled to Batocera's ES. Check its licence before porting a table.
- **ES-DE `es_find_rules.xml`** — per-OS detection rules (PATH names, Flatpak ids, Windows paths,
  AppImage names) as data; the model for `detect` blocks. MIT.
- **EmuDeck** — installs and fully configures on Linux and Windows; GPL-3 shell/PowerShell, GUI-first,
  overwrites user config — the thing D4 is designed against.
- **Freegosy** — MIT Flutter RomM client with a Dart emulator registry (GitHub-release asset filters,
  BIOS registry); seed data for the catalog.
- **RetroDECK, RetroBat, Lutris runners** — monoliths or single-OS; not interfaces.

## 12. Open items

- The Windows-side identity forms (XInput slot vs SDL index) for PCSX2/DuckStation/Dolphin on a
  machine with both kinds of pad plugged in — resolve with fixtures on a real box in P2.
- Whether `enumerate` should ship SDL3 statically or dynamically; static keeps the binary
  self-contained, dynamic keeps it small.
- macOS channels (Homebrew casks, `.dmg`) and the Apple-only emulators — schema slot exists, no
  entries.
- Keyboard and mouse bindings are not in the model; a consumer with a keyboard-first game passes
  native keys through `Patch.native`.
- Version-split adapters beyond melonDS: PCSX2 1.6 and DuckStation's pre-2024 keys are out of
  scope by D9; the door is the `version` field on `Install`.
