#!/usr/bin/env bash
# Golden-image provisioning, executed INSIDE the build VM by the
# talaria-provision.service oneshot (enabled --now by the build's cloud-init
# runcmd). Placed at /etc/talaria/provision.sh from this repo file by
# scripts/image/build.sh.
#
# MicroOS is transactional: /usr is read-only until a new snapshot is booted,
# so installing packages and using what they installed are separate stages
# split by a REBOOT. The same enabled unit re-runs this script after the
# reboot (enabled units start at every boot; unit state does not survive
# reboots) and the markers pick up where we left off.
#
# Markers live in /var/lib/talaria because only / is snapshotted on MicroOS —
# /var survives transactional reboots and rollbacks. A marker under /etc or
# /usr could silently vanish across the snapshot boundary and re-trigger a
# stage against a half-built system.
set -euo pipefail

MARKER_DIR=/var/lib/talaria
TS_REPO_URL=https://pkgs.tailscale.com/stable/opensuse/tumbleweed/tailscale.repo
# curl + unzip: bun's installer requires both. qemu-guest-agent: build.sh sets
# agent=1 on the qm side and the guest half must exist for `qm agent`, clean
# shutdowns and IP reporting. openssl: setup.sh's `rand` prefers it.
# docker + docker-compose: every repo script and the fleet renderer shell out
# to `docker`/`docker compose`; podman ships with MicroOS and stays
# installed-but-unused — removing it buys nothing and risks base-image drift.
SYS_PKGS="docker docker-compose git firewalld tailscale qemu-guest-agent curl unzip openssl"

