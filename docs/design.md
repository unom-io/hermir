# hermir — design

| | |
|---|---|
| **Status** | P0–P3 implemented (§10): install, detect, `prepare`, `apply`/`revert`/`get`/`support` for players, video, region, audio and native keys, profiles, launch, saves, pad enumeration. Golden fixtures from the Flathub builds of 17 emulators (§8). Windows fixtures, and the Windows pad forms they decide, are what remains (§12). |
| **Date** | 2026-10-03 |
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
| D4 | How config files are touched | **Patches, never rewrites. Round-trip safe. Transactional.** Unknown keys and comments survive; `apply` snapshots the files it changes and `revert` restores them byte for byte, or leaves a file the emulator rewrote since and says so. Where the emulator has a profile flag (Dolphin `-u`, RPCS3 `--config`, RetroArch `-c`), a **profile** is an isolated directory instead and the user's own settings are never touched. |
| D5 | Pad identity | **The consumer describes the pads; hermir writes the bindings.** A `PadRef` carries every identity an emulator might key on (kernel name, bus/VID/PID/version, SDL's GUID and name, enumeration index, evdev node), and `Patch.connected` lists every pad present, so a pad is numbered among the pads of its name or GUID as each emulator numbers it. Enumeration is an optional feature (`enumerate`, SDL 3) for the CLI and consumers without their own input stack. |
| D6 | Canonical layout | **SDL game-controller semantics** (A/B/X/Y, bumpers, triggers, sticks, d-pad, start/back/guide, paddles, touchpad, motion) are the neutral buttons. Each platform's emulated pad (DualShock 2, GameCube pad, Wii U GamePad…) has a mapping table from that layout, in the catalog, overridable per call. |
| D7 | Launching | **hermir builds a `LaunchSpec { exe, args, env, cwd, sandbox }`; the library never spawns.** The CLI's `run` spawns for convenience and testing. A host with its own execution rules (punktfunk's exec templates) stays in charge of what runs. |
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
  consumer ──────────► │ facade   Hermir::open(Options) ─► .emulator("pcsx2") ─► EmulatorHandle │
  (host, frontend,     │          ─► .profile(copy, "x") ─► Profile                             │
   CLI, script)        │ model    Entry · Install · Patch · Player/PadRef · Video · Audio        │
                       │          Applied · KnobValue · LaunchSpec · Prepared · Saves            │
                       │ catalog  emulators/*.json · platforms.json   (embedded, schema-checked) │
                       │ store    the prefix: <id>/app, manifests, installed.json, the lock      │
                       │ channel  flatpak · github · url · libretro core   (fetch, verify, place) │
                       │ detect   PATH · Flatpak · known paths · portable markers               │
                       │ config   ini · qt · yaml/bml · xml · json  +  knobs  +  adapters/ · txn │
                       │ launch   template → LaunchSpec (exe, args, env, sandbox)                │
                       │ prepare  first-run answers · firmware (folder, installer, archive, keys) │
                       │ saves    where each emulator keeps saves and states, per platform       │
                       │ pads     (feature `enumerate`) what SDL 3 sees, as PadRef               │
                       └────────────────────────────────────────────────────────────────────────┘
                                                        │
                                            hermir-cli: hermir <noun> <verb> [--json]
```

Dependencies point downward only: the facade (`lib.rs`) uses everything, `config → model +
catalog`, `channel → store`, nothing imports the facade. `model` has no I/O. `catalog` is data
plus a loader. Each adapter is one file that depends on `model`, the format editors and its own
catalog entry, never on another adapter. Every module but `progress` and `pads` is private; the
facade re-exports what a consumer names.

### 2.2 The model

The types as shipped (`model.rs`; every one `Serialize + Deserialize + JsonSchema`, and the
JSON Schema of a catalog entry is generated from them):

```rust
pub struct Entry { id, name, platforms, license, no_install, channels: PerOs<Channel>, detect: PerOs<Detect>,
                   roots: Roots, launch: Launch, firmware: Option<Firmware>, first_run: PerOs<Vec<FirstRun>>,
                   config: Option<Config>, profile: Option<ProfileSupport>,
                   saves: BTreeMap<platform | "*", PlatformSaves>, notes }

/// A copy on this machine, however it got there.
pub struct Install { emulator, kind: InstallKind /* Managed | Flatpak | Native | Portable */,
                     exe: Exe /* Path | FlatpakRun(id) */, version: Option<String>, config_root: Option<PathBuf> }

