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
- The `hermir` CLI: `status`, `detect`, `install`, `update`, `remove`, `where`, `core install`,
  `prepare`, `catalog list|show|validate|schema|resolve`, `doctor`; `--json` on every verb and
  an exit code per kind of failure.