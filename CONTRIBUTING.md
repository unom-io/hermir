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
5. **Install it for real** on the OS you can, and say in the PR which one:
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

## Changing the model

`crates/hermir/src/model.rs` is the catalog's schema. After changing it, regenerate the schema
file (a test compares them):

```sh
cargo run -q -p hermir-cli -- catalog schema > crates/hermir/catalog/schema/entry.schema.json
```

## Releasing

[`docs/publishing.md`](docs/publishing.md).
