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
#
# ── caching, and why this file is shaped like this ───────────────────────────
# The workflow builds with buildx's REGISTRY cache (--cache-from/--cache-to
# type=registry, mode=max, the :buildcache tag), because a hosted runner has
# no state of its own and registry cache — unlike the gha backend — works
# from a workflow_call (release.yml calls this build for every publish).
# mode=max exports the INTERMEDIATE layers, which only pays off if they are
# keyed on something stabler than "any file changed". That is the three
# stages below, the standard cargo-chef shape:
#
#   planner  copies the full source, reduces it to recipe.json — a digest of
#            the manifests and nothing else.
#   deps     cooks the dependency tree from recipe.json ALONE. Its layers
#            bust only when a manifest moves (a dep added, a path dep
#            renamed), so the ~400 third-party crates build once per
#            dependency change and ride the cache forever after.
#   build    copies the real source and builds the workspace. A source-only
#            change recompiles OUR crates and nothing else.
#
# There are deliberately NO `RUN --mount=type=cache` mounts left in here: a
# cache mount's contents are invisible to the layer cache, so every layer
# after one would re-run on every cold runner and the exported cache would
# carry nothing. Layer cache + chef is the whole trick.

# ── planner ──────────────────────────────────────────────────────────────────
# Same 1.97.1 the devboxes and CI pin via api/rust-toolchain.toml, and the
# alpine variant so the default target is musl — the artifact is static-native
# and drops into the app's alpine runtime with no interpreter, no extra
# packages.
FROM docker.io/library/rust:1.97.1-alpine3.21 AS planner
RUN cargo install cargo-chef --locked
WORKDIR /repo
COPY api ./api
# include_str! in talaria-hermes-skills walks to repo-root scripts/. Flattening
# api/ onto /repo made that path /scripts/... and the package build 404'd.
COPY scripts/hermes-skill-authority.json ./scripts/hermes-skill-authority.json
WORKDIR /repo/api
RUN cargo chef prepare --recipe-path recipe.json

# ── deps ─────────────────────────────────────────────────────────────────────
# build-base + cmake: aws-lc-sys — the TLS stack's C half, the same reason
# the devbox image carries build-essential — compiles C here, in CI, once.
FROM docker.io/library/rust:1.97.1-alpine3.21 AS deps
RUN apk add --no-cache build-base cmake
RUN cargo install cargo-chef --locked
WORKDIR /repo/api
COPY --from=planner /repo/api/recipe.json .
RUN cargo chef cook --release --locked --recipe-path recipe.json

# ── build ────────────────────────────────────────────────────────────────────
FROM deps AS build
COPY api ./api
COPY scripts/hermes-skill-authority.json /repo/scripts/hermes-skill-authority.json
# The binary is copied OUT to a stable path before the layer closes.
RUN cargo build --release --locked \
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
