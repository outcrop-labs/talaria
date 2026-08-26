#!/usr/bin/env bash
# Seed a devbox with real starter data from the primary dev environment.
# Invoked by scripts/devbox (new/seed); runnable directly:
#
#   ./scripts/devbox-seed.sh <name> [--force] [--qdrant]
#
# Scope, and why:
#   Postgres   REQUIRED — a point-in-time dump of the primary dev DB (the
#              worktree.sh shape). Everything the app shows comes from here.
#   MinIO      REQUIRED — DB rows reference s3:// blobs; without the mirror
#              the seeded UI shows broken attachments. Box minio runs the
#              same creds as primary, so the mirror is pure bytes.
#   chassis    fleet config — the template with the network repointed at this
#   + fleet/.env  box's private fleet network, and the LLM endpoint copied
#              from the primary fleet/.env (agents need values present; the
#              renderer rewrites them through the box's own gateway anyway).
#   Qdrant     OPTIONAL (--qdrant) — a DERIVED index (vectors of DB-stored
#              docs). Default off: re-run the KB backfill in the box's app
#              instead (the embeddings service is shared, so dimensions
#              match). The flag round-trips an HTTP snapshot.
#   Redis      NEVER — sessions/queues are transient by design.
#
# SNAPSHOT semantics: a seed is a copy, not a link. Later primary changes do
# not flow; re-run with --force to take a fresh copy. Idempotent without it:
# a box that already has tables keeps its data (and your edits to chassis.yml
# / fleet/.env survive — only --force overwrites those two).
set -euo pipefail
cd "$(dirname "$0")/.."
# -P: physical root — see scripts/devbox (symlinked invocation paths must not
# leak into the paths the box resolves).
ROOT="$(pwd -P)"

NAME="${1:-}"
FORCE=0; QDRANT=0
for a in "${@:2}"; do
  case "$a" in
    --force) FORCE=1 ;;
    --qdrant) QDRANT=1 ;;
    *) echo "unknown flag: $a" >&2; exit 1 ;;
  esac
done
[[ "$NAME" =~ ^[a-z0-9][a-z0-9-]*$ ]] || { echo "usage: ./scripts/devbox-seed.sh <name> [--force] [--qdrant]" >&2; exit 1; }

