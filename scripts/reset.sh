#!/usr/bin/env bash
# Talaria dev reset — the way back when something is wedged.
#
# WHY THIS EXISTS
#   Two failure modes have no in-app recovery, because the app cannot fix either
#   of them from the inside:
#
#     · the encryption root is gone. Every provider key, agent secret and OAuth
#       token in the database is sealed with a key derived from TALARIA_SECRET_KEY.
#       Lose that value and the ciphertext is unrecoverable — not by us, not by
#       anyone. The app is right to refuse; what it cannot do is decide FOR you
#       that losing those secrets is acceptable. That is this script's `secrets`
#       mode: accept the loss, keep everything else, get a working instance back.
#
#     · the database is in a state you no longer want to reason about. Mid-
#       migration, half-seeded, an experiment that went sideways. `database`
#       mode drops it and lets the next boot rebuild from the migration array.
#
#   Neither is reversible, so both print exactly what they will destroy — with
#   live counts from the actual database — and require you to type the mode name
#   to proceed. There is no -y flag on purpose.
#
# USAGE
#   ./scripts/reset.sh secrets    clear unreadable secrets, keep all other data
#   ./scripts/reset.sh database   drop + recreate the database (ALL data)
#   ./scripts/reset.sh fleet      remove rendered agent containers + fleet/
#   ./scripts/reset.sh --help
set -euo pipefail
cd "$(dirname "$0")/.."

say()  { printf '\033[1;36m▸ %s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m! %s\033[0m\n' "$*"; }
die()  { printf '\033[31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

usage() {
  sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}
[ $# -eq 1 ] || usage 1
case "$1" in -h|--help|help) usage 0 ;; esac
MODE="$1"

[ -f ui/.env ] || die "ui/.env missing — nothing to reset (run ./scripts/setup.sh first)"
DB_URL=$(grep -m1 '^DATABASE_URL=' ui/.env | cut -d= -f2- || true)
[ -n "$DB_URL" ] || die "no DATABASE_URL in ui/.env"

# Refuse anything that is not obviously a local dev database. This script
# destroys data by design and has no undo; a typo in DATABASE_URL should not be
# able to take out something real.
case "$DB_URL" in
  *@127.0.0.1:*|*@localhost:*) : ;;
  *) die "DATABASE_URL does not point at localhost — refusing. This script is for dev environments." ;;
esac

PG_CONTAINER="${TALARIA_PG_CONTAINER:-talaria-postgres-dev}"
docker ps --format '{{.Names}}' | grep -qx "$PG_CONTAINER" \
  || die "postgres container '$PG_CONTAINER' is not running — start it with ./scripts/dev.sh"
# NO `-i` here. These run BEFORE the confirmation prompt, and `docker exec -i`
# attaches the caller's stdin — which means each count query would swallow the
# line the operator is about to type, and `read` would then see EOF and abort a
# reset they had correctly confirmed. Only the heredoc calls below need stdin.
psql() { docker exec "$PG_CONTAINER" psql -U talaria -d talaria -tAc "$1" 2>/dev/null || echo "0"; }

confirm() {
  printf '\n\033[33mThis cannot be undone.\033[0m Type \033[1m%s\033[0m to proceed (anything else aborts): ' "$1"
  read -r reply
  [ "$reply" = "$1" ] || die "aborted — nothing was changed"
}

