//! Assert the two Rust toolchain pins in this repository name one version.
//!
//! `MODULE.bazel` pins the toolchain `rules_rust` downloads for the Bazel graph.
//! `rust-toolchain.toml` pins the toolchain rustup gives Cargo, clippy, and
//! rustfmt. Nothing else ties them together, so they can drift silently and
//! leave the two build paths compiling this crate tree with different
//! compilers. Both files are read at compile time through `compile_data`.

const MODULE_BAZEL: &str = include_str!("../MODULE.bazel");
const RUST_TOOLCHAIN_TOML: &str = include_str!("../rust-toolchain.toml");

/// Read the `channel = "<version>"` value out of `rust-toolchain.toml`.
fn rustup_channel() -> &'static str {
    let value = RUST_TOOLCHAIN_TOML
        .lines()
        .find_map(|line| line.trim().strip_prefix("channel"))
        .and_then(|rest| rest.trim_start().strip_prefix('='))
        .expect("rust-toolchain.toml must declare a channel");
    quoted(value).expect("rust-toolchain.toml channel must be a quoted version")
}

/// Read the single `rust.toolchain(versions = ["<version>"])` pin.
fn bazel_rust_version() -> &'static str {
    let list = MODULE_BAZEL
        .split_once("rust.toolchain(")
        .expect("MODULE.bazel must call rust.toolchain(...)")
        .1
        .split_once("versions")
        .expect("rust.toolchain(...) must pin versions")
        .1
        .split_once('[')
        .expect("rust.toolchain versions must be a list")
        .1
        .split_once(']')
        .expect("rust.toolchain versions list must be closed")
        .0;
    assert!(
        !list.contains(','),
        "expected exactly one pinned Bazel Rust toolchain, found: {list}"
    );
    quoted(list).expect("rust.toolchain versions must hold a quoted version")
}

/// Return the contents of the first double-quoted run in `text`.
fn quoted(text: &str) -> Option<&str> {
    text.trim()
        .strip_prefix('"')
        .and_then(|rest| rest.split('"').next())
}

#[test]
fn bazel_and_rustup_pin_the_same_rust_version() {
    let bazel = bazel_rust_version();
    let rustup = rustup_channel();
    assert_eq!(
        bazel, rustup,
        "MODULE.bazel pins Rust {bazel} but rust-toolchain.toml pins {rustup}"
    );
}