say(){ printf '\033[1;36m▸ %s\033[0m\n' "$*"; }
ok(){ printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn(){ printf '  \033[33m⚠ %s\033[0m\n' "$*"; }
die(){ printf '\033[31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

# Canonical DEVBOXES — see scripts/devbox (paths must be symlink- and
# `..`-free; the box resolves them too).
DEVBOXES="$(cd "${TALARIA_DEVBOX_HOME:-$ROOT/../devboxes}" && pwd -P)"
BOX="$DEVBOXES/$NAME"
STATE="$BOX/state"
MAIN_PGC="${TALARIA_PG_CONTAINER:-talaria-postgres-dev}"
MAIN_MINIOC="${TALARIA_MINIO_CONTAINER:-talaria-minio-dev}"
MAIN_QDRANT="${TALARIA_QDRANT_CONTAINER:-talaria-qdrant-dev}"
PGC="devbox-$NAME-postgres"

docker inspect "$PGC" >/dev/null 2>&1 || die "box postgres ($PGC) isn't running"
docker inspect "$MAIN_PGC" >/dev/null 2>&1 || die "primary postgres ($MAIN_PGC) isn't running — start the main stack first"

# ── Postgres ─────────────────────────────────────────────────────────────────
say "Postgres — point-in-time copy of the primary dev DB"
if [ "$FORCE" -eq 0 ] && [ -n "$(docker exec "$PGC" psql -U talaria -d talaria -tAc "select 1 from information_schema.tables where table_schema='public' limit 1" 2>/dev/null)" ]; then
  ok "already seeded (box DB has tables) — --force to re-copy"
else
  docker exec "$MAIN_PGC" pg_dump -U talaria -d talaria --clean --if-exists 2>/dev/null \
    | docker exec -i "$PGC" psql -U talaria -d talaria -q >/dev/null
  ok "seeded"
fi

# ── MinIO ────────────────────────────────────────────────────────────────────
say "MinIO — mirroring the primary dev bucket (DB rows reference these blobs)"
S3_KEY=$(grep -E '^TALARIA_S3_ACCESS_KEY=' ui/.env | head -1 | cut -d= -f2- || true); S3_KEY="${S3_KEY:-talaria}"
S3_SECRET=$(grep -E '^TALARIA_S3_SECRET_KEY=' ui/.env | head -1 | cut -d= -f2- || true); S3_SECRET="${S3_SECRET:-talaria-dev-secret}"
S3_BUCKET=$(grep -E '^TALARIA_S3_BUCKET=' ui/.env | head -1 | cut -d= -f2- || true); S3_BUCKET="${S3_BUCKET:-talaria}"
DSTC="devbox-$NAME-minio"
# The two minios live on different networks (primary dev vs this box's own),
# so the mirror runs through a throwaway mc container joined to both. Created
# stopped (networks attach to stopped containers), then started with a sleep
# PID — the image's `mc` entrypoint with no arguments exits instantly, and
# exec needs a live container.
MCT="devbox-$NAME-seed-mc"
docker rm -f "$MCT" >/dev/null 2>&1 || true
docker create --name "$MCT" --entrypoint sh docker.io/minio/mc:latest -c 'sleep infinity' >/dev/null
docker network connect "devbox-${NAME}_default" "$MCT"
docker network connect talaria-dev_default "$MCT"
docker start "$MCT" >/dev/null
if ! docker exec "$MCT" sh -c "
  mc alias set src http://$MAIN_MINIOC:9000 '$S3_KEY' '$S3_SECRET' >/dev/null &&
  mc alias set dst http://$DSTC:9000 '$S3_KEY' '$S3_SECRET' >/dev/null"; then
  warn "couldn't reach one of the minios — attachments in the seeded UI will be broken until re-uploaded"
elif ! docker exec "$MCT" sh -c "mc stat src/$S3_BUCKET >/dev/null 2>&1"; then
  # The app creates the bucket lazily; a primary that never uploaded anything
  # has none. Nothing references a blob, so there is nothing to mirror.
  ok "primary has no '$S3_BUCKET' bucket yet (no uploads ever) — nothing to mirror"
elif docker exec "$MCT" sh -c "
  mc mb --ignore-existing dst/$S3_BUCKET >/dev/null 2>&1;
  mc mirror --overwrite src/$S3_BUCKET dst/$S3_BUCKET >/dev/null"; then
  ok "mirrored"
else
  warn "mirror failed — attachments in the seeded UI will be broken until re-uploaded"
fi
docker rm -f "$MCT" >/dev/null 2>&1 || true

# ── Fleet config plane ──────────────────────────────────────────────────────
say "Fleet config — chassis + LLM endpoint (this box's private fleet network)"
mkdir -p "$STATE/fleet"
if [ "$FORCE" -eq 1 ] || [ ! -f "$STATE/fleet/chassis.yml" ]; then
  cp scripts/chassis.template.yml "$STATE/fleet/chassis.yml"
  # Repoint the fleet network at THIS box's own — the template ships the
  # primary install's default name. Range-scoped to the network: block.
  sed -i "/^network:/,/^[^ ]/ s/^  name: .*/  name: devbox-$NAME-fleet/" "$STATE/fleet/chassis.yml"
  ok "chassis.yml seeded (network: devbox-$NAME-fleet)"
else
  ok "chassis.yml exists — kept (your edits survive; --force overwrites)"
fi
if [ "$FORCE" -eq 1 ] || [ ! -f "$STATE/fleet/.env" ]; then
  {
    echo "# Fleet env for devbox '$NAME' — seeded from the primary fleet/.env."
    echo "# The renderer rewrites the endpoint through this box's own gateway;"
    echo "# agents still need the values present to interpolate."
    grep -E '^(LLM_BASE_URL|LLM_API_KEY|LLM_MODEL|HERMES_IMAGE)=' fleet/.env 2>/dev/null || true
  } > "$STATE/fleet/.env"
  chmod 600 "$STATE/fleet/.env"
  ok "fleet/.env seeded"
else
  ok "fleet/.env exists — kept (your edits survive; --force overwrites)"
fi

# NOTE for the future: gateway ports seeded from the same primary dump are
# IDENTICAL across boxes by construction. That is fine — container-dial mode
# (TALARIA_AGENT_DIAL=container, which every box sets) does not publish them.

# ── Qdrant (optional) ────────────────────────────────────────────────────────
if [ "$QDRANT" -eq 1 ]; then
  say "Qdrant — snapshot round-trip (optional; the index is derived data)"
  # The devbox container is dual-homed (box network + primary dev network) and
  # carries curl: run the whole round-trip through it. Host-side $NAME /
  # $MAIN_QDRANT expand here; \$c / \$snap stay for the inner sh.
  docker exec "devbox-$NAME" sh -c '
    set -e
    for c in $(curl -sf http://'"$MAIN_QDRANT"':6333/collections | grep -o "\"name\":\"[^\"]*\"" | cut -d"\"" -f4); do
      snap=$(curl -sf -X POST http://'"$MAIN_QDRANT"':6333/collections/$c/snapshots | grep -o "\"name\":\"[^\"]*\"" | head -1 | cut -d"\"" -f4)
      curl -sf http://'"$MAIN_QDRANT"':6333/collections/$c/snapshots/$snap -o /tmp/$c.snapshot
      curl -sf -X POST -F "snapshot=@/tmp/$c.snapshot" http://devbox-'"$NAME"'-qdrant:6333/collections/$c/snapshots/upload?priority=snapshot >/dev/null
      rm -f /tmp/$c.snapshot
      echo "  ok $c"
    done' \
    && ok "qdrant restored" || warn "qdrant snapshot failed — re-run the KB backfill in the box's app instead"
else
  warn "skipping Qdrant (derived index) — re-run the KB backfill in the box's app, or pass --qdrant"
fi
