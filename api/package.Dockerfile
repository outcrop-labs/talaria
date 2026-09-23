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
# Local cargo is capped at 2 jobs (api/.cargo/config.toml) so a shared machine
# stays usable. This stage cooks from recipe.json alone and does not have that
# file, so the cap has to be an env var here — otherwise a laptop `docker build`
# of the package takes every core. A dedicated runner passes CARGO_BUILD_JOBS
# (api-package.yml passes nproc); the default keeps 2. The build stage inherits
# this ENV, which also wins over the config file copied in with the source.
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}
RUN apk add --no-cache build-base cmake
RUN cargo install cargo-chef --locked
WORKDIR /repo/api
COPY --from=planner /repo/api/recipe.json .
# WHICH PROFILE THIS PACKAGE IS BUILT WITH — handed in by the workflow, never
# guessed here: release.yml's resolve passes `dev` for the two pre-stable
# channels (`nightly` from testing, `rc`) and leaves `release` standing
# everywhere else (main's feed, every stable tag). `dev` is the workspace's OWN
# dev profile (api/Cargo.toml: our crates -O1, dependencies -O3, debug =
# line-tables-only), which exists for exactly this trade — the ~200-crate
# compile of OUR crates drops from ~25 minutes to minutes, and the unoptimised
# runtime lands on a channel whose entire job is to be smoke-tested, never on
# an instance rolled from main. Incremental stays off (ci.yml's argument,
# verbatim): a cold container gets no reuse out of it, only the churn.
#
# The COOK must use the same profile as the build below, or cargo rebuilds the
# whole dependency tree at build time and the caching trick inverts itself.
ARG PROFILE=release
RUN set -eu; \
    case "$PROFILE" in \
      release) profile_flag=--release ;; \
      dev) export CARGO_INCREMENTAL=0; profile_flag= ;; \
      *) echo "PROFILE must be release or dev (got '$PROFILE')" >&2; exit 1 ;; \
    esac; \
    cargo chef cook --locked --recipe-path recipe.json $profile_flag

# ── build ────────────────────────────────────────────────────────────────────
FROM deps AS build
# ABSOLUTE destinations, and the absolute part is load-bearing: WORKDIR is
# inherited, and `deps` left it at /repo/api — so a relative `COPY api ./api`
# lands at /repo/api/api and cargo-chef's SKELETON (`fn main() {}`) stays the
# only source at /repo/api. Cargo then relinks the skeleton and the package
# ships a 544 KB binary that exits 0 and prints nothing (2026-09-19 → 09-21:
# every app image built on it died at boot with "RUST API EXITED (code 0)").
COPY api /repo/api
COPY scripts/hermes-skill-authority.json /repo/scripts/hermes-skill-authority.json
# The binary is copied OUT to a stable path before the layer closes, and then
# PROVEN to be the api rather than a placeholder — the stub gate. With no
# database and no redis the real server still binds its port and answers
# /api/healthz (503 from an unreachable dependency, 200 from a healthy one);
# the skeleton answers nothing at all. Any HTTP status line is the assertion —
# which status is the environment's to decide, not this gate's.
#
# `set -eu` is what turns a typo'd PROFILE and a failed cargo into the build
# failure they should be — and it is also why the probe below writes `|| true`
# on the grep and `if` instead of `[ -n "$status" ] && break`: under `set -e`,
# an empty FIRST probe (a slow binary, a port that opens a second late) aborts
# the subshell with exit 1 and no output at all, which is indistinguishable
# from the stub this gate exists to catch. Learned when the dev profile — the
# first build whose binary did not answer on the first probe — failed the
# release-only-tested gate.
#
# `dir` matters as much as `flag`: the same ARG that picks the profile picks
# the artifact directory it leaves the binary in (see the deps stage for which
# channel gets which, and why).
ARG PROFILE=release
RUN set -eu; \
    case "$PROFILE" in \
      release) dir=release; flag=--release ;; \
      dev) dir=debug; flag=''; export CARGO_INCREMENTAL=0 ;; \
      *) echo "PROFILE must be release or dev (got '$PROFILE')" >&2; exit 1 ;; \
    esac; \
    cargo build --locked $flag \
 && cp "target/$dir/talaria-api" /talaria-api \
 && (DATABASE_URL=postgres://stub-gate@127.0.0.1:1/stub \
      REDIS_URL=redis://127.0.0.1:1 \
      TALARIA_SECRET_KEY=0000000000000000000000000000000000000000000000000000000000000000 \
      /talaria-api >/tmp/stub-gate.log 2>&1 & \
     pid=$!; \
     status=""; \
     i=0; \
     while [ "$i" -lt 30 ]; do \
       status=$(wget -S -q -O /dev/null http://127.0.0.1:5274/api/healthz 2>&1 \
                | grep -m1 -o 'HTTP/1[.][0-9] [0-9][0-9][0-9]' || true); \
       if [ -n "$status" ]; then break; fi; \
       i=$((i + 1)); \
       sleep 1; \
     done; \
     kill "$pid" 2>/dev/null || true; \
     sleep 1; \
     kill -9 "$pid" 2>/dev/null || true; \
     if [ -z "$status" ]; then \
       cat /tmp/stub-gate.log; \
       echo "stub gate: the built binary never answered on :5274 — this is not the api"; \
       exit 1; \
     fi; \
     echo "stub gate: the built binary answers /api/healthz ($status)")

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
