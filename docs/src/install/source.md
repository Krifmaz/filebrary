# Building from source

You need:

- Rust 1.97 (pinned in `rust-toolchain.toml`; rustup installs it automatically)
- Node.js 22 and npm

```sh
git clone https://github.com/Krifmaz/filebrary
cd filebrary

# 1. Build the web interface (embedded into the binary at compile time).
npm --prefix web ci
npm --prefix web run build

# 2. Build the server.
cargo build --release

./target/release/filebrary   # serves ./data/files on 127.0.0.1:8080
```

For a fully static Linux binary, build for the musl target:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl --bin filebrary
```

## Running the checks

These are the same checks CI runs on Linux, macOS and Windows:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
npm --prefix web run lint
npm --prefix web run build
```

## Building these docs

```sh
cargo install mdbook
mdbook serve docs
```
