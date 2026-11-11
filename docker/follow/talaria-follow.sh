#!/usr/bin/env bash
# talaria-follow — keep the testing instance on the image `:testing` names.
#
# WHAT IT DOES, once per run (talaria-follow.timer runs it every few minutes):
#   1. pulls ghcr.io/outcrop-labs/talaria:testing, which testing-deploy.yml
#      moves to each testing commit it has built and smoke-tested;
#   2. reads that image's org.opencontainers.image.revision label — the commit
#      it was built from;
#   3. if that is not what this instance already runs, pins BOTH halves to that
#      commit: the checkout (a detached HEAD at the revision, which must be on
#      origin/testing) and the image (TALARIA_CHANNEL=testing-<sha12> in
#      docker/.env, the immutable tag of the same build);
#   4. runs `talaria deploy up` (registry mode: pull that tag, `up -d`).
#
# WHY THE CHECKOUT FOLLOWS THE IMAGE, NOT THE BRANCH. The compose files come
#   from the checkout and the app from the image, and they change together.
#   `:testing` lags testing's tip by a build (20-30 minutes when the api
#   changed), so a checkout fast-forwarded to the tip would run compose files
#   ahead of the image they configure. Pinned to the image's own revision, the
#   two are always one commit.
#
# THE MINIMUM GAP (FOLLOW_MIN_GAP_MIN, default 10). Every new digest restarts
#   the app and rolls every running agent (docs/UPDATES.md). Merges into
#   testing come minutes apart, so the follower applies at most once per gap;
#   a newer image waits ("waiting: … queued") and the newest one at the end of
#   the gap is what lands.
#
# THE HOLD. `touch ~/.local/state/talaria-follow/hold` pauses deploys for an
#   exercise session; `rm` it to resume. A forgotten hold is the first thing to
#   check when testing-deploy's `verify` times out (docs/TESTING-INSTANCE.md).
#
# INSTALLED BY COPY (/usr/local/bin/talaria-follow), never run from the
#   checkout: the checkout is testing's code, which an admin may merge without
#   review, and the follower is what moves it. A merge cannot change the
#   follower; re-copy it deliberately when docs/TESTING-INSTANCE.md says it
#   changed.
#
# EXIT. 0: applied, nothing to do, waiting out the gap, or held. Non-zero:
#   something to look at (`journalctl -u talaria-follow`) — a revision that is
#   not on origin/testing, local edits in the checkout, a pull or an `up` that
#   failed. A failed apply is retried on the next run.
set -euo pipefail

REPO_DIR=${TALARIA_HOME:-$HOME/talaria}
STATE=${XDG_STATE_HOME:-$HOME/.local/state}/talaria-follow
IMAGE=${FOLLOW_IMAGE:-ghcr.io/outcrop-labs/talaria}
MIN_GAP_MIN=${FOLLOW_MIN_GAP_MIN:-10}
BUN=${FOLLOW_BUN:-$HOME/.bun/bin/bun}

mkdir -p "$STATE"
cd "$REPO_DIR"

# One run at a time: a manual run beside the timer's would race the checkout.
exec 9> "$STATE/lock"
if ! flock -n 9; then
  echo "another talaria-follow run holds $STATE/lock"
  exit 0
fi

if [ -e "$STATE/hold" ]; then
  echo "held: $STATE/hold exists — remove it to resume"
  exit 0
fi

docker pull -q "$IMAGE:testing" > /dev/null
rev=$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' "$IMAGE:testing")
if ! [[ "$rev" =~ ^[0-9a-f]{40}$ ]]; then
  echo "refusing: $IMAGE:testing carries no commit in its revision label (got '$rev')" >&2
  exit 1
fi
tag=testing-${rev:0:12}

# Up to date when both halves already name this build: the last apply that
# finished, and the channel docker/.env pins (a reset sets it back to
# `testing`, which re-pins on the next run).
cur=$(sed -n 's/^TALARIA_CHANNEL=//p' docker/.env 2> /dev/null | tail -n 1)
applied=$(cat "$STATE/applied" 2> /dev/null || true)
if [ "$cur" = "$tag" ] && [ "$applied" = "$tag" ]; then
  exit 0
fi

last=$(cat "$STATE/last-apply" 2> /dev/null || echo 0)
case "$last" in '' | *[!0-9]*) last=0 ;; esac
if [ $(($(date +%s) - last)) -lt $((MIN_GAP_MIN * 60)) ]; then
  echo "waiting: $tag queued (minimum gap ${MIN_GAP_MIN}m since the last apply)"
  exit 0
fi

git fetch --quiet origin testing
if ! git merge-base --is-ancestor "$rev" origin/testing 2> /dev/null; then
  echo "refusing: $rev is not on origin/testing (was testing rewritten?)" >&2
  exit 1
fi
if ! git diff --quiet HEAD --; then
  echo "refusing: the checkout at $REPO_DIR has local edits to tracked files; the follower never overwrites them" >&2
  git status --short --untracked-files=no >&2
  exit 1
fi
git -c advice.detachedHead=false checkout --quiet --detach "$rev"

if grep -q '^TALARIA_CHANNEL=' docker/.env 2> /dev/null; then
  sed -i "s/^TALARIA_CHANNEL=.*/TALARIA_CHANNEL=$tag/" docker/.env
else
  echo "TALARIA_CHANNEL=$tag" >> docker/.env
fi

# Registry mode (COMPOSE_FILE layers docker/compose.registry.yml): pulls the
# immutable $tag and runs `up -d`. A plain `up` never touches git.
echo "applying $tag (checkout and image at ${rev:0:12})"
"$BUN" cli/bin/talaria.ts deploy up

echo "$tag" > "$STATE/applied"
date +%s > "$STATE/last-apply"

# Disk: every earlier testing-<sha12> this host pulled (an image a container
# still uses is refused, and kept), then whatever that left dangling.
docker image ls --format '{{.Repository}}:{{.Tag}}' "$IMAGE" \
  | grep -E ":testing-[0-9a-f]{12}$" | grep -vx "$IMAGE:$tag" \
  | xargs -r docker image rm > /dev/null 2>&1 || true
docker image prune -f > /dev/null || true
echo "applied $tag"
