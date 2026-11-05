# Releasing

Every merge to `main` is a release. Nobody tags, bumps a version or pushes a
release branch by hand.

## What happens on a merge

The **Release** workflow (`.github/workflows/release.yml`) runs on every push
to `main`, through the shared workflows of
[Krifmaz/kit](https://github.com/Krifmaz/kit) (pinned to a kit tag):

1. **test** (`kit-ci`): `cargo fmt --check`, clippy with `-D warnings` and the
   whole test suite.
2. **prepare** (`kit-release`): picks the version. By default it is the newest
   release's PATCH + 1 (`v0.2.0` → `v0.2.1`).
3. **binaries**: the server with the web UI embedded, for Linux (static musl,
   x86_64 and aarch64), macOS (Intel and Apple silicon) and Windows, as
   `filebrary-<version>-<target>.tar.gz` (`.zip` on Windows).
4. **docker** (`kit-docker`): multi-arch (amd64, arm64) images on GHCR:
   `ghcr.io/krifmaz/filebrary:<version>` and `:latest`, plus the image with
   ffmpeg and LibreOffice as `:<version>-full` and `:latest-full`.
5. **publish** (`kit-release`): the GitHub release `v<version>` (always a
   normal release marked Latest, never a pre-release) with every file above
   and a `SHA256SUMS` over them.

Every build is stamped with the chosen version (`APP_VERSION`): the
binaries, the images, `/readyz` and the OpenAPI document all report it, so
`Cargo.toml` does not need a version-bump commit.

## Minor and major releases

A pull request raises MINOR or MAJOR by adding the notes for that version as
`docs/releases/<version>.md` (for example `docs/releases/0.3.0.md`). That file
is also the release's notes; without one, the notes are the list of commits
since the previous release.

## By hand

Actions → **Release** → *Run workflow* releases the current `main` and lets you
choose the bump (`auto`, `patch`, `minor`, `major`) or type the version.

## Rules

* Runs are queued one at a time, never in parallel.
* A release is only published when the tests, every binary and both images
  built. A failed run publishes nothing; fix it and merge again (or run the
  workflow by hand).
* Pull requests run the **CI** workflow: the same kit checks on Linux and
  macOS, Windows, the web UI, the storage backends, the docs book and a
  Docker build that is started and probed.

## Building locally

```sh
docker build -t filebrary .                              # default image
docker build --build-arg VARIANT=full -t filebrary:full . # with ffmpeg + LibreOffice
docker build --build-arg APP_VERSION=1.2.3 -t filebrary . # stamp a version
```
