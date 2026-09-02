# Talaria api — the pre-built package.
#
# This is the ONLY Dockerfile that compiles the Rust api, and it runs only on
# GitHub runners (.github/workflows/api-package.yml). The root Dockerfile
# consumes the result with `COPY --from`, so no box that builds an app image
# — a GitHub runner, an operator's machine, a customer VM (Dokploy builds
# from a checkout on every push to main) — ever needs cargo, a C toolchain,
# or the minutes aws-lc-sys costs. The binary is musl-static (the app
# runtime is bare alpine), and the image doubles as a standalone api
# container: `docker run` it with a DATABASE_URL and it serves.
#
# Build from the REPO ROOT (the context needs api/; .dockerignore keeps
# target/ out of it):
#
#   docker build --network=host -f api/package.Dockerfile -t talaria-api:local .
#
# Then hand it to the app image (docs/CONTAINER.md → The api binary):
#
#   docker build --build-arg TALARIA_API_IMAGE=talaria-api:local .

# ── build ────────────────────────────────────────────────────────────────────
# Same 1.97.1 the devboxes and CI pin via api/rust-toolchain.toml, and the
# alpine variant so the default target is musl — the artifact is static-native
# and drops into the app's alpine runtime with no interpreter, no extra
# packages.
#
# build-base + cmake: aws-lc-sys — the TLS stack's C half, the same reason
# the devbox image carries build-essential — compiles C here, in CI, once.
FROM docker.io/library/rust:1.97.1-alpine3.21 AS build
RUN apk add --no-cache build-base cmake

WORKDIR /repo
COPY api ./
# Cache mounts carry the registry and target dir across CI runs, so a source
# change recompiles the crate, not the dependency tree. The binary is copied
# OUT of the target mount before the layer closes — a cache mount's contents
# do not survive into the image.
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/repo/api/target \
    cargo build --release --locked \
 && cp target/release/talaria-api /talaria-api

# ── package ──────────────────────────────────────────────────────────────────
# Not scratch: the api verifies outbound TLS through rustls-platform-verifier,
# which reads the system root store, so the package carries ca-certificates —
# without it every provider call (every LLM request) fails at runtime.
# Everything else the binary needs is compiled in.
FROM docker.io/library/alpine:3.21

# Release identity, same contract as the root Dockerfile: passed by the
# workflow (REVISION is the commit the package was built from — with the
# sha-<sha12> tag, that is the package's whole version story), defaulted so a
# local build works with no args.
ARG REVISION=unknown
ARG CREATED=unknown
LABEL org.opencontainers.image.title="Talaria API" \
      org.opencontainers.image.description="The Rust api — every /api/* route except the four permanent TS residents" \
      org.opencontainers.image.source="https://github.com/outcrop-labs/talaria" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.created="${CREATED}"

RUN apk add --no-cache ca-certificates
COPY --from=build /talaria-api /usr/local/bin/talaria-api
# Exec form: the binary is PID 1 and drains on SIGTERM, same shutdown
# contract server-entry.js relies on when it spawns a copy from the app image.
ENTRYPOINT ["/usr/local/bin/talaria-api"]
