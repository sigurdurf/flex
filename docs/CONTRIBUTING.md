# Contributing to Flex

Thanks for your interest in Flex! This guide covers local setup, the commands
CI runs, and what a good pull request looks like.

## Development setup

Flex is a single Cargo workspace (`crates/flex-protocol`,
`crates/flex-server`, `crates/flex-client`), edition 2021, Rust stable pinned in
[`rust-toolchain.toml`](../rust-toolchain.toml).

### System dependencies

```sh
# Debian / Ubuntu
sudo apt-get install ffmpeg pkg-config libsqlite3-dev \
    libgtk-4-dev libadwaita-1-dev libmpv-dev libclang-dev clang
```

- `ffmpeg` / `ffprobe` are required by the server.
- `libsqlite3-dev` is needed because `sqlx` links system SQLite.
- GTK4, libadwaita, libmpv and clang are needed to build the desktop client.
  The client only needs to **compile** in CI — there is no display, so nothing
  is launched.

### Get to a running server

```sh
cp config/flex.example.toml flex.toml   # edit the library paths
./scripts/dev.sh                        # creates flex.toml if missing, then runs
```

`scripts/dev.sh` runs `cargo run -p flex-server -- --config flex.toml`.

## Workspace commands

```sh
cargo build --workspace
cargo test --workspace

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Run individual crates with `-p`, for example
`cargo run -p flex-client -- --server http://127.0.0.1:8096`.

## Formatting and linting

CI enforces both of these, so run them before pushing:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

- Formatting is configured in [`rustfmt.toml`](../rustfmt.toml)
  (edition 2021, `max_width = 100`).
- Clippy warnings are **denied**: a warning fails the build.

## Smoke test

[`scripts/smoke.sh`](../scripts/smoke.sh) generates two tiny sample videos with
`ffmpeg`, starts a temporary server on a non-default port, and checks the scan,
item listing and a `Range` request. It is a good pre-push sanity check:

```sh
./scripts/smoke.sh
```

## Pull requests

- Keep changes focused; one logical change per PR.
- Ensure `cargo fmt --all -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` pass locally, along
  with `cargo test --workspace`.
- Add or update tests for behaviour changes.
- Update the docs that your change touches: [`README.md`](../README.md),
  [`docs/API.md`](API.md), [`docs/ARCHITECTURE.md`](ARCHITECTURE.md),
  [`docs/DEPLOYMENT.md`](DEPLOYMENT.md).
- If you change the wire format, change it in **`flex-protocol`** (the single
  source of truth) and update `docs/API.md` to match.
- Write a clear PR description: what changed, why, and how you verified it.

## License

By contributing you agree that your contributions are licensed under the MIT
license (see [`LICENSE`](../LICENSE)).
