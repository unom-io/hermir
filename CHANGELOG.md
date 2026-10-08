# Changelog

Notable changes to `hermir` and `hermir-cli`, which share a version. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org), where a 0.x minor may break.

## [Unreleased]

The first release: P0–P3 of the [design](docs/design.md) — get, set, players, and the rest.

### Added

- A catalog of 23 emulators, embedded in the library, parsed strictly, with a JSON Schema and
  per-OS channels, detection rules, config roots and firmware folders.
- Install, update and remove from each emulator's own channel — Flatpak on Linux; GitHub
  releases or a pinned official URL on Windows — over https only, with the OS's trust store and
  a timeout on every step. A download is checked against the sha256 GitHub publishes or the one
  the catalog pins; `installed.json` records which (`verified: published | pinned | flatpak |
  none`), the CLI says `not verified: <why>` when there was nothing to check against, and
  `Options::require_verified` / `--require-verified` refuses such a download. Downloads resume
  only into the same build. An update or a remove keeps shipped files the user edited (the new
  ones beside them as `.new`), never touches config, saves or firmware, never follows a link the
  user put in the copy's folder, and finishes a place that stopped halfway.
- Archives are refused when an entry or a link would land outside the copy's folder.
- Detection of copies the user installed: `PATH`, Flatpak (user and system), known install
  paths, portable markers.
- libretro cores from the buildbot into RetroArch's cores directory.
- `prepare`: answers a copy's first-run questions (the PCSX2 and DuckStation setup wizards,
  DuckStation's update check, Dolphin's statistics question, Cemu's getting-started dialog,
  RPCS3's welcome box) and places a platform's firmware, or runs RPCS3's own installer on a
  `PS3UPDAT.PUP`.
- RPCS3 on Linux installs as the project's own AppImage, unpacked: the Flathub build asks at
  every launch whether to run an unofficial build.
- Xenia Canary on Linux installs as the project's own AppImage; it outran the Windows build
  under Proton. Windows takes the main repo's release too.
- Eden's keys and system firmware: `prepare` places `prod.keys`/`title.keys` and unpacks a
  firmware zip while no firmware is installed.
- `apply_players`: the consumer's pads written into an emulator's own bindings, seat by seat —
  Eden, Azahar, Dolphin (GameCube pad, Wii Remote + Nunchuk), melonDS, Cemu, PCSX2, DuckStation,
  RPCS3, RetroArch, Supermodel and xemu; the SDL-native ones (PPSSPP, Flycast, Xenia, shadPS4,
  Vita3K, mGBA) say they map pads themselves. Every file is snapshotted first;
  `revert_players` puts the player's own settings back byte for byte.
- shadPS4 (the SDL core, from its zipped AppImage) and Supermodel (its Linux tarball, with
  `Games.xml` and the crosshairs copied into `~/.supermodel` on first run).
