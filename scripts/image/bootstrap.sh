#!/usr/bin/env bash
# User-level install of Talaria from a checkout — the golden-image counterpart
# of the manual runbook (docs/SELF-HOSTING.md). Runs as the app user via
# runuser from /etc/talaria/firstboot.sh, from the repo root; the root half
# (installing the systemd unit) stays in firstboot.sh.
#
# Everything here is idempotent AND re-entrant: firstboot retries this on any
# failure, so a half-completed attempt must not poison the retry. That is
# stronger than setup.sh's own idempotency, which skips a step when its OUTPUT
# DIRECTORY exists — see the node_modules check below for why that's not
# enough, and don't add a new skip-if-exists step without the same treatment.
set -euo pipefail
cd "$(dirname "$0")/../.."

say()  { printf '\033[1;36m▸ %s\033[0m\n' "$*"; }
warn() { printf '\033[33m! %s\033[0m\n' "$*"; }

command -v bun >/dev/null || { echo "✗ bun not on PATH — firstboot.sh must set it" >&2; exit 1; }

say "setup.sh"
./scripts/setup.sh

# ── The retry trap ───────────────────────────────────────────────────────────
# setup.sh skips `bun install` when node_modules merely EXISTS. An install
# killed mid-download leaves a half-populated tree that every retry skips and
# every later build chokes on — and the failure surfaces as a vite/tsc error
# far from the retry, looking like a code bug. Verify the trees are actually
# usable; rebuild here when they are not.
if [ ! -x ui/node_modules/.bin/vite ]; then
  say "ui/node_modules incomplete (interrupted install?) — rebuilding"
  rm -rf ui/node_modules && (cd ui && bun install)
fi
if [ -d mcp/node_modules ] && [ ! -x mcp/node_modules/.bin/tsc ]; then
  say "mcp/node_modules incomplete (interrupted install?) — rebuilding"
  rm -rf mcp/node_modules && (cd mcp && bun install)
fi

# ── Infra containers ─────────────────────────────────────────────────────────
# Built-in storage creds: compose interpolates from the ENVIRONMENT, not
# ui/.env (scripts/dev.sh does this same lift). Without it minio and the app
# disagree on the secret, and the built-in bucket fails auth quietly, later.
for var in TALARIA_S3_ACCESS_KEY TALARIA_S3_SECRET_KEY; do
  val=$(grep "^${var}=" ui/.env | head -1 | cut -d= -f2- || true)
  [ -n "$val" ] && export "$var"="$val"
done

# Core first, fatally — the app cannot boot without these.
say "infra (postgres redis qdrant minio)"
docker compose -f docker/dev-compose.yml up -d postgres redis qdrant minio

# Search and embeddings start separately and non-fatally, for the same reason
# dev.sh starts them that way: one unpullable image must not abort an install
# that is otherwise fine. The app runs degraded-but-healthy without either —
# web_search reports itself unavailable; retrieval loses precision.
say "web search (SearXNG)"
bash scripts/render-searxng.sh
docker compose -f docker/dev-compose.yml up -d searxng || {
  warn "search service failed to start — web_search will report itself unavailable"
}
say "embeddings (TEI)"
docker compose -f docker/dev-compose.yml up -d embeddings || {
  warn "embeddings failed to start — retrieval will be degraded"
}

say "waiting for postgres…"
for i in $(seq 1 40); do
  docker exec "${TALARIA_PG_CONTAINER:-talaria-postgres-dev}" pg_isready -U talaria -d talaria >/dev/null 2>&1 && break
  sleep 1
  [ "$i" = 40 ] && { echo "postgres never became ready" >&2; exit 1; }
done

# ── The build ────────────────────────────────────────────────────────────────
say "building (bun run build)"
bun run build

say "bootstrap complete"
