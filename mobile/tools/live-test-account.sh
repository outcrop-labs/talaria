#!/usr/bin/env bash
# mobile/tools/live-test-account.sh — provision a dedicated, least-privilege
# account for the live integration tests (mobile/.../LiveInstanceTest.kt).
#
#   bash mobile/tools/live-test-account.sh http://instance:6302 you@example.com
#   bash mobile/tools/live-test-account.sh http://instance:6302 you@example.com --admin
#
# WHY A SCRIPT AND NOT A README STEP. Two secrets are in play and neither should
# ever reach a shell history, a terminal transcript or an agent's context:
#
#   1. YOUR admin password, which this reads from a prompt with `read -s` — it
#      is never echoed, never passed as an argument (argv is world-readable in
#      /proc), and never written anywhere.
#   2. The TEST account's password, which this GENERATES rather than asks for,
#      and writes only into a 0600 env file. It is never printed. The script
#      tells you the path; the test run sources it. So nobody — including
#      whatever agent is driving the tests — has to see it to use it.
#
# LEAST PRIVILEGE BY DEFAULT. The account is created as a `member`. Pass
# --admin to promote it, which is the only way the admin-only half of
# /api/home's shape (`alerts`, `costToday`) gets exercised — those are null for
# a member by design. Promoting a test account on a production instance is a
# real decision, so it is opt-in rather than convenient.
#
# IDEMPOTENT-ISH: re-running resets the test account's password to a fresh one
# (PUT) rather than failing on the 409 a duplicate create would return.
set -euo pipefail

ORIGIN=${1:-}
ADMIN_EMAIL=${2:-}
MAKE_ADMIN=no
[ "${3:-}" = "--admin" ] && MAKE_ADMIN=yes

if [ -z "$ORIGIN" ] || [ -z "$ADMIN_EMAIL" ]; then
  echo "usage: $0 <origin> <your-admin-email> [--admin]" >&2
  exit 64
fi

ORIGIN=${ORIGIN%/}
TEST_EMAIL=${TALARIA_TEST_EMAIL:-mobile-tests@talaria.local}
OUT_DIR=${XDG_CONFIG_HOME:-$HOME/.config}/talaria
OUT=$OUT_DIR/mobile-live-test.env

jarfile=$(mktemp)
cleanup() { rm -f "$jarfile"; }
trap cleanup EXIT

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

# --- who you are -----------------------------------------------------------
say "Signing in as $ADMIN_EMAIL on $ORIGIN"
printf 'admin password (not echoed): '
read -rs ADMIN_PASSWORD
printf '\n'

# The password goes in on stdin via @-, never on the command line.
login_body=$(printf '{"username":%s,"password":%s}' \
  "$(printf '%s' "$ADMIN_EMAIL" | python3 -c 'import json,sys;print(json.dumps(sys.stdin.read()))')" \
  "$(printf '%s' "$ADMIN_PASSWORD" | python3 -c 'import json,sys;print(json.dumps(sys.stdin.read()))')")
unset ADMIN_PASSWORD

code=$(printf '%s' "$login_body" | curl -sS -o /dev/null -w '%{http_code}' \
  -c "$jarfile" -H 'Content-Type: application/json' --data-binary @- \
  "$ORIGIN/api/auth/password")
unset login_body
[ "$code" = "200" ] || { echo "sign-in failed ($code) — wrong password, or not an admin" >&2; exit 1; }

# A session that is not an admin's cannot do the rest, and finding that out
# from a 403 three calls later is worse than finding it out now.
role=$(curl -sS -b "$jarfile" "$ORIGIN/api/auth/session" | python3 -c \
  'import json,sys;print(json.load(sys.stdin).get("user",{}).get("role",""))' 2>/dev/null || echo "")
[ "$role" = "admin" ] || { echo "that account is '${role:-unknown}', not an admin" >&2; exit 1; }

# --- the test account ------------------------------------------------------
# 32 url-safe characters from the OS CSPRNG. Generated, never asked for, so
# there is no human-chosen password to leak or reuse.
TEST_PASSWORD=$(python3 -c 'import secrets;print(secrets.token_urlsafe(24))')

say "Creating $TEST_EMAIL"
body=$(printf '{"email":"%s","password":"%s","name":"Mobile integration tests"}' "$TEST_EMAIL" "$TEST_PASSWORD")
response=$(printf '%s' "$body" | curl -sS -w '\n%{http_code}' -b "$jarfile" \
  -H 'Content-Type: application/json' --data-binary @- \
  "$ORIGIN/api/admin/password-accounts")
code=$(printf '%s' "$response" | tail -1)
payload=$(printf '%s' "$response" | sed '$d')

user_id=""
case "$code" in
  200)
    user_id=$(printf '%s' "$payload" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("userId",""))')
    echo "  created"
    ;;
  409)
    echo "  already exists — resetting its password instead"
    user_id=$(curl -sS -b "$jarfile" "$ORIGIN/api/admin/password-accounts" | python3 -c "
import json,sys
accounts = json.load(sys.stdin).get('accounts', [])
print(next((a.get('userId') or a.get('id','') for a in accounts
            if a.get('email','').lower() == '$TEST_EMAIL'.lower()), ''))
")
    [ -n "$user_id" ] || { echo "could not find the existing account's id" >&2; exit 1; }
    reset=$(printf '{"userId":"%s","password":"%s"}' "$user_id" "$TEST_PASSWORD")
    rc=$(printf '%s' "$reset" | curl -sS -o /dev/null -w '%{http_code}' -b "$jarfile" -X PUT \
      -H 'Content-Type: application/json' --data-binary @- \
      "$ORIGIN/api/admin/password-accounts")
    [ "$rc" = "200" ] || { echo "password reset failed ($rc)" >&2; exit 1; }
    ;;
  *)
    echo "create failed ($code): $payload" >&2
    exit 1
    ;;
esac

if [ "$MAKE_ADMIN" = "yes" ] && [ -n "$user_id" ]; then
  say "Promoting to admin (--admin was passed)"
  rc=$(printf '{"userId":"%s","role":"admin"}' "$user_id" | curl -sS -o /dev/null -w '%{http_code}' \
    -b "$jarfile" -X PUT -H 'Content-Type: application/json' --data-binary @- \
    "$ORIGIN/api/admin/users")
  [ "$rc" = "200" ] || echo "  warning: promotion failed ($rc); the account is still a member" >&2
fi

# --- hand it over, without printing it -------------------------------------
mkdir -p "$OUT_DIR"
umask 077
cat >"$OUT" <<ENVFILE
# Generated by mobile/tools/live-test-account.sh — 0600, do not commit.
# Source this to run the authed live tests; the password is deliberately not
# printed anywhere else.
export TALARIA_LIVE_ORIGIN=$ORIGIN
export TALARIA_LIVE_USERNAME=$TEST_EMAIL
export TALARIA_LIVE_PASSWORD=$TEST_PASSWORD
ENVFILE
chmod 600 "$OUT"
unset TEST_PASSWORD

cat <<DONE

$(printf '\033[1m==> ready\033[0m')
  account   $TEST_EMAIL  ($([ "$MAKE_ADMIN" = yes ] && echo admin || echo member))
  env file  $OUT  (0600)

Run the live tests with:

  set -a; . $OUT; set +a
  cd mobile && ./gradlew :shared:jvmTest --rerun-tasks

The generated password is in that file and nowhere else — not in your shell
history, not on stdout, not in any transcript. Revoke the account from
Admin → People when you are done with it.
DONE
