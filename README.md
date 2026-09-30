# hermir

[![crates.io](https://img.shields.io/crates/v/hermir.svg)](https://crates.io/crates/hermir)
[![docs.rs](https://img.shields.io/docsrs/hermir)](https://docs.rs/hermir)
[![ci](https://github.com/unom-io/hermir/actions/workflows/ci.yml/badge.svg)](https://github.com/unom-io/hermir/actions/workflows/ci.yml)
[![catalog dry run](https://github.com/unom-io/hermir/actions/workflows/catalog-dry-run.yml/badge.svg)](https://github.com/unom-io/hermir/actions/workflows/catalog-dry-run.yml)
[![licence](https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-blue.svg)](#licence)

One interface for managing emulators: install them, find them, configure them, build their
launch, know where their firmware and saves live. A Rust library, and a CLI that is the same
thing for every other language.

*hermir* is Icelandic for "emulator", from *herma*, to mimic.

```
hermir status                       managed and detected emulators
hermir install pcsx2                Flatpak on Linux, the official portable build on Windows
hermir update --all
hermir where dolphin                exe and config root of the best copy on this machine
hermir detect --json                what the user installed, as data
hermir core install snes9x          a libretro core into RetroArch's cores directory
hermir prepare pcsx2 --platform ps2 --firmware scph39001.bin
hermir players pcsx2                     # the Xbox pad in seat 1, into PCSX2's own bindings
hermir players pcsx2 --revert            # the player's settings back
                                    first-run answers and the BIOS, on every copy
hermir catalog resolve --all        where every channel points, without downloading
hermir doctor
```

Every verb takes `--json`. The catalog is data
([`crates/hermir/catalog/`](https://github.com/unom-io/hermir/tree/main/crates/hermir/catalog)),
reviewed by pull request: each entry points at the emulator's own release channel and nothing
else, downloads are verified against the digest the release page publishes, and no Switch
emulator carries an install channel.

The design — the model, the facade, how configuration is patched and reverted, what comes
next — is in [`docs/design.md`](https://github.com/unom-io/hermir/blob/main/docs/design.md).
This is the first step of it: get, find, update, remove.

## Install

```sh
cargo install hermir-cli            # the binary is `hermir`
cargo binstall hermir-cli           # the release binary, no build
```

Or take a binary from [Releases](https://github.com/unom-io/hermir/releases) (Linux x86-64,
Windows x86-64, macOS arm64). On Linux, installs go through Flatpak, so `flatpak` must be on
`PATH`; `hermir doctor` says what is missing. Set `GITHUB_TOKEN` to lift the GitHub API's
anonymous rate limit.

The library is `cargo add hermir`.

## Emulators

| Emulator | Plays | Linux | Windows |
|---|---|---|---|
| [Azahar](https://github.com/azahar-emu/azahar) | Nintendo 3DS | Flatpak | release build |
| [Cemu](https://github.com/cemu-project/Cemu) | Wii U | Flatpak | release build |
| [Dolphin](https://dolphin-emu.org) | GameCube, Wii | Flatpak | release build |
| [DOSBox Staging](https://github.com/dosbox-staging/dosbox-staging) | DOS | Flatpak | release build |
| [DuckStation](https://github.com/stenzek/duckstation) | PlayStation | Flatpak | release build |
| [Flycast](https://github.com/flyinghead/flycast) | Dreamcast | Flatpak | release build |
| [melonDS](https://github.com/melonDS-emu/melonDS) | Nintendo DS | Flatpak | release build |
| [mGBA](https://github.com/mgba-emu/mgba) | Game Boy, Game Boy Color, Game Boy Advance | Flatpak | release build |
| [PCSX2](https://github.com/PCSX2/pcsx2) | PlayStation 2 | Flatpak | release build |
| [PPSSPP](https://github.com/hrydgard/ppsspp) | PlayStation Portable | Flatpak | release build |
| [RetroArch](https://www.retroarch.com) | 37 systems, cores via `hermir core install` | Flatpak | release build |
| [RPCS3](https://github.com/RPCS3/rpcs3) | PlayStation 3 | Flatpak | release build |
| [ScummVM](https://www.scummvm.org) | ScummVM games | Flatpak | release build |
| [shadPS4](https://github.com/shadps4-emu/shadPS4) | PlayStation 4 | release build | release build |
| [Supermodel](https://github.com/trzy/Supermodel) | Sega Model 3 | release build | release build |
| [Vita3K](https://github.com/Vita3K/Vita3K) | PlayStation Vita | — | release build |
| [xemu](https://github.com/xemu-project/xemu) | Xbox | Flatpak | release build |
| [Xenia Canary](https://github.com/xenia-canary/xenia-canary) | Xbox 360 | release build | release build |
| Eden, Ryujinx | Nintendo Switch | detect only | detect only |

`hermir catalog list` is the live list; `hermir catalog show <id>` is one entry in full. A new
emulator is a JSON file and a pull request — see
[CONTRIBUTING.md](https://github.com/unom-io/hermir/blob/main/CONTRIBUTING.md).

## Library

```rust
use hermir::{Hermir, Options, progress::Quiet};

let h = Hermir::open(Options::default())?;
let pcsx2 = h.emulator("pcsx2")?;
let row = pcsx2.install(&Quiet)?;      // Installed { exe, version, release, digest, .. }
for i in h.installs()? {               // managed + detected, one per exe
    println!("{} {:?} {}", i.emulator, i.kind, i.exe);
}
```

Every type is serde and JSON Schema, so the CLI's `--json` is the same contract as the library.
The API reference is on [docs.rs](https://docs.rs/hermir).

## Licence

MIT or Apache-2.0, at your option. The emulators have their own licences; the catalog names
each.
