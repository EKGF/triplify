---
title: "Install"
description: "Install the triplify command with Homebrew, uv, pipx, pip or Cargo on Linux, macOS or Windows."
permalink: /install/
nav_order: 2
grapharch:
  subject: urn:triplify:website:page:install
---

# Install

Triplify is a single native executable, `triplify`, written in Rust. The
package is named `triplify` on both PyPI and crates.io.

## Install with Homebrew

On macOS (Apple silicon) and Linux, install from the [EKGF tap](https://github.com/EKGF/homebrew-tap):

```sh
brew install ekgf/tap/triplify
```

The formula is available from release 0.2.1 on.

## Install from PyPI

Install the native executable in an isolated tool environment with uv:

```sh
uv tool install triplify
```

You can also use pipx or pip:

```sh
pipx install triplify
pip install triplify
```

The wheels contain the native executable, need no Python dependencies and
download nothing at runtime. They cover:

- Linux (glibc 2.28 or later) on x86-64 and ARM64;
- macOS 11 or later on Apple silicon;
- Windows on x86-64 and ARM64.

There is no wheel for Intel Macs. On one, install with Cargo instead.

## Install with Cargo

With [cargo-binstall](https://github.com/cargo-bins/cargo-binstall), Cargo
downloads the prebuilt executable from the GitHub release instead of
compiling it:

```sh
cargo binstall triplify
```

The prebuilt executables cover the same platforms as the wheels. On any other
platform, and for releases before 0.2.1 (which have no prebuilt executables),
cargo-binstall compiles from source, as this does:

```sh
cargo install triplify --locked
```

Both install from [crates.io](https://crates.io/crates/triplify) into
`~/.cargo/bin` and work with stable Rust.

## Build from source

```sh
git clone https://github.com/EKGF/triplify
cd triplify
cargo install --path . --locked
```

Run this inside the checkout: rustup then installs the pinned nightly
toolchain from `rust-toolchain.toml` automatically.

## Check the installation

```sh
triplify --version
triplify --help
```

## Run Triplify

```sh
triplify ./data -o raw.nq.gz
```

See [Usage](usage.md) for input formats and command-line options.
For source builds and local wheel packaging, see [Contributing](contributing.md#build-from-source).
