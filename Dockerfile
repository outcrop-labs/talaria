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
# Three stages: build (toolchain + vite/tsc), prod-deps (pruned node_modules —
# the SSR bundle externalizes node_modules, so they ship too), runtime (bun +
# docker CLI + compose plugin + git, nothing else).

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

# docker CLI + compose v2: the app drives the host daemon through the mounted
# socket (fleet render/up, docker exec for agent memory/media/crons).
# git: app installs (git clone from the marketplace). tzdata: the app renders
# local times. libgcc + libstdc++: the bun binary links them (they come free
# with the node-based build stages but not with bare alpine — same set the
# oven/bun alpine image itself carries). Nothing else — every runtime
# dependency is pure JS.
RUN apk add --no-cache docker-cli docker-cli-compose git ca-certificates tzdata libgcc libstdc++

COPY --from=oven/bun:1.4.0-alpine /usr/local/bin/bun /usr/local/bin/bun
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

# The production posture of scripts/image/talaria.service, as env defaults the
# operator can still override: PORT=5273 is the fleet contract (agents dial
# host.docker.internal:5273 / the app's network alias), COOKIE_SECURE=0 because
# the default posture is plain http (browsers drop Secure cookies over http —
# flip it behind TLS, see docs/CONTAINER.md), TALARIA_UPDATER=off because the
# orchestrator (compose/Dokploy) owns deploys and the image has no git checkout.
# State lives under /var/lib/talaria via the existing path overrides, never in
# default paths inside /app (those are image-owned).
ENV PORT=5273 \
    HOST=0.0.0.0 \
    NODE_ENV=production \
    COOKIE_SECURE=0 \
    TALARIA_UPDATER=off \
    TALARIA_UPLOADS_DIR=/var/lib/talaria/uploads \
    TALARIA_FLEET_DIR=/var/lib/talaria/fleet \
    TALARIA_APPS_DIR=/var/lib/talaria/apps

USER talaria
WORKDIR /app/ui
# Exec form: bun becomes PID 1 and receives SIGTERM (server-entry.js shuts down
# gracefully — in-flight requests drain, agents get a clean socket close).
ENTRYPOINT ["/usr/local/bin/talaria-entrypoint"]
