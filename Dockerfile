# The deployment artifact: the Telegram webhook and dispatcher, built for the machine they run on.
#
# `ratatoskr-workspace/docs/DEPLOYMENT_TARGET.md` fixes the target as one Raspberry Pi 5: `aarch64`,
# Debian 12, glibc 2.36. A binary linked against a newer glibc does not start against an older one,
# so the builder is Debian 12 (`bookworm`) as well, and the runtime stage is the same release.
#
# Three stages, in this order: `builder`, `export` and `runtime`. A plain `docker build` builds the
# LAST stage, so `runtime` stays last; `deploy/tests/artifact_smoke.sh` runs both binaries in it on
# the arm64 CI runner. `export` holds only the two binaries, and is what `ratatoskr-workspace`'s
# reusable-release-artifact workflow publishes for each commit on main:
#
#     docker buildx build --platform linux/arm64 --target export --output type=local,dest=out .
#
# Both base images are pinned by digest as well as by tag, so a re-pushed tag cannot change the
# artifact without a commit here.

# ---------------------------------------------------------------------------------------------
# builder
# ---------------------------------------------------------------------------------------------
# The same toolchain as `rust-toolchain.toml`, so cargo does not download a second one.
FROM rust:1.97.0-slim-bookworm@sha256:6d220bf85c74e842a79da63997af8d2e74455c0b8847d8bb3a5888572334991d AS builder

# `git` for the contracts git dependencies pinned to a SHA; a C compiler is already in the image.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates git pkg-config \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# The commit this artifact was built from, for the operator plane's version report.
ARG RATATOSKR_GIT_SHA=unknown
ENV RATATOSKR_GIT_SHA=${RATATOSKR_GIT_SHA}

COPY . .

# `--locked`, as the gate builds. The registry, the git checkouts and `target/` are BuildKit cache
# mounts; a cache mount is not part of the layer, so the binaries are copied to `/out` in the same
# step.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked \
      -p ratatoskr-telegram-webhook -p ratatoskr-telegram-dispatcher \
 && mkdir /out \
 && cp target/release/ratatoskr-telegram-webhook target/release/ratatoskr-telegram-dispatcher /out/

# ---------------------------------------------------------------------------------------------
# export
# ---------------------------------------------------------------------------------------------
# Only the binaries: the local exporter writes this stage's whole filesystem, and all of it is
# published. The release-artifact check refuses a file that no unit starts.
FROM scratch AS export
COPY --from=builder /out/ /

# ---------------------------------------------------------------------------------------------
# runtime
# ---------------------------------------------------------------------------------------------
FROM debian:12-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587 AS runtime

# The trust store the Bot API client verifies `api.telegram.org` against.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --no-create-home --shell /usr/sbin/nologin ratatoskr

COPY --from=builder /out/ratatoskr-telegram-webhook /out/ratatoskr-telegram-dispatcher /usr/local/bin/

ARG RATATOSKR_GIT_SHA=unknown
ENV RATATOSKR_GIT_SHA=${RATATOSKR_GIT_SHA}

# No default command: the image carries two roles, and every caller names its binary.
USER ratatoskr
