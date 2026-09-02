# Talaria — the whole app in one production container.
#
# Build from the REPO ROOT (the context needs ui/, mcp/, apps/, scripts/skills,
# scripts/chassis.template.yml and docker/searxng/):
#
#   docker build -t talaria:latest .
#
# Or let docker/compose.yml build it (the default — see docs/CONTAINER.md).
# The image carries no config: every knob is an environment variable, and the
# entrypoint generates what's missing into the persistent state dir
# (/var/lib/talaria, a host bind mounted at the same path — the fleet renderer
# bakes absolute host paths into agent bind mounts, so state must be host-real).
#
# Three stages: build (toolchain + vite/tsc), prod-deps (pruned
# node_modules — the SSR bundle externalizes node_modules, so they ship too),
# runtime (bun + docker CLI + compose plugin + git + the api binary, nothing
# else). The Rust api never compiles here: it arrives as a pre-built package
# image (the `api` stage below; api/package.Dockerfile is its source of
# truth), because app images build on GitHub runners, on operators'
# machines, and on customer VMs — none of which should own a Rust toolchain.

# The api package that stage consumes. Declared HERE, before the first FROM,
# because FROM-line interpolation only sees args declared at the top level —
# an ARG between stages belongs to no stage and reads as blank (this file's
# one live lesson from building it for real). Used by exactly one FROM, so a
# changed value never invalidates the expensive install/build layers.
ARG TALARIA_API_IMAGE=ghcr.io/outcrop-labs/talaria-api:main

# ── build ────────────────────────────────────────────────────────────────────
# Node alongside bun for the same reason CI carries both (see
# .github/workflows/ci.yml, "WHY BUN *AND* NODE"): the toolchain binaries
# (vite, vitest, tsc) carry `env node` shebangs, and `bun run` executes them
# through Node. Bun itself is pinned by the root package.json packageManager.
FROM docker.io/library/node:22-alpine AS build
COPY --from=oven/bun:1.4.0-alpine /usr/local/bin/bun /usr/local/bin/bun

WORKDIR /repo
# Manifests first so dependency install layer-caches across source changes.
COPY ui/package.json ui/bun.lock ./ui/
COPY mcp/package.json mcp/bun.lock ./mcp/
# mcp's install skips lifecycle scripts: its prepare runs tsc against sources
# that don't exist in this manifests-only layer — the explicit build below
# compiles it instead.
RUN --mount=type=cache,target=/root/.bun/install/cache cd ui && bun install --frozen-lockfile \
 && cd ../mcp && bun install --frozen-lockfile --ignore-scripts

# Everything else. .dockerignore keeps node_modules/dist out of this copy, so
# the installs above survive it.
# apps/ is NOT ignored: apps compile into the bundle at build time
# (import.meta.glob in ui/src/server/apps.ts) — including gitignored local
# apps (e.g. apps/leadworks) that are part of local builds by design.
COPY . .
RUN cd ui && bun run build \
 && cd ../mcp && bun run build

# ── api ──────────────────────────────────────────────────────────────────────
# The pre-built Rust api, consumed as a package image — never compiled in an
# app-image build (the stage's whole reason to exist; see api/package.Dockerfile).
# The default (TALARIA_API_IMAGE, declared at the top) tracks main's package,
# which api-package.yml publishes on every api-touching push to main — the
# same ref Dokploy builds, so a checkout build on a deploy box pulls instead
# of compiling. release.yml pins the exact sha-tagged package it just built,
# so a released image names the api bits it carries. A local api change
# overrides the source (docs/CONTAINER.md):
#
#   docker build -f api/package.Dockerfile -t talaria-api:local .
#   docker build --build-arg TALARIA_API_IMAGE=talaria-api:local .
FROM ${TALARIA_API_IMAGE} AS api

# ── prod-deps ────────────────────────────────────────────────────────────────
# Runtime node_modules, pruned to production deps. Kept as a separate stage so
# the runtime never carries devDependencies (vite, svelte-check, tailwind…).
FROM docker.io/library/node:22-alpine AS prod-deps
COPY --from=oven/bun:1.4.0-alpine /usr/local/bin/bun /usr/local/bin/bun

WORKDIR /repo
COPY ui/package.json ui/bun.lock ./ui/
COPY mcp/package.json mcp/bun.lock ./mcp/
RUN --mount=type=cache,target=/root/.bun/install/cache cd ui && bun install --production --frozen-lockfile \
 # mcp's prepare script runs tsc — a devDependency — so a production install
 # must skip lifecycle scripts; the built dist/ comes from the build stage.
 && cd ../mcp && bun install --production --frozen-lockfile --ignore-scripts

# ── runtime ──────────────────────────────────────────────────────────────────
FROM docker.io/library/alpine:3.21

# Release identity, injected by the build (.github/workflows/release.yml).
# Declared in THIS stage only: an ARG invalidates the build cache of every
# layer after it, so keeping the identity at the tail preserves the expensive
# install/build layers across version changes. The defaults matter as much
# as the values: a plain `docker build .` — an operator's local build,
# compose's deploy-time build, CI's chassis smoke — passes no args and must
# keep working, and then the labels read "unknown", which is simply true of
# a local build.
ARG VERSION=unknown
ARG REVISION=unknown
ARG CREATED=unknown

