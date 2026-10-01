# Changelog

Notable changes to `hermir` and `hermir-cli`, which share a version. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org), where a 0.x minor may break.

## [Unreleased]

The first release: P0 of the [design](docs/design.md), *get*.

### Added

- A catalog of 20 emulators, embedded in the library, parsed strictly, with a JSON Schema and
  per-OS channels, detection rules, config roots and firmware folders.
- Install, update and remove from each emulator's own channel — Flatpak on Linux; GitHub
  releases or a pinned official URL on Windows — verified against the published sha256 and
  resumable. An update keeps shipped files the user edited and never touches config, saves or
  firmware.
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
- The `hermir` CLI: `status`, `detect`, `install`, `update`, `remove`, `where`, `core install`,
  `prepare`, `players`, `config apply|revert|support`,
  `catalog list|show|validate|schema|resolve`, `doctor`; `--json` on every verb and an exit
  code per kind of failure.
- `unsafe` is forbidden across the workspace, as a lint the build enforces.

### Changed

- Player bindings moved from `players.rs` into `config/`, one adapter file per emulator, over
  the transaction that `apply` shares with every other knob; `apply_players` and
  `revert_players` remain as shortcuts, and `revert_all_players` is `revert_all`.
- Dolphin and RetroArch write XInput bindings on Windows (`XInput/<n>/Gamepad`, the `xinput`
  joypad driver) instead of the Linux evdev and udev forms; emulators keyed by SDL's GUID say
  on Windows that the USB-derived GUID is a best effort there.
- Seats past an emulator's ports (xemu's four, Supermodel's two, melonDS's one) are noted
  rather than written.

### Fixed

- RPCS3 on Windows keeps `config.yml`, `GuiConfigs/` and `input_configs/` under `config/`;
  the first-run answer and the bindings go there now.
- Dolphin's settings and pad files on Windows are under `Config/`, not the user directory
  itself.
- Eden's settings directory for a native (non-Flatpak) install was derived one level off.
- A UTF-8 byte-order mark at the start of a settings file no longer hides its first section.