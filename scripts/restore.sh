#!/usr/bin/env bash
# Restore a snapshot taken by scripts/backup.sh. DESTRUCTIVE: the dump drops and
# recreates every object it owns in the target database.
#
#   ./scripts/restore.sh <snapshot-dir> [--target <postgres-url>] [--db-only|--uploads-only] [--yes]
#
# Stop the app first — it runs migrations on its first query and holds a pool
# open, and neither survives the schema being swapped underneath it.
#
# The target defaults to DATABASE_URL (environment, then ui/.env). Always name
# --target explicitly when restoring somewhere other than this checkout's app.
# Full procedure, including the drill you should actually run: docs/BACKUPS.md.
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/backup-lib.sh

SNAP="" TARGET="" WHAT=all ASSUME_YES=0
while [ $# -gt 0 ]; do
  case "$1" in
    --target) TARGET="${2:-}"; shift 2 ;;
    --db-only) WHAT=db; shift ;;
    --uploads-only) WHAT=uploads; shift ;;
    -y | --yes) ASSUME_YES=1; shift ;;
    -h | --help) sed -n '2,12{s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
    -*) die "unknown flag $1" ;;
    *) SNAP="${1%/}"; shift ;;
  esac
done
[ -n "$SNAP" ] || die "usage: ./scripts/restore.sh <snapshot-dir> [--target <postgres-url>] [--db-only|--uploads-only] [--yes]"
[ -f "$SNAP/manifest.txt" ] || die "$SNAP is not a Talaria snapshot (no manifest.txt)"

load_app_env
[ -n "$TARGET" ] || TARGET="${DATABASE_URL:-}"
[ -n "$TARGET" ] || die "no target database — pass --target, or set DATABASE_URL"
SNAP_ABS="$(cd "$SNAP" && pwd)"

say "Verifying $SNAP"
(cd "$SNAP" && sha256sum -c SHA256SUMS >/dev/null) || die "checksum mismatch — this snapshot is corrupt, use an older one"
ok "checksums match (taken $(manifest_get "$SNAP/manifest.txt" created_at) from $(manifest_get "$SNAP/manifest.txt" database))"

# Confirmation is not optional theatre here: the usual way to lose data is to
# restore a good snapshot over the wrong database.
printf '\n  This will REPLACE %s in \033[1m%s\033[0m\n\n' \
  "$([ "$WHAT" = all ] && echo 'the database and the upload blobs' || echo "the $WHAT")" "${TARGET##*@}"
if [ "$ASSUME_YES" != 1 ]; then
  [ -t 0 ] || die "refusing to restore non-interactively without --yes"
  read -r -p "  Type 'restore' to continue: " reply
  [ "$reply" = restore ] || die "aborted"
fi

if [ "$WHAT" != uploads ]; then
  say "Postgres ← $SNAP/db.sql.gz"
  # ON_ERROR_STOP so a partial restore is a failure, not a warning scrolling by.
  gunzip -c "$SNAP/db.sql.gz" | pg_client psql "$TARGET" -v ON_ERROR_STOP=1 -q -o /dev/null ||
    die "restore failed — the target database is now in an incomplete state, fix the cause and re-run"
  ok "$(pg_query "$TARGET" "select count(*) from information_schema.tables where table_schema='public'") tables restored"
fi

if [ "$WHAT" != db ]; then
  say "Upload blobs ← $SNAP/uploads.tar.gz"
  storage_from_manifest "$SNAP/manifest.txt"
  case "$STORAGE_MODE" in
    local)
      # The snapshot's uploads_dir is where they lived on the SOURCE host; this
      # host's config decides where they land.
      UPLOADS_DIR="${TALARIA_UPLOADS_DIR:-$PWD/ui/.uploads}"
      mkdir -p "$UPLOADS_DIR"
      tar -xzf "$SNAP/uploads.tar.gz" -C "$UPLOADS_DIR"
      ok "extracted to $UPLOADS_DIR"
      ;;
    internal | s3)
      TMP="$(mktemp -d "${TMPDIR:-/tmp}/talaria-restore.XXXXXX")"
      trap 'rm -rf "$TMP"' EXIT
      tar -xzf "$SNAP_ABS/uploads.tar.gz" -C "$TMP"
      mc_run "$TMP" "mc --config-dir /tmp/mc-talaria mirror --quiet --overwrite '$TMP' $(bucket_uploads_path) >/dev/null" ||
        die "could not mirror into $(bucket_uploads_path) — check the endpoint and credentials"
      rm -rf "$TMP"
      trap - EXIT
      ok "uploaded to bucket $S3_BUCKET at $S3_ENDPOINT"
      ;;
  esac
fi

echo
printf '\033[1;32mRestored.\033[0m\n\n'
echo "  Start the app, sign in, and open a message attachment — that exercises"
echo "  both halves (row in Postgres, bytes in storage) in one click."