pub struct Player { seat: u8 /* 1-based */, pad: PadRef }
pub struct PadRef { name /* the kernel's */, bus, vendor, product, version, index /* SDL order */,
                    evdev: Option<PathBuf>, guid: Option<String> /* as SDL reports it */,
                    gamepad_name: Option<String> /* SDL's */ }

pub struct Patch { players: Option<Vec<Player>>, connected: Option<Vec<PadRef>>,
                   video: Option<Video>, region: Option<Region>, audio: Option<Audio>, native: Vec<Native> }
pub struct Video { fullscreen: Option<bool>, scale: Option<u8> /* 1× = native */, vsync: Option<bool>, aspect: Option<Aspect> }
pub struct Audio { device: Option<String> /* a sink's name, or "default" */, latency_ms: Option<u32> }

/// What `apply` did, knob by knob. A consumer shows this; it never guesses.
pub struct Applied { emulator, knobs: Vec<KnobChange>, steps: Vec<PrepareStep>, note: Option<String> }
pub struct KnobChange { knob, support: Support /* Applied | Partial | Unsupported | Failed */, note, file }
pub struct KnobValue { knob, value /* neutral */, literal /* as written */, file, note }   // `get`

pub struct LaunchRequest { file, platform, fullscreen, core, profile, patch: Option<Patch> }
pub struct LaunchSpec { exe: Exe, args: Vec<String>, env: BTreeMap<String, String>, cwd, sandbox: Vec<String> }

pub struct Saves { platform, locations: Vec<SaveLocation>, unknown: Option<String> }
pub struct SaveLocation { kind: SaveKind /* Save | Memcard | State */, path, beside_game, pattern,
                          per_game, from_setting, exists, note }
```

Three properties hold everywhere: every optional field means "leave as is"; every result says
what it did (`Applied`, `Prepared`, a revert's steps) rather than claiming success; the public
enums are `#[non_exhaustive]`.

### 2.3 The facade

```rust
let h = Hermir::open(Options::default())?;          // prefix, OS, HTTP, runner and Env injectable
h.catalog().entries();                              // what exists
h.installs()?;  h.detect();  h.status(None, false)?;

let e = h.emulator("pcsx2")?;                       // EmulatorHandle: one entry on this machine
e.install(&Quiet)?;  e.update(&Quiet)?;  e.remove(false)?;
let copy = e.best()?.expect("installed");           // every copy: e.copies()?
e.prepare(&copy, Some("ps2"), &[bios])?;            // first-run answers + firmware, step by step
let done = e.apply(&copy, &Patch { video: Some(Video { scale: Some(3), fullscreen: Some(true), ..Default::default() }),
                                   ..Default::default() })?;
e.get(&copy)?;  e.get_native(&copy, "main", "EmuCore/GS", "upscale_multiplier")?;
e.revert(false)?;                                   // byte-identical, or a conflict left as the emulator wrote it
let p = e.profile(&copy, "punktfunk")?;  p.apply(&patch)?;          // isolated where the emulator allows it
e.launch(&copy, &LaunchRequest { file: Some(game), profile: Some("punktfunk".into()), ..Default::default() })?;
e.saves(&copy, Some("ps2"))?;
```

`Hermir` is `Send + Sync`; a handle is cheap. Every write holds the prefix lock (`Error::Locked`,
exit 8, when another hermir has it).

### 2.4 The CLI

