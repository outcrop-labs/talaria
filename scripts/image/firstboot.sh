#!/usr/bin/env bash
# Talaria golden-image first boot. Runs ONCE per instance (the unit's
# Condition checks /var/lib/talaria/firstboot-done, written by this script's
# last successful line), as root, from /etc/talaria/firstboot.sh — placed
# there at image-build time from this repo file.
#
# Shape: join the tailnet if handed a key, clone Talaria at the requested ref,
# run the repo's own install path — scripts/image/bootstrap.sh FROM THE CLONE,
# so the install logic always matches the code it installs — then drop in the
# app unit. The image deliberately ships no checkout: every instance installs
# what is current at its own first boot, and the image only goes stale when
# the base OS or system packages do.
#
# Everything here is idempotent and never destructive: the unit retries this
# on failure, and a re-run must converge, not reset. Uploads (ui/.uploads/),
# the rendered fleet and stack state live INSIDE the checkout and are
# invisible to git — nothing in this tree is ever deleted to "start over".
set -euo pipefail

APP_USER=talaria
APP_HOME=/home/$APP_USER
REPO_DIR=$APP_HOME/talaria
BUN_BIN=$APP_HOME/.bun/bin
# The public clone URL is the default; a fork or mirror overrides it per
# instance with TALARIA_REPO in /etc/talaria.env.
REPO_URL="${TALARIA_REPO:-https://github.com/outcrop-labs/talaria.git}"
REF="${TALARIA_REF:-main}"

say()  { printf '\033[1;36m▸ %s\033[0m\n' "$*"; }
warn() { printf '\033[33m! %s\033[0m\n' "$*"; }

# The Proxmox snippet's write_files output, when the instance got one. The
# unit also passes this file to the app later (EnvironmentFile); sourcing it
# here makes the steering vars visible to clone/tailscale before any of that
# exists.
[ -f /etc/talaria.env ] && . /etc/talaria.env

# runuser sets HOME/SHELL/USER but keeps the caller's PATH — root's PATH has
# no ~/.bun/bin, and setup.sh hard-dies without bun on it. So every user-level
# command goes through this wrapper with the PATH spelled out.
as_user() {
  runuser -u "$APP_USER" -- env PATH="$BUN_BIN:/usr/local/bin:/usr/bin:/bin" HOME="$APP_HOME" "$@"
}

# ── Tailnet ──────────────────────────────────────────────────────────────────
# Non-fatal by design: an expired or typo'd key must not keep the box from
# becoming a working LAN instance.
if [ -n "${TS_AUTHKEY:-}" ]; then
  say "tailscale: joining as ${TALARIA_HOSTNAME:-$(hostname)}"
  # --accept-dns=false: tailscaled must not rewrite this host's resolver — a
  # container host's DNS is load-bearing for the whole stack.
  if tailscale up --auth-key="$TS_AUTHKEY" --hostname="${TALARIA_HOSTNAME:-$(hostname)}" --accept-dns=false; then
    # Auth keys are single-use; a spent one only confuses later re-runs.
    sed -i '/^TS_AUTHKEY=/d' /etc/talaria.env
    # The image's permanent config binds tailscale0 to the trusted zone by
    # name; bind it live too, now that the interface exists.
    firewall-cmd --zone=trusted --add-interface=tailscale0 >/dev/null 2>&1 || true
    say "tailscale: joined"
  else
    warn "tailscale up failed (expired/typo'd key?) — continuing, LAN only"
  fi
else
  say "no TS_AUTHKEY — LAN only"
fi

# ── The checkout ─────────────────────────────────────────────────────────────
if [ -e "$REPO_DIR" ]; then
  if [ -d "$REPO_DIR/.git" ]; then
    # A re-run after a partial first boot: fetch (cheap, and picks up tags
    # moved since) and land on the ref. The checkout is minutes old in this
    # path, so local branch refs cannot be meaningfully stale.
    say "checkout exists → fetching $REF"
    as_user git -C "$REPO_DIR" fetch origin --tags --prune --force
    as_user git -C "$REPO_DIR" -c advice.detachedHead=false checkout "$REF"
  else
    # NEVER rm -rf here: uploads and fleet state live in this directory and
    # are not recoverable from git. A non-repo at this path is a human
    # decision to make over a console, not one for a boot script.
    echo "✗ $REPO_DIR exists but is not a git checkout — refusing to touch it." >&2
    echo "  Move it aside by hand if you know what it is, then re-run." >&2
    exit 1
  fi
else
  say "cloning $REPO_URL ($REF)"
  as_user git clone "$REPO_URL" "$REPO_DIR"
  # checkout after clone (not clone --branch) so REF can be a tag or a commit
  # sha, not just a branch.
  as_user git -C "$REPO_DIR" -c advice.detachedHead=false checkout "$REF"
fi

# ── Install (user level) ─────────────────────────────────────────────────────
say "installing (scripts/image/bootstrap.sh from the clone)"
as_user bash "$REPO_DIR/scripts/image/bootstrap.sh"

# ── The app unit (root) ──────────────────────────────────────────────────────
say "installing talaria.service"
cp "$REPO_DIR/scripts/image/talaria.service" /etc/systemd/system/talaria.service
systemctl daemon-reload
systemctl enable --now talaria

# LAST successful act — everything above had to pass for this to run. Its
# presence is what the unit's ConditionPathExists checks.
mkdir -p /var/lib/talaria
touch /var/lib/talaria/firstboot-done
say "done — Talaria is starting on :5273"