# OCI annotations: `docker inspect ghcr.io/outcrop-labs/talaria:nightly` is
# how an operator answers "what am I actually running" without a git checkout.
LABEL org.opencontainers.image.title="Talaria" \
      org.opencontainers.image.description="The operations platform for companies that run on people and AI agents" \
      org.opencontainers.image.source="https://github.com/outcrop-labs/talaria" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.created="${CREATED}"

# docker CLI + compose v2: the app drives the host daemon through the mounted
# socket (fleet render/up, docker exec for agent memory/media/crons).
# git: app installs (git clone from the marketplace). tzdata: the app renders
# local times. libgcc + libstdc++: the bun binary links them (they come free
# with the node-based build stages but not with bare alpine — same set the
# oven/bun alpine image itself carries). Nothing else — every runtime
# dependency is pure JS.
RUN apk add --no-cache docker-cli docker-cli-compose git ca-certificates tzdata libgcc libstdc++

COPY --from=oven/bun:1.4.0-alpine /usr/local/bin/bun /usr/local/bin/bun
# The api binary: server-entry.js spawns it at boot (adopting any instance
# already on the port), so the SPA process and the api share one supervisor,
# one log, and one SIGTERM. musl-static out of the package stage — no runtime
# packages follow it (the package carries its own ca-certificates story; this
# runtime's apk line already installs them too).
COPY --from=api /usr/local/bin/talaria-api /usr/local/bin/talaria-api
COPY docker/entrypoint.sh /usr/local/bin/talaria-entrypoint
COPY docker/await-deps.mjs /app/docker/await-deps.mjs
RUN chmod 755 /usr/local/bin/talaria-entrypoint

WORKDIR /app
# ui/: the built app. src/server/env.ts ships as SOURCE on purpose —
# server-entry.js imports it by name at boot (Node >= 22.18 strips types; bun
# executes TS natively), so it must exist next to the bundle.
COPY --from=build /repo/ui/server-entry.js ./ui/
COPY --from=build /repo/ui/dist ./ui/dist
COPY --from=build /repo/ui/src/server/env.ts ./ui/src/server/env.ts
COPY --from=build /repo/ui/package.json ./ui/
COPY --from=prod-deps /repo/ui/node_modules ./ui/node_modules
# mcp/: the fleet's toolkit service, spawned as a child process
# (ui/src/server/mcp-service.ts execs ../mcp/dist/index.js).
COPY --from=build /repo/mcp/dist ./mcp/dist
COPY --from=build /repo/mcp/package.json ./mcp/
COPY --from=prod-deps /repo/mcp/node_modules ./mcp/node_modules
# scripts/: seeded fleet skills + the chassis template the entrypoint copies
# into the state dir on first boot.
COPY scripts/skills ./scripts/skills
COPY scripts/chassis.template.yml ./scripts/
COPY docker/searxng/settings.template.yml ./docker/searxng/

# Fixed uid so host-side ownership is documentable (state bind, docker group).
RUN adduser -D -u 10001 talaria \
 && mkdir -p /var/lib/talaria && chown talaria:talaria /var/lib/talaria

# The production posture as env defaults the operator can still override:
# PORT=5273 is the fleet contract (agents dial
# host.docker.internal:5273 / the app's network alias), COOKIE_SECURE=0 because
# the default posture is plain http (browsers drop Secure cookies over http —
# flip it behind TLS, see docs/CONTAINER.md), TALARIA_UPDATER=off because the
# orchestrator (compose/Dokploy) owns deploys and the image has no git checkout.
# TALARIA_API_BIN tells server-entry.js where the api binary it spawns lives —
# the path is fixed by the COPY above; the env keeps the entry from guessing a
# repo-layout path that only exists on dev boxes.
# TALARIA_JS_RUNTIME=bun because this image ships bun and NO node: the api's
# mcp supervisor (api/src/mcp/service.rs) spawns mcp/dist/index.js under this
# runtime, and its node default dies with ENOENT in a container that has no
# node to find.
# State lives under /var/lib/talaria via the existing path overrides, never in
# default paths inside /app (those are image-owned).
ENV PORT=5273 \
    HOST=0.0.0.0 \
    NODE_ENV=production \
    COOKIE_SECURE=0 \
    TALARIA_UPDATER=off \
    TALARIA_API_BIN=/usr/local/bin/talaria-api \
    TALARIA_JS_RUNTIME=bun \
    TALARIA_UPLOADS_DIR=/var/lib/talaria/uploads \
    TALARIA_FLEET_DIR=/var/lib/talaria/fleet \
    TALARIA_APPS_DIR=/var/lib/talaria/apps \
    # From ARG VERSION above: the LABEL carries it for `docker inspect`, this
    # carries it for the process and `docker exec`.
    TALARIA_VERSION=${VERSION}

USER talaria
WORKDIR /app/ui
# Exec form: bun becomes PID 1 and receives SIGTERM (server-entry.js shuts down
# gracefully — in-flight requests drain, agents get a clean socket close).
ENTRYPOINT ["/usr/local/bin/talaria-entrypoint"]
