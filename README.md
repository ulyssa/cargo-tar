# cargo-tar

[![Build Status](https://github.com/ulyssa/cargo-tar/actions/workflows/ci.yml/badge.svg)](https://github.com/ulyssa/cargo-tar/actions?query=workflow%3ACI+)
[![License: Apache 2.0](https://img.shields.io/crates/l/cargo-tar.svg?logo=apache)](https://crates.io/crates/cargo-tar)
[![#cargo-tar:0x.badd.cafe](https://img.shields.io/badge/matrix-%23cargo-tar:0x.badd.cafe-blue)](https://matrix.to/#/#cargo-tar:0x.badd.cafe)
[![Latest Version](https://img.shields.io/crates/v/cargo-tar.svg?logo=rust)](https://crates.io/crates/cargo-tar)
[![Docs Status](https://docs.rs/cargo-tar/badge.svg)](https://docs.rs/crate/cargo-tar/)

## About

This is a [Cargo] helper command which can automatically create release
tarballs from your Cargo projects, and provides an interface similar to
[cargo-deb] and [cargo-generate-rpm].

Using this command, you can build `.tar`, `.tar.gz`, and `.tar.bz2` release
archives from a Cargo project, by declaring the layout in your `Cargo.toml`.

_You may want to [see this page as it was when the latest version was published][crates-io-cargo-tar]._

## Install

```sh
cargo install --locked cargo-tar
```

## Usage

```console
$ cargo tar
target/tar/iamb-0.0.13-x86_64-unknown-linux-gnu.tar.gz (2.1 MiB)
```

## Declaring assets

```toml
[package.metadata.tar]
format = "tar.gz"
bin-dir = "bin"
assets = [
    # Manual pages:
    { source = "docs/iamb.1", dest = "man/iamb.1", mode = "644" },
    { source = "docs/iamb.5", dest = "man/iamb.5", mode = "644" },
    # Packaged themes:
    { source = "themes/*.toml", dest = "themes/", mode = "644" },
    # Other assets:
    { source = "iamb.desktop", dest = "assets/iamb.desktop", mode = "644" },
    { source = "docs/iamb.svg", dest = "assets/iamb.svg", mode = "644" },
    { source = "config.example.toml", dest = "config.example.toml", mode = "644" },
]
```

The package's `[[bin]]` targets are added automatically, so they need no `assets`
entry.

## License

`cargo-tar` is released under the [MIT License].

[MIT License]: https://github.com/ulyssa/cargo-tar/blob/master/LICENSE
[crates-io-cargo-tar]: https://crates.io/crates/cargo-tar
[Cargo]: https://doc.rust-lang.org/cargo/
[cargo-deb]: https://crates.io/crates/cargo-deb
[cargo-generate-rpm]: https://crates.io/crates/cargo-generate-rpm
[SOURCE_DATE_EPOCH]: https://reproducible-builds.org/docs/source-date-epoch/
