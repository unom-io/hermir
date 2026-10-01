# Contributing

Pull requests are welcome; a new emulator is the most common one and is mostly data. Read
[`docs/design.md`](docs/design.md) first for anything beyond a catalog entry.

## Checks

What CI runs, on Linux, Windows and macOS:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -q -p hermir-cli -- catalog validate crates/hermir/catalog
cargo package --workspace          # Linux only: builds each crate as crates.io will
```

The toolchain is pinned in `rust-toolchain.toml`. Commits follow
[Conventional Commits](https://www.conventionalcommits.org) (`feat(catalog): …`, `fix: …`).
`unsafe` is forbidden by a workspace lint; a change that needs it needs a dependency that
keeps it behind a safe API instead.

## Adding an emulator

The catalog lives in [`crates/hermir/catalog/`](crates/hermir/catalog) — inside the library
crate, because it is embedded at build time and a published crate carries nothing from outside
its own directory.

1. **Write `crates/hermir/catalog/emulators/<id>.json`.** Start from an entry with the same
   channels (`pcsx2.json` for Flatpak + GitHub release, `dolphin.json` for a pinned URL). The
   shape is [`catalog/schema/entry.schema.json`](crates/hermir/catalog/schema/entry.schema.json);
   point your editor at it. Parsing is strict: an unknown key is an error.
2. **Embed it:** add `(id, include_str!(…))` to `EMULATORS` in
   [`crates/hermir/src/catalog.rs`](crates/hermir/src/catalog.rs), in alphabetical order. A test
   fails if a file is missing from the list.
3. **Rank it:** add the id to `emulators` of each platform it plays in
   [`platforms.json`](crates/hermir/catalog/platforms.json), in order of preference. A platform
   that is new needs its entry there too, with its aliases.
4. **Resolve it** against the live release pages, for each OS it has a channel on:
   ```sh
   cargo run -q -p hermir-cli -- --os windows catalog resolve <id>
   cargo run -q -p hermir-cli -- --os linux catalog resolve <id>
   ```
   Exactly one asset must match; `GITHUB_TOKEN` avoids the anonymous rate limit.
5. **Describe its settings** in a `config` block, as far as you know them (below). An entry
   without one still installs and detects; `hermir config support <id>` then says "not
   described yet" for each knob, which is honest and fine for a first PR.
6. **Install it for real** on the OS you can, and say in the PR which one:
   `hermir --prefix /tmp/h install <id>` then `hermir --prefix /tmp/h where <id>`.

The rules a reviewer holds an entry to:

- **The emulator's own channel, nothing else.** Flathub on Linux; the project's GitHub releases
  or its official download on Windows. No mirrors, no rehosting, no third-party builds.
- **Verifiable downloads.** GitHub releases publish a sha256 per asset. A `url` channel carries
  `sha256` when the project publishes one.
- **No Switch emulator carries an install channel.** Such an entry has `no_install` and is
  detect-and-configure only.
- **`license` is the emulator's SPDX expression**, taken from its repository.

A weekly job resolves every channel, so a renamed asset shows up as a red
[catalog dry run](https://github.com/unom-io/hermir/actions/workflows/catalog-dry-run.yml);
fixing one is a small PR to the entry's `asset` filter.

## Describing settings

`hermir config apply` is driven by the entry's `config` block, so most of configuration is a
catalog change too:

```jsonc
"config": {
  "files": { "main": { "path": "inis/PCSX2.ini", "format": "ini" } },
  "knobs": {
    "video.fullscreen": { "file": "main", "section": "UI", "key": "StartFullscreen", "bool": ["true", "false"] },
    "video.scale":      { "file": "main", "section": "EmuCore/GS", "key": "upscale_multiplier", "scale": { "multiplier": { "max": 8 } } },
    "video.aspect":     { "file": "main", "section": "EmuCore/GS", "key": "AspectRatio",
                          "values": { "auto": "Auto 4:3/3:2", "4:3": "4:3", "16:9": "16:9", "stretch": "Stretch" } },
    "region":           { "unsupported": "the BIOS decides the region; PCSX2 has no override" }
  },
  "players": { "adapter": "pcsx2" }      // or { "automatic": "…" } or { "unsupported": "…" }
}
```

- **Files** are relative to the config root (`roots`), per OS where the layout differs
  (`"path": { "linux": "Dolphin.ini", "windows": "Config/Dolphin.ini" }`). `{config}/` at the
  start means the settings directory beside a data root (`~/.config/<x>` for
  `~/.local/share/<x>`). Formats: `ini` (also flat TOML and RetroArch's cfg), `qt` (Qt's ini,
  which needs a `key\default=false` companion), `yaml` (two levels), `bml`, `xml` (with the
  document element as `root`), `json` (a key of the root, or of one object under it).
- **Knobs** are `video.fullscreen`, `video.scale`, `video.vsync`, `video.aspect`, `region`.
  A bool knob takes `bool: [true, false]` in the emulator's spelling; `video.scale` takes a
  `scale` (`multiplier`, `percent`, `lines` with a base, or a `map` from `n`); the others a
  `values` table from the neutral value to the emulator's literal, quotes included where the
  format wants them. A neutral value left out of the table is reported unsupported, which is
  the right answer when the emulator has no such choice. `also` writes companion keys
  (`{value}` is the knob's own value); `note` makes the knob partial and says what to know.
  Prefer an honest `unsupported` with a reason over a guess: the note is what a UI shows.
- **Players** name an adapter in `crates/hermir/src/config/adapters/`, or say the emulator
  maps SDL pads itself, or why there is nothing to write. A new adapter is one file there, a
  line in `ADAPTERS` and the `plan` match, and the catalog line that names it; a test fails
  when any of the three is missing. Adapters take the catalog's files by name (`cx.file("main")`)
  and never parse text themselves.

Every spelling here should come from the emulator's own config file, not memory; a fixture
in the PR (the fresh-install file with the knob toggled once in the emulator's UI) is the
best evidence.

## Changing the model

`crates/hermir/src/model.rs` is the catalog's schema. After changing it, regenerate the schema
file (a test compares them):

```sh
cargo run -q -p hermir-cli -- catalog schema > crates/hermir/catalog/schema/entry.schema.json
```

## Releasing

[`docs/publishing.md`](docs/publishing.md).
