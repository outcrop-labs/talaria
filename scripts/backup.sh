#!/usr/bin/env bash
# Talaria backup — one snapshot of the two things that cannot be rebuilt: the
# Postgres database and the upload blobs (local disk, the bundled MinIO bucket,
# or an external S3 bucket — whichever Admin → Storage is using).
#
#   ./scripts/backup.sh [dest-dir]
#
# Redis is deliberately NOT backed up: it holds sessions and ephemeral state, so
# losing it signs everyone out and nothing more. Qdrant isn't either — every
# vector is re-derivable from Postgres by reindexing.
#
# Nothing here schedules itself; point cron/systemd at it (the in-app scheduler
# is a later milestone). Retention + the RESTORE procedure: docs/BACKUPS.md.
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/backup-lib.sh

DEST="${1:-${TALARIA_BACKUP_DIR:-backups}}"
KEEP="${TALARIA_BACKUP_KEEP:-7}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
SNAP="$DEST/$STAMP"
STAGE="$SNAP.partial"

command -v docker >/dev/null || command -v pg_dump >/dev/null || die "need either pg_dump on PATH or docker"
load_app_env
[ -n "${DATABASE_URL:-}" ] || die "DATABASE_URL is not set (looked in the environment and ${TALARIA_ENV_FILE:-ui/.env})"

# Everything is written to <stamp>.partial and renamed at the very end, so a
# half-written snapshot never looks complete and retention can never delete a
# good one in favour of a broken one.
[ -e "$SNAP" ] && die "$SNAP already exists"
mkdir -p "$DEST"
# A snapshot is a full database dump; the default destination sits inside the
# checkout. Make the directory ignore itself rather than trusting that whichever
# repo it lands in has a rule for it.
[ -e "$DEST/.gitignore" ] || printf '*\n' >"$DEST/.gitignore"
mkdir -p "$STAGE"
STAGE_ABS="$(cd "$STAGE" && pwd)" # mc runs in a container: it needs real paths
trap 'rm -rf "$STAGE"' EXIT

say "Postgres → $STAGE/db.sql.gz  ($(db_label))"
# --no-owner/--no-privileges so the dump restores under whatever role does the
# restoring; --clean --if-exists so it lands on a non-empty database too.
pg_client pg_dump --clean --if-exists --no-owner --no-privileges "$DATABASE_URL" | gzip >"$STAGE/db.sql.gz"
# A truncated dump is the classic silent backup failure (a full disk, an
# OOM-killed client, a connection dropped mid-stream). pg_dump writes its
# completion trailer only when it actually finished, so check for that as well
# as for a well-formed archive.
gzip -t "$STAGE/db.sql.gz" || die "the dump is not a valid gzip archive"
gunzip -c "$STAGE/db.sql.gz" | tail -5 | grep -q 'PostgreSQL database dump complete' ||
  die "the dump has no completion marker — it was truncated, refusing to keep it"
ok "$(du -h "$STAGE/db.sql.gz" | cut -f1) compressed"

say "Upload blobs"
storage_from_db
# Both storage shapes produce the same artifact: a flat tar of blob files named
# <upload-id><ext>, which is exactly what uploads.ts writes on disk AND the tail
# of every bucket key. So a snapshot taken in one mode restores into the other.
case "$STORAGE_MODE" in
  local)
    UPLOADS_DIR="${TALARIA_UPLOADS_DIR:-$PWD/ui/.uploads}"
    if [ -d "$UPLOADS_DIR" ]; then
      tar -czf "$STAGE/uploads.tar.gz" -C "$UPLOADS_DIR" .
      ok "local disk — $UPLOADS_DIR"
    else
      tar -czf "$STAGE/uploads.tar.gz" -T /dev/null
      warn "local disk — $UPLOADS_DIR does not exist yet (empty archive written)"
    fi
    ;;
  internal | s3)
    mkdir -p "$STAGE_ABS/blobs"
    if ! mc_run "$STAGE_ABS" "mc --config-dir /tmp/mc-talaria mirror --quiet --overwrite $(bucket_uploads_path) '$STAGE_ABS/blobs' >/dev/null"; then
      # An empty prefix is not an error — but an unreachable bucket is, and mc
      # reports both by exiting non-zero. Tell them apart before continuing.
      mc_run "$STAGE_ABS" "mc --config-dir /tmp/mc-talaria ls t/$S3_BUCKET >/dev/null" ||
        die "bucket $S3_BUCKET at $S3_ENDPOINT is unreachable — check the endpoint and credentials"
      warn "no objects under $(bucket_uploads_path) yet"
    fi
    tar -czf "$STAGE/uploads.tar.gz" -C "$STAGE_ABS/blobs" .
    rm -rf "$STAGE_ABS/blobs" # the tar is the artifact; the mirror was scratch
    ok "bucket $S3_BUCKET at $S3_ENDPOINT (${STORAGE_MODE})"
    ;;
esac
BLOBS=$(tar -tzf "$STAGE/uploads.tar.gz" | grep -vc '/$' || true)
ok "$BLOBS blob(s), $(du -h "$STAGE/uploads.tar.gz" | cut -f1) compressed"

# Identifiers only — never credentials. restore.sh reads the storage fields back
# out of here, because at restore time the database isn't there to ask.
cat >"$STAGE/manifest.txt" <<EOF
talaria_backup=1
created_at=$(date -u +%FT%TZ)
created_on=$(hostname)
database=$(db_label)
storage_mode=$STORAGE_MODE
storage_endpoint=$S3_ENDPOINT
storage_bucket=$S3_BUCKET
storage_prefix=$S3_PREFIX
uploads_dir=${UPLOADS_DIR:-}
blob_count=$BLOBS
EOF
(cd "$STAGE" && sha256sum db.sql.gz uploads.tar.gz manifest.txt >SHA256SUMS)

mv "$STAGE" "$SNAP"
trap - EXIT
ok "snapshot complete — $SNAP ($(du -sh "$SNAP" | cut -f1))"

# Retention. Snapshot directory names sort chronologically, so newest-first is a
# reverse sort. Only ever touch directories that carry a manifest — a stray
# directory in the backup folder is not ours to delete.
if [ "$KEEP" -gt 0 ]; then
  say "Retention (keeping the newest $KEEP)"
  PRUNED=0
  while IFS= read -r old; do
    [ -f "$old/manifest.txt" ] || continue
    rm -rf "$old"
    PRUNED=$((PRUNED + 1))
  done < <(find "$DEST" -mindepth 1 -maxdepth 1 -type d -name '*Z' | sort -r | tail -n "+$((KEEP + 1))")
  ok "$PRUNED older snapshot(s) removed"
fi

echo
printf '\033[1;32mBacked up.\033[0m\n\n'
echo "  Restore it:   ./scripts/restore.sh $SNAP --target <postgres-url>"
echo "  Procedure:    docs/BACKUPS.md   (test a restore before you need one)"
