# Releasing

A release is a tag. Pushing `v<version>` runs [`release.yml`](../.github/workflows/release.yml):
it builds the `hermir` binary for Linux x86-64, Windows x86-64 and macOS arm64 onto a GitHub
release and, once enabled, publishes both crates to crates.io.

| crate | what | from |
|---|---|---|
| [`hermir`](https://crates.io/crates/hermir) | the library, with the catalog embedded | `crates/hermir` |
| [`hermir-cli`](https://crates.io/crates/hermir-cli) | the `hermir` binary | `crates/hermir-cli` |

Both share one version, `[workspace.package].version` in the root `Cargo.toml`.

## Every release

1. Set the version in `Cargo.toml` twice: `[workspace.package].version`, and the `version` of
   `hermir` under `[workspace.dependencies]`, which is what `hermir-cli` asks crates.io for.
2. Move `Unreleased` in [`CHANGELOG.md`](../CHANGELOG.md) under the new version.
3. `cargo publish --workspace --dry-run` — packages each crate and builds it from its tarball,
   `hermir` first.
4. Commit, tag `v<version>`, push the tag.

A published version is permanent: it can be yanked (`cargo yank`), never replaced. Release
archive names — `hermir-v<version>-<target>.tar.gz` with the binary at the root — are what
`cargo binstall hermir-cli` downloads (`[package.metadata.binstall]` in
`crates/hermir-cli/Cargo.toml`); renaming them means changing that too.

## Enabling crates.io publishing (once)

The publish job uses [trusted publishing](https://crates.io/docs/trusted-publishing): crates.io
trusts this repository's `release.yml` and hands it a short-lived token, so no secret is
stored. A trusted publisher is configured on a crate that exists, so the first version goes
up by hand:

1. On crates.io, *Account Settings → API Tokens*: a token scoped to `publish-new` and
   `publish-update`, for crates `hermir*`, short expiry.
2. From a clean checkout of the release tag: `cargo login`, then `cargo publish --workspace`.
3. For **each** crate, *crate → Settings → Trusted Publishing → Add*: GitHub, owner
   `unom-io`, repository `hermir`, workflow `release.yml`.
4. Optionally add a second owner, so the crates outlive one account:
   `cargo owner --add github:unom-io:<team> hermir hermir-cli`.
5. In the repository, *Settings → Secrets and variables → Actions → Variables*: add
   `PUBLISH_CRATES` = `true`.
6. Revoke the token.

From then on, every `v*` tag publishes both crates after the binaries built.
