# syntax=docker/dockerfile:1.7
#
# Filebrary container image.
#
#   docker build -t filebrary .                     # small image (default, distroless)
#   docker build --target full -t filebrary:full .  # adds ffmpeg + LibreOffice
#   docker build --build-arg VARIANT=full ...       # the same, for builders that
#                                                   # take build args but no target
#   docker build --build-arg APP_VERSION=1.2.3 .    # stamp the release version
#
# The Rust stage runs on the build host's architecture and cross-compiles a
# static musl binary for the target platform with tonistiigi/xx, so multi-arch
# builds never compile Rust under QEMU emulation.

ARG RUST_VERSION=1.97
ARG NODE_VERSION=22
ARG ALPINE_VERSION=3.22
# Which image the last stage becomes: runtime (distroless) or full.
ARG VARIANT=runtime

# ---------------------------------------------------------------------------
# Web UI (platform independent output, always built natively)
# ---------------------------------------------------------------------------
FROM --platform=$BUILDPLATFORM node:${NODE_VERSION}-alpine AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# ---------------------------------------------------------------------------
# Rust server (cross-compiled, static musl)
# ---------------------------------------------------------------------------
FROM --platform=$BUILDPLATFORM tonistiigi/xx:1.6.1 AS xx

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-alpine AS build
COPY --from=xx / /
RUN apk add --no-cache clang lld musl-dev file
ARG TARGETPLATFORM
# C toolchain pieces for the target (bundled SQLite is compiled with cc).
RUN xx-apk add --no-cache musl-dev gcc
WORKDIR /src
COPY . .
COPY --from=web /src/web/dist ./web/dist
# The release version (kit-release picks it); empty keeps Cargo.toml's. The
# first `version =` in the root manifest is [workspace.package], and every
# workspace crate is named filebrary*, so Cargo.lock is stamped to match.
ARG APP_VERSION=
RUN if [ -n "$APP_VERSION" ]; then \
      awk -v v="$APP_VERSION" '!done && /^version = /{print "version = \"" v "\""; done=1; next} {print}' Cargo.toml > /tmp/c && mv /tmp/c Cargo.toml \
   && awk -v v="$APP_VERSION" '/^name = "filebrary(-[a-z0-9-]+)?"$/{f=1; print; next} f && /^version = /{print "version = \"" v "\""; f=0; next} {print}' Cargo.lock > /tmp/l && mv /tmp/l Cargo.lock \
   && grep -m1 '^version' Cargo.toml; \
    fi
ENV CARGO_PROFILE_RELEASE_STRIP=true \
    CARGO_PROFILE_RELEASE_LTO=true \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/src/target,id=filebrary-target-${TARGETPLATFORM} \
    xx-cargo build --release --locked --bin filebrary \
 && cp "target/$(xx-cargo --print-target-triple)/release/filebrary" /filebrary \
 && xx-verify --static /filebrary
# Pre-create runtime directories owned by the distroless "nonroot" user.
RUN mkdir -p /rootfs/data /rootfs/srv && chown 65532:65532 /rootfs/data /rootfs/srv

# ---------------------------------------------------------------------------
# Full image: ffmpeg + LibreOffice for previews and conversions
# ---------------------------------------------------------------------------
FROM alpine:${ALPINE_VERSION} AS full
# Tools the preview and conversion code looks up on PATH at startup:
# ffmpeg/ffprobe (video, audio), pdftoppm (PDF), soffice (Office -> PDF),
# heif-dec (HEIC).
RUN apk add --no-cache ca-certificates tzdata ffmpeg poppler-utils \
        libreoffice-common libreoffice-writer libreoffice-calc libreoffice-impress \
        libheif-tools font-noto ttf-dejavu \
 && addgroup -S -g 65532 nonroot \
 && adduser -S -D -H -u 65532 -G nonroot -h /data nonroot
COPY --from=build --chown=65532:65532 /rootfs/data /data
COPY --from=build --chown=65532:65532 /rootfs/srv /srv
COPY --from=build /filebrary /usr/local/bin/filebrary
ENV FILEBRARY_LISTEN=0.0.0.0:8080 \
    FILEBRARY_DATA_DIR=/data \
    FILEBRARY_SOURCES=files=/srv \
    HOME=/data
USER 65532:65532
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/filebrary", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/filebrary"]

# ---------------------------------------------------------------------------
# Default image: distroless static (no shell, no package manager)
# ---------------------------------------------------------------------------
FROM gcr.io/distroless/static-debian12:nonroot AS runtime
LABEL org.opencontainers.image.title="Filebrary" \
      org.opencontainers.image.description="Self-hosted file manager in one binary" \
      org.opencontainers.image.source="https://github.com/Krifmaz/filebrary" \
      org.opencontainers.image.licenses="Apache-2.0"
COPY --from=build --chown=65532:65532 /rootfs/data /data
COPY --from=build --chown=65532:65532 /rootfs/srv /srv
COPY --from=build /filebrary /usr/local/bin/filebrary
ENV FILEBRARY_LISTEN=0.0.0.0:8080 \
    FILEBRARY_DATA_DIR=/data \
    FILEBRARY_SOURCES=files=/srv
USER 65532:65532
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/filebrary", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/filebrary"]

# ---------------------------------------------------------------------------
# The image a plain `docker build` produces: runtime, or full with
# --build-arg VARIANT=full (how the release builds the -full tags).
# ---------------------------------------------------------------------------
FROM ${VARIANT} AS final
