# Changelog

Notable changes to `hermir` and `hermir-cli`, which share a version. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org), where a 0.x minor may break.

## [Unreleased]

The first release: P0 of the [design](docs/design.md), *get*.

### Added

- A catalog of 18 emulators, embedded in the library, parsed strictly, with a JSON Schema and
  per-OS channels, detection rules, config roots and firmware folders.
- Install, update and remove from each emulator's own channel — Flatpak on Linux; GitHub
  releases or a pinned official URL on Windows — verified against the published sha256 and
  resumable. An update keeps shipped files the user edited and never touches config, saves or
  firmware.
- Detection of copies the user installed: `PATH`, Flatpak (user and system), known install
  paths, portable markers.
- libretro cores from the buildbot into RetroArch's cores directory.
- The `hermir` CLI: `status`, `detect`, `install`, `update`, `remove`, `where`,
  `core install`, `catalog list|show|validate|schema|resolve`, `doctor`; `--json` on every verb
  and an exit code per kind of failure.
