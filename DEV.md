# Development

## Commands

```bash
maturin develop && pytest -q
ship-rs-build
```

## Layout

The root package is a pure Rust crate, published to crates.io. Rust users who depend on it never build PyO3. The Python extension is the `py` package, which maturin builds through `[tool.maturin].manifest-path` and which is never published. It reports the crate version only and doesn't depend on the root crate: the Python client is pure Python, in `python/fastgpui`, and linking the server into the extension would build GPUI into it.

`cargo test` runs the story test in `examples/counter.rs`, which serves the counter example in process. `pytest -q` builds the same example and drives it over the socket.

## Versioning

The canonical version lives in `[workspace.package]` in `Cargo.toml`, which both crates inherit. `pyproject.toml` gets the Python package version from Cargo via `dynamic = ["version"]`.

## Build profiles

uv builds and `maturin develop --release` use the incremental `release` profile for fast local iteration. `cargo test` and `cargo develop` build the `test` profile, which inherits `release`. CI builds distributed wheels with `dist`, which enables full LTO and one codegen unit, disables incremental compilation, and strips the result.

## Release

1. Run `maturin develop && pytest -q`.
2. Confirm the release version in `Cargo.toml` (`[workspace.package].version`).
3. Run `ship-release`.

`ship-release` pushes a `v<version>` tag, which starts the release workflow in GitHub Actions, then bumps the version.
