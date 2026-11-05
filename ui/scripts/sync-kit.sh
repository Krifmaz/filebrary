#!/usr/bin/env sh
# Copies the kit's colour tokens and component stylesheet at the tag ui/Cargo.toml pins into ui/static/css/.
# Run it after changing the kit tag; `cargo test` fails while kit.css differs from the pinned kit's.
set -eu
cd "$(dirname "$0")/.."
tag=$(sed -n 's/^kit-leptos = .*tag = "\([^"]*\)".*/\1/p' Cargo.toml)
[ -n "$tag" ] || { echo "No kit tag found in ui/Cargo.toml" >&2; exit 1; }
raw="https://raw.githubusercontent.com/Krifmaz/kit/$tag"
curl -fsSL "$raw/design/out/tokens.css" -o static/css/tokens.css
curl -fsSL "$raw/crates/kit-leptos/style/kit.css" -o static/css/kit.css
echo "Copied tokens.css and kit.css from kit $tag"
