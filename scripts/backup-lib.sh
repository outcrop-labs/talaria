# Shared plumbing for scripts/backup.sh and scripts/restore.sh. Sourced, never run.
#
# Both scripts need the same three things: the app's own config, a Postgres
# client, and the real home of the upload blobs — which is local disk, the
# bundled MinIO container, or an external S3-compatible bucket depending on
# what Admin → Storage is set to (see ui/src/server/storage.ts).

say()  { printf '\033[1;36m▸ %s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m! %s\033[0m\n' "$*"; }
die()  { printf '\033[31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

# Client images, only used when the host has no psql/pg_dump/mc. postgres:16
# matches docker/dev-compose.yml — a dump is refused if the client is older
# than the server, so bump this together with the server image.
PG_IMAGE="${TALARIA_PG_IMAGE:-postgres:16-alpine}"
MC_IMAGE="${TALARIA_MC_IMAGE:-minio/mc:latest}"

# Lift the app's config out of ui/.env (same one-var-at-a-time read as dev.sh).
# A value already exported ALWAYS wins, so an operator can back up a remote
# instance without a checkout of its .env.
load_app_env() {
  local file="${TALARIA_ENV_FILE:-ui/.env}" var val
  for var in DATABASE_URL TALARIA_UPLOADS_DIR TALARIA_S3_URL TALARIA_S3_BUCKET \
             TALARIA_S3_ACCESS_KEY TALARIA_S3_SECRET_KEY TALARIA_MINIO_PORT; do
    [ -n "${!var:-}" ] && continue
    [ -f "$file" ] || continue
    val=$(grep "^${var}=" "$file" | head -1 | cut -d= -f2- || true)
    val=${val%\"}; val=${val#\"} # tolerate hand-quoted values
    [ -n "$val" ] && export "$var"="$val"
  done
  return 0 # a missing last var leaves the loop's test failing — not an error
}

# The connection string minus its credentials — safe to print and to record in
# a manifest that sits next to the dump.
db_label() { printf '%s' "${DATABASE_URL##*@}"; }

# Run a Postgres client binary against a connection string. Host binaries win;
# otherwise borrow them from a throwaway container on the host network, so a
# 127.0.0.1 URL resolves the same either way (Linux; on macOS/Windows install
# the postgres client instead — Docker's host networking isn't the default).
pg_client() {
  local bin="$1"; shift
  if command -v "$bin" >/dev/null 2>&1; then
    "$bin" "$@"
  else
    docker run --rm -i --network host "$PG_IMAGE" "$bin" "$@"
  fi
}

# One-shot query, no headers. The field separator is US (0x1f) rather than a tab
# because `read` collapses runs of whitespace separators — which would silently
# shift every column after an empty one.
PG_FS=$'\x1f'
pg_query() { pg_client psql "$1" -v ON_ERROR_STOP=1 -At -F"$PG_FS" -q -c "$2"; }

# Where the blobs live. Sets STORAGE_MODE + (for bucket modes) S3_ENDPOINT,
# S3_BUCKET, S3_PREFIX, S3_ACCESS_KEY, S3_SECRET_KEY. Two entry points because
# a backup reads the live config out of the database and a restore reads it out
# of the snapshot's manifest (the database may not exist yet).
STORAGE_MODE=local S3_ENDPOINT= S3_BUCKET= S3_PREFIX= S3_ACCESS_KEY= S3_SECRET_KEY=

# The bundled MinIO container, resolved exactly as storage.ts's internalTarget()
# does — same env vars, same defaults, empty prefix.
internal_target() {
  S3_ENDPOINT="${TALARIA_S3_URL:-http://127.0.0.1:${TALARIA_MINIO_PORT:-9010}}"
  S3_BUCKET="${TALARIA_S3_BUCKET:-talaria}"
  S3_ACCESS_KEY="${TALARIA_S3_ACCESS_KEY:-talaria}"
  S3_SECRET_KEY="${TALARIA_S3_SECRET_KEY:-talaria-dev-secret}"
  S3_PREFIX=""
}

# An external bucket's secret key is SEALED in app_settings (secretbox), and a
# backup must not need the app's decryption keys to run. So the operator hands
# it over out of band. The access key id is readable, but on restore the
# database isn't there to read it from — hence both env vars.
external_creds() {
  [ -n "${TALARIA_BACKUP_S3_SECRET_KEY:-}" ] || die \
    "storage mode is \"s3\" — set TALARIA_BACKUP_S3_SECRET_KEY (the bucket secret is sealed at rest; see docs/BACKUPS.md)"
  S3_SECRET_KEY="$TALARIA_BACKUP_S3_SECRET_KEY"
  [ -n "$S3_ACCESS_KEY" ] || S3_ACCESS_KEY="${TALARIA_BACKUP_S3_ACCESS_KEY:-}"
  [ -n "$S3_ACCESS_KEY" ] || die "storage mode is \"s3\" — set TALARIA_BACKUP_S3_ACCESS_KEY"
}

storage_from_db() {
  local row probe
  # Probe the table first, and keep the two failures apart: a missing
  # app_settings means the app never wrote a storage config (local is right), an
  # unreachable database means the whole run is wrong. Collapsing them would
  # silently back up zero blobs and call it a success.
  probe=$(pg_query "$DATABASE_URL" "select to_regclass('public.app_settings') is not null") ||
    die "cannot query $(db_label) — can't tell where the upload blobs live"
  [ "$probe" = t ] || { STORAGE_MODE=local; return; }
  row=$(pg_query "$DATABASE_URL" \
    "select coalesce(value->>'mode','local'), coalesce(value->>'endpoint',''), coalesce(value->>'bucket',''),
            coalesce(value->>'prefix',''), coalesce(value->>'accessKeyId','')
       from app_settings where key = 'storage_config'")
  # No row = the app never left the local default.
  [ -n "$row" ] || { STORAGE_MODE=local; return; }
  IFS="$PG_FS" read -r STORAGE_MODE S3_ENDPOINT S3_BUCKET S3_PREFIX S3_ACCESS_KEY <<<"$row"
  case "$STORAGE_MODE" in
    local) ;;
    internal) internal_target ;;
    s3) external_creds ;;
    *) die "unknown storage mode \"$STORAGE_MODE\" in app_settings" ;;
  esac
}

