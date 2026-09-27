# hermir

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
hermir catalog resolve --all        where every channel points, without downloading
hermir doctor
```

Every verb takes `--json`. The catalog is data (`catalog/`), reviewed by pull request: each
entry points at the emulator's own release channel and nothing else, downloads are verified
against the digest the release page publishes, and no Switch emulator carries an install
channel.

The design — the model, the facade, how configuration is patched and reverted, what comes
next — is in [`docs/design.md`](docs/design.md). This is the first step of it: get, find,
update, remove.

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

## Licence

MIT or Apache-2.0, at your option. The emulators have their own licences; the catalog names
each.