- `apply`: a session's settings into an emulator's own files, knob by knob — `video.fullscreen`,
  `video.scale` (a multiple of the console's resolution), `video.vsync`, `video.aspect`,
  `region`, players, and `native` keys for anything the model lacks. Each knob comes back
  applied, partial (with what to know) or unsupported (with why). `revert` restores every
  file a session touched, byte for byte; `support` is the knob × emulator matrix. Where a
  knob lands is catalog data (`config.files`, `config.knobs`, `config.players`), described
  for every entry: ini and Qt ini, flat TOML, RetroArch's cfg, two-level YAML (RPCS3, Vita3K),
  BML (ares), XML (Cemu), JSON (Ryujinx, shadPS4), each patched in place with comments, order, BOM
  and line endings kept.
- ares (most cartridge-era systems), Rosalie's Mupen GUI (Nintendo 64) and Snes9x.
- `get` and `config get`: every knob read back through its binding, neutral and literal; a
  literal no table names comes back as itself, never as a guess.
- Audio: `audio.device` and `audio.latency_ms`, written where the emulator keeps them (PCSX2,
  DuckStation, RPCS3, RetroArch; latency in ares, DOSBox Staging and Snes9x too). On Linux an
  emulator without a device setting gets the device through `PULSE_SINK` at launch.
- `launch` and `run`: the catalog's argument template rendered for a game, a platform, a
  RetroArch core and fullscreen into a `LaunchSpec` the consumer runs its own way; a Flatpak
  gets `--filesystem=` for the game's folder. Fullscreen travels on the command line where the
  emulator has no setting for it (ares, Cemu, melonDS).
- Profiles: settings of a copy's own for a session, made from the player's or from defaults,
  where the emulator has a flag for another settings folder (Dolphin `-u`, RPCS3 `--config`,
  RetroArch `-c`); `launch` adds the flag. Elsewhere a profile is the snapshotted in-place patch,
  and `apply` says so.
- Pads: `PadRef` carries SDL's GUID and name when the consumer read them, and `Patch.connected`
  lists every pad present, so RPCS3, Dolphin, Eden, Azahar and Cemu get a pad's number among the
  pads of its name or GUID rather than its index. Feature `enumerate` (`enumerate-static` with
  SDL 3 built in, as the release binaries are) lists the pads SDL sees: `hermir pads`,
  `--pad auto`.
- Saves: where each emulator keeps saves, memory cards and states, per platform, for all 23
  entries; a folder the player moved in the emulator's settings is followed.
  `EmulatorHandle::saves`, `hermir saves where`.
- Firmware read from settings keys (xemu's MCPX ROM, BIOS and disk image), Vita3K's firmware
  through its own installer, Ryujinx's keys.
- Vita3K on Linux, from its continuous AppImage; Windows detection at Scoop's install path for
  Eden, Flycast, melonDS, shadPS4, Supermodel, Vita3K and Xenia Canary.
- `hermir check <emulator>`: install, start, prepare, apply, start again, read back and revert,
  in a throwaway prefix and home — whether an entry works on this machine.
- The `hermir` CLI: `status`, `detect`, `install`, `update`, `remove`, `where`, `saves where`,
  `core install`, `prepare`, `players`, `config apply|revert|get|support`, `launch`, `run`,
  `profile list|create|reset|remove|where`, `pads`, `check`,
  `catalog list|show|validate|schema|resolve`, `doctor`; `--json` on every verb (one document
  per run, usage errors included) and an exit code per kind of failure, 0 to 9.
- `unsafe` is forbidden across the workspace, as a lint the build enforces.
- `Output` is public, so a consumer can pass its own `Runner` in `Options::runner`: an
  emulator's installer then runs as whichever account the consumer chooses.

### Changed

- The library's surface: every module private and its types re-exported, the public enums
  `#[non_exhaustive]`, `Hermir` `Send + Sync`, writes under a prefix lock (`Error::Locked`),
  `prepare` and `apply` returning `Result`, `Install::new` for a copy a host found itself, and
  the machine (`Env`), HTTP and process runner injectable through `Options`.
- `revert` leaves a file the emulator rewrote after hermir wrote it, and says so; `--force`
  restores it anyway.

- Player bindings moved from `players.rs` into `config/`, one adapter file per emulator, over
  the transaction that `apply` shares with every other knob; `apply_players` and
  `revert_players` remain as shortcuts, and `revert_all_players` is `revert_all`.
- Dolphin and RetroArch write XInput bindings on Windows (`XInput/<n>/Gamepad`, the `xinput`
  joypad driver) instead of the Linux evdev and udev forms; emulators keyed by SDL's GUID say
  on Windows that the USB-derived GUID is a best effort there.
- Seats past an emulator's ports (xemu's four, Supermodel's two, melonDS's one) are noted
  rather than written.
- Player bindings work for every pad kind, not only an Xbox 360 pad: Dolphin binds through its
  SDL backend, RetroArch through its `sdl2` driver, Azahar reads any pad, and the emulators
  that bind raw numbers (Eden, melonDS, mGBA, ares, Snes9x) follow the layout SDL gives the
  pad (evdev, or HIDAPI under SDL 2 or 3), read off its GUID.
- A seat without a pad is unplugged: PCSX2 and DuckStation pads typed `None`, Dolphin's
  GameCube ports and Wii Remotes off, Eden's players disconnected, Cemu's profiles removed.
  A third PS1 or PS2 player turns the multitap on.
- Gyro reaches Dolphin's Wii Remote, Cemu's GamePad, Eden, Ryujinx and Azahar.
- Every Linux launch sets SDL to report a pad's face buttons by position and to read pads
  without window focus.
- Flycast, RMG, mGBA, shadPS4, Ryujinx, ares and Snes9x (GTK) get player bindings; Flycast's
  ports past A, and RMG's profiles, hold no controller unless told.

### Fixed

- Cemu's Pro Controllers took the GamePad's mapping ids, so the d-pad landed on Home.
- Supermodel's coin keys were the Service and Test keys.

- RPCS3 on Windows keeps `config.yml`, `GuiConfigs/` and `input_configs/` under `config/`;
  the first-run answer and the bindings go there now.
- Dolphin's settings and pad files on Windows are under `Config/`, not the user directory
  itself.
- Eden's settings directory for a native (non-Flatpak) install was derived one level off.
- A UTF-8 byte-order mark at the start of a settings file no longer hides its first section.
- Emulators that offer a folder as their portable marker (Cemu, Azahar, PPSSPP, shadPS4) get
  the folder after an install, so a managed copy runs portable.
- Values quoted by RPCS3's YAML, Cemu's XML and the Qt ini are escaped, so a pad name or path
  cannot end the line it is on.
- PPSSPP's portrait and landscape layouts, Supermodel's Flatpak root and Dolphin's
  case-insensitive section names, found by the golden fixtures.

### For contributors

- Golden tests over real files: each emulator's first-start files, captured headless from its
  Flathub build (`cargo xtask capture`), with standard sessions checked for what `apply` reports,
  its output, independent parsing, stray writes, `revert`, idempotence, read-back and profiles;
  `capture --interactive` records what the emulator itself writes for the same session. A weekly
  job (`cargo xtask drift`) does it again for every new release and opens a pull request.
- cargo-deny in CI; the `enumerate` feature built and tested on Linux, Windows and macOS.