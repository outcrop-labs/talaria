#!/usr/bin/env bash
# Bring up the whole Talaria dev stack: infra (postgres + redis), wait until
# they're actually ready (avoids the cached-migration-failure boot gotcha),
# then the app dev server on :5273.
set -euo pipefail
cd "$(dirname "$0")/.."

[ -f ui/.env ] || { echo "ui/.env missing — run ./scripts/setup.sh first" >&2; exit 1; }

# Guard: a LINKED git worktree (its .git is a file, not a dir) must have its own
# isolated stack, or it will run a second app against the MAIN dev DB and can
# corrupt shared state (see docs/WORKTREES.md). scripts/worktree.sh stamps
# TALARIA_WORKTREE; a plain `git worktree add` won't have it.
if [ -f .git ] && ! grep -q '^TALARIA_WORKTREE=' ui/.env; then
  echo "✗ This is a git worktree without an isolated stack." >&2
  echo "  Don't run dev.sh here — it would share the main dev database." >&2
  echo "  Create isolated worktrees with:  ./scripts/worktree.sh <name>   (see docs/WORKTREES.md)" >&2
  exit 1
fi

# Built-in object storage creds: compose must match the app, so lift them out
# of ui/.env for interpolation (both fall back to the same dev defaults).
for var in TALARIA_S3_ACCESS_KEY TALARIA_S3_SECRET_KEY; do
  val=$(grep "^${var}=" ui/.env | head -1 | cut -d= -f2- || true)
  [ -n "$val" ] && export "$var"="$val"
done

echo "▸ infra (postgres + redis + qdrant + minio)"
docker compose -f docker/dev-compose.yml up -d postgres redis qdrant minio

# Search started separately, and non-fatally, for the same reason embeddings is:
# one unpullable image must not abort the services the app cannot boot without.
# Talaria runs fine with search down — `web_search` reports that live search is
# unavailable and models are told to say so rather than answer from memory.
# SearXNG reads its secret from settings.yml, not the environment, and the
# image's own substitution needs a writable file — which would mean the
# container rewriting a tracked file. So the template is rendered here instead,
# with the per-install key from ui/.env, into a gitignored file compose mounts.
SEARXNG_SECRET=$(grep '^SEARXNG_SECRET=' ui/.env 2>/dev/null | head -1 | cut -d= -f2- || true)
[ -n "$SEARXNG_SECRET" ] || SEARXNG_SECRET="talaria-$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')"
if [ ! -f docker/searxng/settings.local.yml ] || [ docker/searxng/settings.template.yml -nt docker/searxng/settings.local.yml ]; then
  sed "s|__SEARXNG_SECRET__|${SEARXNG_SECRET}|" docker/searxng/settings.template.yml > docker/searxng/settings.local.yml
  echo "▸ rendered docker/searxng/settings.local.yml"
fi

echo "▸ web search (SearXNG)"
docker compose -f docker/dev-compose.yml up -d searxng || {
  echo "⚠ search service failed to start — web_search will report itself unavailable." >&2
  echo "  Continuing without it; see docker/searxng/settings.yml." >&2
}

# Embeddings started separately: a single `up` resolves every image before
# creating any container, so one unpullable/broken image (e.g. #151) would
# abort postgres/redis/qdrant too. The app boots without embeddings —
# retrieval just degrades — so failure here warns instead of exiting.
echo "▸ embeddings (TEI)"
docker compose -f docker/dev-compose.yml up -d embeddings || {
  echo "⚠ embeddings service failed to start — retrieval will be degraded." >&2
  echo "  Continuing without it; see docker/dev-compose.yml + issue #151." >&2
}

echo "▸ waiting for postgres…"
for i in $(seq 1 40); do
  docker exec "${TALARIA_PG_CONTAINER:-talaria-postgres-dev}" pg_isready -U talaria -d talaria >/dev/null 2>&1 && break
  sleep 1
  [ "$i" = 40 ] && { echo "postgres never became ready" >&2; exit 1; }
done
echo "▸ waiting for redis…"
for i in $(seq 1 20); do
  docker exec "${TALARIA_REDIS_CONTAINER:-talaria-redis-dev}" redis-cli ping >/dev/null 2>&1 && break
  sleep 1
done

[ -d ui/node_modules ] || (cd ui && npm install --no-fund --no-audit)

# The fleet's Talaria toolkit. mcp/ compiles to mcp/dist/index.js, which the app
# SPAWNS (ui/src/server/mcp-service.ts) — and mcp/dist is gitignored, so a pull,
# a rebase or an edit to mcp/src changes nothing at runtime until it is rebuilt.
# Nothing did that before this step, so every agent kept talking to whatever dist
# happened to be on disk: stale tool descriptions and, worse, stale AUTH.
# TALARIA_SKIP_MCP_BUILD=1 to get past a build you are mid-way through fixing.
if [ "${TALARIA_SKIP_MCP_BUILD:-0}" = "1" ]; then
  echo "▸ toolkit MCP — skipped (TALARIA_SKIP_MCP_BUILD=1); mcp/dist may be stale"
else
  [ -d mcp/node_modules ] || (cd mcp && npm install --no-fund --no-audit)
  # Rebuild when dist is missing or ANY source file is newer than it. `find -newer`
  # rather than a timestamp we cache, so a `git checkout` of an older src still
  # triggers (its mtime is the checkout time, i.e. newer).
  if [ ! -f mcp/dist/index.js ] || [ -n "$(find mcp/src -newer mcp/dist/index.js -print -quit)" ]; then
    echo "▸ toolkit MCP → mcp/dist"
    (cd mcp && npm run build) || {
      echo "✗ mcp/ failed to build — mcp/dist is now STALE or missing and the fleet toolkit" >&2
      echo "  will serve the old build (or nothing at all). Fix mcp/src, or re-run with" >&2
      echo "  TALARIA_SKIP_MCP_BUILD=1 if you meant to leave it." >&2
      exit 1
    }
  else
    echo "▸ toolkit MCP — mcp/dist up to date"
  fi
fi

echo "▸ app → http://localhost:5273"
cd ui && exec npm run dev
