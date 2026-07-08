#!/usr/bin/env bash
# Bring up the whole Talaria dev stack: infra (postgres + redis), wait until
# they're actually ready (avoids the cached-migration-failure boot gotcha),
# then the app dev server on :5273.
set -euo pipefail
cd "$(dirname "$0")/.."

[ -f ui/.env ] || { echo "ui/.env missing — run ./scripts/setup.sh first" >&2; exit 1; }

echo "▸ infra (postgres + redis)"
docker compose -f docker/dev-compose.yml up -d

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