storage_from_manifest() {
  local file="$1"
  STORAGE_MODE=$(manifest_get "$file" storage_mode)
  case "$STORAGE_MODE" in
    local) ;;
    internal) internal_target ;;
    s3)
      # The snapshot names the bucket it came from; the TALARIA_BACKUP_S3_*
      # overrides point a restore somewhere else — a drill bucket, or a new
      # provider you're migrating to.
      S3_ENDPOINT="${TALARIA_BACKUP_S3_ENDPOINT:-$(manifest_get "$file" storage_endpoint)}"
      S3_BUCKET="${TALARIA_BACKUP_S3_BUCKET:-$(manifest_get "$file" storage_bucket)}"
      S3_PREFIX="${TALARIA_BACKUP_S3_PREFIX-$(manifest_get "$file" storage_prefix)}"
      external_creds
      ;;
    *) die "manifest has no usable storage_mode" ;;
  esac
}

manifest_get() { grep "^$2=" "$1" | head -1 | cut -d= -f2- || true; }

# Run an `mc` command line against the resolved bucket, with the alias already
# set up as `t`. Host paths are mounted at the SAME path inside the container so
# one command string works either way. --config-dir keeps a host `mc` from
# touching the operator's ~/.mc.
mc_run() {
  local dir="$1" cmd="$2"
  local script="mc --config-dir /tmp/mc-talaria alias set t \"\$S3_ENDPOINT\" \"\$S3_ACCESS_KEY\" \"\$S3_SECRET_KEY\" >/dev/null && $cmd"
  export S3_ENDPOINT S3_ACCESS_KEY S3_SECRET_KEY # -e VAR / sh -c both read them from here
  if command -v mc >/dev/null 2>&1; then
    sh -c "$script"
  else
    docker run --rm --network host --user "$(id -u):$(id -g)" \
      -e S3_ENDPOINT -e S3_ACCESS_KEY -e S3_SECRET_KEY -e HOME=/tmp \
      -v "$dir:$dir" --entrypoint sh "$MC_IMAGE" -c "$script"
  fi
}

# Every blob key the app writes lives under "<prefix>uploads/" (uploads.ts).
bucket_uploads_path() { printf 't/%s/%suploads' "$S3_BUCKET" "$S3_PREFIX"; }
