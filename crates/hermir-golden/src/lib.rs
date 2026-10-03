//! Golden tests for hermir: real config files, written by each emulator on its first start,
//! patched through hermir's public API and reverted. `tests/golden.rs` runs every fixture under
//! `fixtures/`; `cargo xtask capture` makes them. See `fixtures/README.md`.
pub mod check;
pub mod fixture;
pub mod oracle;