# ── secrets ──────────────────────────────────────────────────────────────────
# Everything sealed with the data key, so the next boot mints a fresh one and
# the app works again. What is cleared is the CIPHERTEXT, not the feature: you
# re-enter provider keys and reconnect accounts, and keep every board, ticket,
# document and conversation.
reset_secrets() {
  say "What this will clear (live counts)"
  printf '  %-42s %s\n' "llm_endpoints (provider API keys)"      "$(psql "select count(*) from llm_endpoints where api_key_cipher is not null")"
  printf '  %-42s %s\n' "agent_secrets (per-agent env secrets)"  "$(psql "select count(*) from agent_secrets")"
  printf '  %-42s %s\n' "agent_keys (per-agent credentials)"     "$(psql "select count(*) from agent_keys")"
  printf '  %-42s %s\n' "google_connections (per-user OAuth)"    "$(psql "select count(*) from google_connections")"
  printf '  %-42s %s\n' "google_org_connection (org OAuth)"      "$(psql "select count(*) from google_org_connection")"
  printf '  %-42s %s\n' "mcp_oauth_tokens (connected accounts)"  "$(psql "select count(*) from mcp_oauth_tokens")"
  printf '  %-42s %s\n' "mcp_user_credentials (headers)"         "$(psql "select count(*) from mcp_user_credentials")"
  printf '  %-42s %s\n' "app_settings: email/storage/github"     "$(psql "select count(*) from app_settings where key in ('email_config','storage_config','github_config')")"
  printf '  %-42s %s\n' "secret_keys (the data keys themselves)" "$(psql "select count(*) from secret_keys")"
  echo
  say "What this will KEEP"
  printf '  %-42s %s\n' "boards"        "$(psql "select count(*) from boards")"
  printf '  %-42s %s\n' "tasks"         "$(psql "select count(*) from tasks")"
  printf '  %-42s %s\n' "users"         "$(psql "select count(*) from users")"
  printf '  %-42s %s\n' "agent_defs"    "$(psql "select count(*) from agent_defs")"
  printf '  %-42s %s\n' "messages"      "$(psql "select count(*) from messages")"
  warn "Agents keep their identities and configuration; they lose their CREDENTIALS."
  warn "After this: re-add provider keys on /models, reconnect Google and MCP accounts,"
  warn "then re-render the fleet so agents get fresh keys (Admin → Agents, or /api/fleet/render)."
  confirm secrets

  say "Clearing"
  # Ciphertext columns first, then the keys themselves — so a crash midway
  # leaves unreadable-but-present rows rather than readable rows with no key.
  docker exec -i "$PG_CONTAINER" psql -U talaria -d talaria -v ON_ERROR_STOP=1 <<'SQL'
begin;
update llm_endpoints set api_key_cipher = null;
delete from agent_secrets;
delete from agent_keys;
delete from google_connections;
delete from google_org_connection;
delete from mcp_oauth_tokens;
delete from mcp_user_credentials;
delete from app_settings where key in ('email_config','storage_config','github_config');
delete from secret_keys;
commit;
SQL
  ok "sealed data cleared — the next boot mints a fresh data key"
  echo
  warn "If you still have the ORIGINAL TALARIA_SECRET_KEY somewhere, stop and use it instead:"
  warn "restoring it recovers everything above. This script is the path when it is genuinely gone."
}

# ── database ─────────────────────────────────────────────────────────────────
reset_database() {
  say "What this will destroy — the ENTIRE database"
  for t in boards tasks users agent_defs messages channels kb_docs artifacts; do
    printf '  %-42s %s\n' "$t" "$(psql "select count(*) from $t")"
  done
  warn "Everything. The next boot replays the migration array into an empty database."
  warn "Uploads on disk or in object storage are NOT touched — only Postgres."
  confirm database

  say "Dropping and recreating"
  docker exec -i "$PG_CONTAINER" psql -U talaria -d postgres -v ON_ERROR_STOP=1 <<'SQL'
select pg_terminate_backend(pid) from pg_stat_activity where datname = 'talaria' and pid <> pg_backend_pid();
drop database if exists talaria;
create database talaria owner talaria;
SQL
  ok "database recreated — restart the app and it will migrate from scratch"
}

# ── fleet ────────────────────────────────────────────────────────────────────
reset_fleet() {
  say "What this will remove"
  printf '  %-42s %s\n' "running agent containers" "$(docker ps --format '{{.Names}}' | grep -c 'talaria-fleet-agent' || true)"
  printf '  %-42s %s\n' "fleet/ (rendered config + keys)" "$([ -d fleet ] && echo present || echo absent)"
  warn "agent_defs in the database are KEPT — this removes only what the renderer produced."
  warn "Re-render afterwards (Admin → Agents) to bring the fleet back."
  confirm fleet

  say "Removing"
  if [ -f fleet/docker-compose.yml ]; then
    docker compose -f fleet/docker-compose.yml down --remove-orphans 2>/dev/null || true
  fi
  docker ps -a --format '{{.Names}}' | grep 'talaria-fleet-agent' | xargs -r docker rm -f >/dev/null 2>&1 || true
  # fleet/.env holds the agent credentials; it is regenerated by the renderer.
  rm -rf fleet/docker-compose.yml fleet/fleet.json fleet/.env fleet/agents
  ok "fleet removed — re-render to rebuild it"
}

case "$MODE" in
  secrets)  reset_secrets ;;
  database) reset_database ;;
  fleet)    reset_fleet ;;
  *)        usage 1 ;;
esac