say()  { printf '\033[1;36m▸ %s\033[0m\n' "$*"; }
warn() { printf '\033[33m! %s\033[0m\n' "$*"; }
die()  { printf '\033[31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

mkdir -p "$MARKER_DIR"

# ── Stage 1: system packages, into a new snapshot ────────────────────────────
if [ ! -f "$MARKER_DIR/stage1-done" ]; then
  say "stage 1: system packages (transactional — reboot follows)"

  # Repo config lives in /etc/zypp/repos.d, writable without a transaction.
  # Tailscale's own repo, GPG-pinned (repo_gpgcheck=1 in the .repo file).
  zypper --gpg-auto-import-keys addrepo -f "$TS_REPO_URL" tailscale-stable

  # Node: pick the newest major the distro carries that is >= 20, via dry-run
  # (no transaction created), so a single install transaction carries it.
  # setup.sh hard-requires node >= 20 for the toolchain's node-shebang bins.
  node_pkg=""
  for candidate in nodejs24 nodejs22 nodejs20; do
    if zypper --non-interactive install --dry-run "$candidate" >/dev/null 2>&1; then
      node_pkg="$candidate"
      break
    fi
  done
  [ -n "$node_pkg" ] || die "no nodejs >= 20 found in the distro repos"
  say "node: $node_pkg"

  transactional-update --non-interactive pkg install $SYS_PKGS "$node_pkg"

  # NOTE: no `systemctl enable` here. The unit files for what we just
  # installed do not exist in the RUNNING snapshot's /usr yet — enabling from
  # here fails with "unit not found". Stage 2 (after the reboot into the new
  # snapshot) does it.
  touch "$MARKER_DIR/stage1-done"
  say "rebooting into the new snapshot"
  systemctl reboot
  exit 0
fi

# ── Stage 2: image assembly, in the new snapshot ─────────────────────────────
if [ ! -f "$MARKER_DIR/stage2-done" ]; then
  say "stage 2: image assembly"

  node -e 'process.exit(process.versions.node.split(".")[0] >= 20 ? 0 : 1)' \
    || die "node >= 20 not live after reboot — the transaction did not apply"

  # Services first, firewalld before docker: both program netfilter, and
  # docker inserts its rules at start. firewalld up → rules → docker up is
  # the order that leaves both rule sets coexisting. (Any LATER
  # `firewall-cmd --reload` by an operator re-breaks docker's rules until
  # docker restarts — that tax is documented in SELF-HOSTING.md, not fixable
  # from here.)
  systemctl enable firewalld.service
  systemctl start firewalld.service

  # Docker bridges land in the DEFAULT zone when the network is created —
  # bind the rules to whatever that zone actually is, not a hardcoded name.
  def_zone=$(firewall-cmd --get-default-zone)
  # Agents reach the app + toolkit container→host (docs/AGENT-NETWORKING.md):
  # INPUT-chain rules Docker does not manage. By subnet, not interface —
  # bridge names churn when networks are recreated.
  firewall-cmd --permanent --zone="$def_zone" \
    --add-rich-rule='rule family=ipv4 source address=172.16.0.0/12 port port=5273 protocol=tcp accept'
  firewall-cmd --permanent --zone="$def_zone" \
    --add-rich-rule='rule family=ipv4 source address=172.16.0.0/12 port port=5280 protocol=tcp accept'
  # The web UI, on LAN + tailnet. This opens it to whatever the bridge behind
  # the VM can route — see SELF-HOSTING.md for scoping it to a LAN CIDR.
  firewall-cmd --permanent --zone="$def_zone" --add-port=5273/tcp
  firewall-cmd --permanent --zone="$def_zone" --add-service=ssh
  # Tailscale direct connections (skips this and traffic relays via DERP —
  # works, just slower).
  firewall-cmd --permanent --zone="$def_zone" --add-port=41641/udp
  # The tailnet itself: authenticated private overlay, trusted wholesale.
  # Permanent binding by interface NAME applies whenever the link appears —
  # tailscale0 does not exist yet, and firstboot also binds it live after
  # `tailscale up` as belt-and-braces.
  firewall-cmd --permanent --zone=trusted --add-interface=tailscale0
  firewall-cmd --reload

  systemctl enable docker.service tailscaled.service
  systemctl start docker.service
  docker info >/dev/null 2>&1 || die "docker daemon not answering"

  # The app user. cloud-init usually already created `talaria` (build.sh sets
  # --ciuser talaria); guard anyway. docker group is REQUIRED — the fleet
  # renderer and every repo script drive `docker` as this user.
  id talaria >/dev/null 2>&1 || useradd -m -U -s /bin/bash talaria
  usermod -aG docker talaria

  # Bun, per-user in /home — home is writable without transactions and
  # survives future transactional updates (only / is snapshotted).
  if [ ! -x /home/talaria/.bun/bin/bun ]; then
    say "installing bun"
    runuser -u talaria -- env HOME=/home/talaria bash -c \
      'curl -fsSL https://bun.sh/install | bash'
  fi
  [ -x /home/talaria/.bun/bin/bun ] || die "bun did not install"

  # Compose v2 as a docker SUBCOMMAND. The openSUSE package normally puts the
  # cli-plugin where docker finds it; assert as the app user, and only shim
  # if the assert fails. A broken compose is a build failure here, not a
  # deferred first-boot surprise (setup.sh hard-requires it).
  if ! runuser -u talaria -- env HOME=/home/talaria PATH=/usr/local/bin:/usr/bin:/bin \
      docker compose version >/dev/null 2>&1; then
    say "shimming docker compose subcommand"
    runuser -u talaria -- env HOME=/home/talaria bash -c \
      'mkdir -p ~/.docker/cli-plugins && ln -sf "$(command -v docker-compose)" ~/.docker/cli-plugins/docker-compose'
  fi
  runuser -u talaria -- env HOME=/home/talaria PATH=/usr/local/bin:/usr/bin:/bin \
    docker compose version >/dev/null 2>&1 || die "docker compose unusable as talaria"

  # Pre-pull the infra images into /var/lib/docker — the template disk
  # carries them, so every clone skips the largest network-failure surface
  # and several minutes of pulling. Each is non-fatal: firstboot retries
  # whatever is missing. The agent chassis image is the one that pays for
  # "design an agent and it starts in seconds".
  for image in \
    docker.io/library/postgres:16-alpine \
    docker.io/library/redis:7-alpine \
    docker.io/qdrant/qdrant:latest \
    ghcr.io/huggingface/text-embeddings-inference:cpu-latest \
    docker.io/minio/minio:latest \
    docker.io/searxng/searxng:latest \
    nousresearch/hermes-agent:latest; do
    say "pre-pulling $image"
    docker pull -q "$image" || warn "could not pre-pull $image — firstboot will retry"
  done

  # Enable the firstboot unit HERE, not at build's runcmd time: enabling it
  # any earlier would make the BUILD VM run it on this very reboot — the
  # build VM must stay an empty machine. From now on there are no more
  # reboots before templating, so enabling is safe.
  systemctl enable talaria-firstboot.service

  # ── Template hygiene: make every clone a fresh machine ───────────────────
  # cloud-init clean resets per-instance state so each clone applies its OWN
  # snippet (the build's user-data must not shadow it); --machine-id gives
  # every clone its own DHCP/hostname identity; --logs keeps the provisioning
  # trail out of clones. cc_ssh regenerates host keys on the clone's first
  # boot, but delete them here too so the template never ships the build's.
  cloud-init clean --logs --machine-id
  rm -f /etc/ssh/ssh_host*
  # A neutral hostname: a clone with no --hostname and no snippet hostname
  # would otherwise inherit the build VM's name.
  echo talaria-image > /etc/hostname
  journalctl --rotate >/dev/null 2>&1 || true
  journalctl --vacuum-time=1s >/dev/null 2>&1 || true

  touch "$MARKER_DIR/stage2-done" "$MARKER_DIR/provision-done"
  # The Condition on the unit already stops future runs; disabling removes
  # the temptation to hand-start it on a clone.
  systemctl disable talaria-provision.service
  say "done — powering off for templating"
  systemctl poweroff
fi

say "provision already complete — nothing to do"