Nouns first, verbs second, `--json` everywhere (one JSON document per run, errors included),
exit codes that mean something:

```
hermir catalog   list | show <emu> | resolve [<emu>] | schema | validate <dir>
hermir status    [<emu>] [--check]              managed + detected, versions, what's outdated
hermir detect
hermir install   <emu>        hermir update <emu> | --all        hermir remove <emu> [--purge]
hermir where     <emu>                          exe and config root of the best copy
hermir saves     where <emu> [<platform>]       each save folder, resolved, and whether it is there
hermir prepare   <emu> [--platform p] [--firmware <file>…]
hermir players   <emu> [--pad VID:PID[:NAME][@INDEX]… | --pad auto | --players FILE] [--connected FILE] [--revert]
hermir config    apply <emu> [--fullscreen] [--scale n] [--vsync] [--aspect a] [--region r]
                             [--audio-device NAME] [--audio-latency MS] [--set s/k=v --file f] [--pad …] [--profile x]
                 revert [<emu> | --all] [--force]
                 get <emu> [--profile x]        each knob read back, neutral and literal
                 support [<emu>]                the knob × emulator matrix
hermir profile   list | create | reset | remove | where  <emu> [<name>] [--fresh]
hermir launch    <emu> [<file>] [--platform p] [--core c] [--profile x] [--fullscreen] [--audio-device SINK]
hermir run       …the same…                     spawns it, forwards its exit code
hermir core      install <core>                 a libretro core for the RetroArch here
hermir pads      [--watch]                      (feature `enumerate`) what SDL 3 sees, as PadRef JSON
hermir check     <emu> [--seconds n] [--keep]   install, start, prepare, apply, start, read back, revert
hermir doctor
```

Exit: 0 ok · 1 I/O (or SDL could not list the pads) · 2 catalog, policy or an impossible
request · 3 network · 4 verification · 5 extract or place · 6 not offered on this OS · 7 a config
file could not be written · 8 the prefix is locked · 9 not on this machine. Data on stdout,
progress and what went wrong on stderr.

`--players` takes the library's own `Player` list as JSON, and `--connected` the `PadRef` list
`hermir pads --json` prints. Nothing on the CLI is a second model.

## 3. Catalog

`catalog/emulators/<id>.json`, one file per emulator, validated by `catalog/schema/entry.schema.json`
in CI. The catalog directory lives inside the library crate (`crates/hermir/catalog/`): it is
embedded at build time, and a published crate carries nothing from outside its own directory.
The shape, on the emulator that exercises most of it:

```jsonc
{
  "id": "pcsx2", "name": "PCSX2", "platforms": ["ps2"], "license": "GPL-3.0-or-later",
  "channels": {
    "linux":   { "flatpak": "net.pcsx2.PCSX2" },
    "windows": { "github": "PCSX2/pcsx2", "asset": { "all": ["windows-x64-Qt", ".7z"], "none": ["symbols"] },
                 "exe": "pcsx2-qt.exe", "portable": { "file": "portable.ini" } }
  },
  "detect": {
    "linux":   { "path": ["pcsx2-qt", "pcsx2"], "flatpak": "net.pcsx2.PCSX2" },
    "windows": { "paths": ["%ProgramFiles%\\PCSX2\\pcsx2-qt.exe"], "portable_marker": "portable.ini" }
  },
  "roots": { "flatpak": "~/.var/app/net.pcsx2.PCSX2/config/PCSX2", "native": "~/.config/PCSX2",
             "portable": "<app>", "windows": "%USERPROFILE%\\Documents\\PCSX2" },
  "launch": { "args": ["-batch", "-nogui", "{fullscreen:-fullscreen}", "--", "{file}"] },
  "firmware": { "dir": "bios", "platforms": { "ps2": { "any_of": ["*.bin"], "note": "a dumped PlayStation 2 BIOS" } } },
  "first_run": { "linux": [ { "seed": "inis/PCSX2.ini", "content": "[UI]\nSettingsVersion = 1\n" },
                            { "ini": "inis/PCSX2.ini", "section": "UI", "key": "SetupWizardIncomplete", "value": "false" } ] },
  "config": {
    "files": { "main": { "path": "inis/PCSX2.ini", "format": "ini" } },
    "knobs": {
      "video.fullscreen": { "file": "main", "section": "UI", "key": "StartFullscreen", "bool": ["true", "false"] },
      "video.scale":      { "file": "main", "section": "EmuCore/GS", "key": "upscale_multiplier", "scale": { "multiplier": {} } },
      "audio.device":     { "file": "main", "section": "SPU2/Output", "key": "DeviceName", "values": { "default": "" }, "text": "{value}" },
      "region":           { "unsupported": "the BIOS decides the region; PCSX2 has no override" }
    },
    "players": { "adapter": "pcsx2" }       // a Rust adapter writes the bindings
  },
  "saves": { "ps2": [ { "kind": "memcard", "path": "memcards", "pattern": "*.ps2",
                        "setting": { "file": "main", "section": "Folders", "key": "MemoryCards" } },
                      { "kind": "state", "path": "sstates", "per_game": true, "pattern": "*.p2s" } ] }
}
```

