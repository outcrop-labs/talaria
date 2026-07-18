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

echo "▸ app → http://localhost:5273"
cd ui && exec npm run dev