`catalog/platforms.json` carries the platform ids and their **aliases** (RomM slug, ES-DE folder,
libretro system name), extensions and a ranked list of emulators, so a consumer maps its own
vocabulary once, at the edge. The canonical → emulated-pad button tables live in the adapters
(§4.3), next to the code that writes them.

**Channels** (§2.1): `flatpak` shells out to `flatpak install --user -y --noninteractive
flathub <id>`; `github` resolves the latest release, picks the asset by `all`/`none` (or exact
`name`) case-insensitive substring filters (regex invites mistakes), verifies the API's `digest`
(sha256, since 2025-06), and tells a rolling release rebuilt under one tag (Vita3K's
`continuous`, Xenia Canary's) by its release id; `url` takes a fixed URL and pins its sha256 in
the entry (Dolphin, RetroArch, ScummVM); a libretro core
fetches `buildbot.libretro.com/nightly/<os>/x86_64/latest/<core>_libretro.<so|dll>.zip` into the
cores dir — the URL RetroArch's own updater uses. Every download lands as `.part`, is verified,
then extracted (zip, 7z, tar.gz in-process; an AppImage unpacked) into `<prefix>/<id>/app/`, the emulator's home: a portable emulator keeps its
config, saves and firmware beside its exe, so they live in there too. `<id>/manifest.json` lists
what the release shipped (path → sha256) and is what makes the rest safe: an **update** replaces
shipped files, keeps a shipped file the user modified (the new one lands beside it as `.new`),
deletes unmodified files the new release dropped, and touches nothing else; a **remove** takes
the unmodified shipped files away and leaves the data, unless purged. `installed.json` records
id, version, release id, channel, the sha256 that arrived and what it was verified against
(`published`, `pinned`, `flatpak`, or nothing, which `--require-verified` refuses). The whole store
is idempotent and resumable, never follows a link out of itself, and holds the prefix lock.

**Policy lives here.** No Switch emulator has a channel. Every source is the emulator's own
release channel. A catalog entry is a reviewed PR, and a weekly CI job dry-resolves every entry so
a renamed asset is a PR, not a support thread.

## 4. Configuration

### 4.1 Formats

Six, each a small round-trip-safe line editor (`config/{ini,yaml,xml,json}.rs`) that patches
one key in place and leaves everything else — comments, order, unknown keys, a byte-order
mark, line endings — untouched. No parsing library: the files are flat enough, and what is not
changed is not re-serialised.

| format | used by | editor |
|---|---|---|
| `ini` (also flat TOML and RetroArch's `key = "value"`) | PCSX2, DuckStation, Dolphin, PPSSPP, mGBA, xemu, Xenia, melonDS, RetroArch, Flycast, Supermodel, shadPS4 | own, line-based, keeps the file's `key = value` spelling |
| `qt` (Qt's ini) | Azahar, Eden | `ini` plus the `key\default=false` companion Qt needs |
| `yaml` (two levels) / `bml` | RPCS3, Vita3K / ares | own, line-based, section + key, no reflow |
| `xml` (elements by path) | Cemu `settings.xml` | own, text of one element, missing branches created |
| `json` (root, or one object under it) | Ryujinx `Config.json`, shadPS4 `config.json` | own, one scalar per line |

A format editor never knows what an emulator is; an adapter never parses text. Whole files
hermir owns (Cemu's controller profiles, RPCS3's input config) are written as such.

### 4.2 Knobs (data)

A knob is a neutral name (`video.fullscreen`, `video.scale`, `video.vsync`, `video.aspect`,
`region`, `audio.device`, `audio.latency_ms`) bound in the catalog entry's `config` block to a
file, a section, a key and a spelling: `bool: [true, false]` as the emulator writes them, a
`values` table from the neutral value to the emulator's literal, a `scale` (`multiplier`,
`percent`, `lines` with a base, a `map` from `n`), or a `text` template (`"\"{value}\""` where the
format quotes) for a device's name or milliseconds, with a `range` where the emulator has one. `also` carries companion keys (melonDS's
renderer with its scale, Xenia's second axis, shadPS4's mode string), `note` makes a knob
partial. `apply` walks the patch, renders each knob, patches the file through the transaction
(§4.3), and records a `KnobChange` — `Unsupported` with the catalog's note when the emulator
has no such setting, or when the table lacks that value (RPCS3 has no "auto" region). That
note is the honest answer a UI shows; `hermir config support` is generated from the same data,
and `get` reads every bound knob back through the same binding (a literal no table names comes
back as its literal, never a guess). `Patch::native` reaches any other key of a named file
through the same transaction.

Where a setting exists only as a launch flag (Dolphin's `-C Dolphin.Display.Fullscreen=True`,
Flycast's `-config window:fullscreen=yes`), the knob's binding says `"via": "launch"` and the
value travels in the `LaunchSpec` instead of a file (ares, Cemu and melonDS take fullscreen
that way). Audio routing on Linux has a fallback no emulator needs to support: `launch` sets
`PULSE_SINK` (in `env`, or `--env=` in a Flatpak's `sandbox`), which PulseAudio and PipeWire's
Pulse server honour; `apply` reports `audio.device` Partial with that note where the emulator has
no device setting of its own.

### 4.3 Players (code)

Controllers are where the formats stop being flat. Each emulator keys a player's device
differently, and that is the whole reason `PadRef` carries every identity:

| emulator | device identity in its config | bindings | file |
|---|---|---|---|
| PCSX2, DuckStation | `SDL-<index>/<control>` (shared input code) | per emulated button, `[Pad1]` | `PCSX2.ini` / `settings.ini` |
| Dolphin | `evdev/<n>/<kernel name>`, n counting same-named devices (Linux); `XInput/<slot>/Gamepad` (Windows) | `Buttons/A = `Button 0`` | `GCPadNew.ini`, `WiimoteNew.ini` |
| RPCS3 | handler `SDL`, `Device: <SDL name> <n>`, n counting same-named pads from 1 | per PS3 button | `input_configs/global/Default.yml` |
| RetroArch | `input_playerN_joypad_index`; autoconfig by the driver does the rest | — | `retroarch.cfg` |
| Cemu | `<uuid>` = `<n>_<SDL GUID>`, n counting same-GUID controllers | XML profile per controller | `controllerProfiles/controllerN.xml` |
| xemu | `portN = "<SDL GUID>"` | fixed Xbox layout | `xemu.toml` |
| Azahar, Eden | `engine:sdl,guid:<GUID>,port:<n>,…`, n counting same-GUID pads (Eden zeroes the GUID's CRC) | per 3DS / Switch button | `qt-config.ini` |
| melonDS | joystick index | raw button numbers | `melonDS.toml` |
| Supermodel | `JOY<index+1>_…` on its SDL game-controller system | per cabinet input | `Supermodel.ini` |
| PPSSPP, Flycast, Vita3K, RMG | — | they map SDL pads themselves, in the order they appear | — |

Each row is pinned by the golden fixtures (§8): `players-1-xbox`, `players-2-xbox` and
`players-2-mixed` (an Xbox pad and a DualSense, seated out of SDL's order, with the connected
list).

An adapter is a function from the copy and its `Seating` (the seats, and every pad connected
when the consumer gave them) to edits and a caveat, one file per emulator in
`config/adapters/`, listed in one table that catalog validation checks against:

```rust
pub(crate) struct Adapter { pub name: &'static str, pub players: fn(&Cx, &Seating) -> Plan }
pub(crate) type Plan = Result<Bindings /* edits + an optional note */, String /* why not */>;
```

The transaction (`config/txn.rs`): every file an apply touches is snapshotted first (its
bytes, or the fact that it did not exist, under `<prefix>/.snapshots/<id>/`), the edits of one
file are applied to its text in order, and the result lands atomically (a unique temp file,
fsync, rename; links followed, permissions kept). The first snapshot of a file is the one that
stays, so a second apply writes over hermir's own text, not the player's, and one `revert`
undoes the whole session and forgets the snapshot. A file the emulator rewrote after hermir
wrote it is a **conflict**: `revert` leaves it and says so, unless forced. A crash mid-apply
leaves a snapshot that `hermir config revert` finishes.

The **index trap** is handled, not hidden. An SDL index is the enumeration order at launch,
and several emulators number a pad among the pads of its name or GUID instead. With
`Patch.connected` (every pad present, which `enumerate` reads just before `launch`), the numbers
are exact; without it the seated pads stand for all of them, and `players` is Partial with a note
when a gap below a seated pad makes that a guess. Emulators that bind raw button numbers (Dolphin's
evdev backend, melonDS) are written for the Xbox pad's layout and say so for any other pad.

### 4.4 Profiles

A profile is an isolated config root for one consumer's purposes. Where the emulator has a flag
(Dolphin `-u <dir>`, RPCS3 `--config <file>`, RetroArch `-c <cfg>`), `profile(copy, "x")` seeds
it from the player's settings once (or from the emulator's defaults, `--fresh`), patches only the
copy, and `launch` adds the flag. A Flatpak's profiles live in its own data folder
(`~/.var/app/<id>/data/hermir/profiles/<name>`), where its sandbox can reach them; anyone else's
under `<prefix>/profiles/<emu>/<name>`. Where there is no flag (DuckStation has none for its
settings file), a profile degrades to the transactional in-place patch and `Applied.note` says so.
RetroArch takes `-c`, not `--appendconfig`: with an appended config, save-on-exit writes the
session's values into the player's own `retroarch.cfg`. The player's own settings are never the
thing a consumer edits by accident.

## 5. Detection

`detect` runs the catalog's `detect` block per OS — `PATH` names, Flatpak app ids (user and system
installations, `$FLATPAK_USER_DIR` and `$FLATPAK_SYSTEM_DIR` honoured), known install paths with
`%VAR%`, `$VAR` and `~` expansion (Scoop's `%USERPROFILE%\scoop\apps\<app>\current` among them),
portable markers beside a found exe — and the managed prefix, deduplicated by canonical path.
Results are `Install`s with `kind` and the config root resolved from `roots`. A detect block
whose only rule is a portable marker never fires, so validation refuses it. Versions are known
for managed copies (the release); detected copies carry none yet. Detection is pure over an
injectable `Env` (file system, `which`, variables, home), so it is unit-tested against fake trees
for both OSes; `RealEnv::with_home` moves the home and every per-user variable with it
(`hermir check`).

## 6. Launch

`launch` renders the catalog template with the request — each argument a literal or one
placeholder: `{file}`, `{fullscreen:<arg>}`, `{platform}`, `{core}` (RetroArch's libretro core for
the platform, by `launch.cores`) — puts the profile's flag first, and returns the `LaunchSpec`.
Without a game only the `{fullscreen:…}` arguments stay, so the emulator opens on its own. For a
Flatpak, `exe` is `FlatpakRun("net.pcsx2.PCSX2")` and `sandbox` carries the `flatpak run`
arguments the run needs (`--filesystem=<the game's folder>`, `--env=PULSE_SINK=…`); the consumer
decides whether that becomes `flatpak run …` or a portal call. Files are single arguments;
nothing is ever shell-joined. `hermir run` spawns the spec and forwards the exit code.

## 7. Firmware and saves

Two views of catalog knowledge, resolved against an `Install`:

- `prepare(install, platform, files)` answers the copy's first-run questions — catalog
  `first_run`: one ini key, a seed file, a folder or a file copied from the program folder, what
  clicking through writes — and puts the platform's firmware in place, in one of four forms:
  copied into the firmware folder; handed to the emulator's own installer where that is the only
  way (RPCS3 `--headless --installfw <PUP>`, Vita3K `--firmware <PUP>`, judged by a file the
  install leaves, not by the exit code); unpacked from an archive while none is installed (Eden's
  system firmware zip); or copied in and pointed at by a key of the emulator's settings (`keys`:
  xemu's MCPX boot ROM, flash BIOS and disk image, each told from the others by its name; a key
  that already names a file is the player's and stays). Each step comes back applied, present or
  failed; a platform still without its firmware says so with the catalog's note.
- `saves(install, platform)` → one `Saves` per platform: each `SaveLocation { kind: Save | Memcard |
  State, path, beside_game, pattern, per_game, from_setting, exists }`. The catalog's folders are
  under the config root, beside it (`{data}/`: the XDG or Flatpak data directory that goes with
  it), or beside each game (`{game}`); a setting that moves a folder (PCSX2's `[Folders]`, Cemu's
  `mlc_path`, RetroArch's `savefile_directory`) is read first. So a sync client knows what to
  watch. hermir does not sync.

## 8. Testing

- **Formats**: property tests — patch then revert is byte-identical; patch never changes an
  untouched line; set then get returns what was set.
- **Golden fixtures** (`crates/hermir-golden`, `fixtures/`): each emulator's own first-start files,
  captured headless from its Flathub build (`cargo xtask capture`, in Docker or on the host), and
  standard sessions (video, region, audio, one and two Xbox pads, mixed pads). Each fixture ×
  session is a test that checks what `apply` reports (A), its output byte for byte (B), that every
  output parses with an independent parser (C), that only the bound keys changed (D), against what
  the emulator itself wrote when a person set the same in its UI (E, `capture --interactive`),
  `revert` (F), a second `apply` (G), `get` reading it back (H), and profiles (P).
- **Drift**: a weekly job (`cargo xtask drift --all`) re-captures every emulator's current
  release, reruns the checks, starts the emulator on the files hermir wrote and checks it kept
  them, checks that its save folders appear where the catalog says, and opens a pull request with
  what moved.
- **Catalog**: schema validation on every PR; weekly dry-resolution of every channel, with an
  issue opened on failure; cargo-deny for licences and advisories.
- **Store**: install/update/remove/resume against a local fake release server, hostile archives.
- **Detection**: fake trees per OS.
- **On glass**: `hermir check <emu>` installs into a throwaway prefix and home, starts the
  emulator, prepares it, applies a session, starts it again, reads the session back and reverts —
  the thing a contributor runs on a real box before saying an entry works.

## 9. Repository

```
hermir/
  Cargo.toml                 workspace: hermir, hermir-cli, hermir-golden, xtask
  crates/hermir/src/
    lib.rs (the facade) model.rs error.rs catalog.rs store.rs progress.rs
    channel/{mod,flatpak,github,url,libretro,http,extract}.rs
    detect.rs players.rs prepare.rs launch.rs saves.rs pads.rs (feature `enumerate`)
    config/{mod,knobs,txn,ini,yaml,xml,json}.rs
    config/adapters/{mod,pad_ini,azahar,cemu,dolphin,duckstation,eden,melonds,pcsx2,retroarch,rpcs3,supermodel,xemu}.rs
  crates/hermir/catalog/{emulators/*.json,platforms.json,schema/entry.schema.json}
  crates/hermir-cli/src/{main,cli,parse,render}.rs, cmd/*.rs
  crates/hermir-golden/      the golden harness (publish = false)
  crates/xtask/              capture, drift, sessions (publish = false)
  ci/capture/                the capture image, script and recipes
  fixtures/<emu>/<version>/<os>/…
  docs/design.md docs/plan.md docs/publishing.md
  .github/workflows/{ci,catalog-dry-run,golden-drift,release}.yml
```

Dependencies: `serde`, `serde_json`, `schemars`, `thiserror`, `sha2`, `ureq` (with the platform
verifier), `zip`, `sevenz-rust2`, `tar`, `flate2`; the CLI adds `clap`; feature `enumerate` adds
`sdl3` (the system's SDL 3), `enumerate-static` builds a joysticks-only SDL 3 from source and
links it in (the release binaries; CMake and a C compiler). Every one permissively licensed, which
`deny.toml` holds to. No `unsafe` in the workspace.

## 10. Phases

| # | Delivers | Done when |
|---|---|---|
| **P0 — get** | workspace, model, catalog + schema, store, channels, detect, `status/install/update/remove/where/doctor`, release binaries **(done)** | `hermir install pcsx2` places a working PCSX2 on Windows and a Flatpak on Linux; `status --json` lists managed + detected; a `.part` resumes; weekly dry-run green |
| **P1 — set** | formats, data knobs, the transaction, `config apply/revert/support/get`, `launch`/`run` **(done; Linux fixtures)** | `apply --scale 3 --fullscreen` on PCSX2, Dolphin, DuckStation, RPCS3, RetroArch; `revert` byte-identical; support matrix generated |
| **P2 — players** | `PadRef`, adapters, `connected` ordinals, `enumerate` feature, `hermir pads`, `--pad auto` **(done; Windows forms open)** | two pads in a chosen seat order land as player 1 and 2 in all five, on both OSes, from a real machine |
| **P3 — the rest** | the other adapters; profiles; firmware + saves views; audio **(done)** | every catalog emulator has fixtures and a green `hermir check` on one real box per OS |
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

- **Windows fixtures.** Every golden fixture so far is a Linux Flatpak's. The Windows ones — and
  with them the Windows pad forms (XInput slot or SDL index) for PCSX2, DuckStation and Dolphin on
  a machine with both kinds of pad — need one real Windows box (`cargo xtask capture`, then
  `capture --interactive` for the `after/` files).
- **Unverified data**, each marked in its entry or the plan: Cemu's `<n>_<guid>` counting (from
  its source, not a capture), the Windows portable layouts of Flycast and Vita3K, the Scoop app
  names, ares's per-system BIOS files, Snes9x's `FullscreenOnOpen` beyond its first start.
- **Versions of detected copies**: the field exists, nothing fills it yet (an exe's version
  resource on Windows, Flatpak metadata, `--version` where it is fast).
- **macOS**: channels (Homebrew casks, `.dmg`) and the Apple-only emulators — the schema slot
  exists, no entries.
- Keyboard and mouse bindings are not in the model; a consumer with a keyboard-first game passes
  native keys through `Patch.native`.
- Version-split adapters beyond melonDS: PCSX2 1.6 and DuckStation's pre-2024 keys are out of
  scope by D9; the door is the `version` field on `Install`.
