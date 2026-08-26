# Self-hosting Talaria — golden image on Proxmox (openSUSE MicroOS)

For running Talaria on your own hardware at more-than-one-instance scale: build a
template once, spin instances that install current Talaria on their own first
boot, and steer each instance with environment variables set at the Proxmox
level. For a single hand-run install (any Linux box with Docker, Node ≥ 20 and
Bun), the [quick start](../README.md#quick-start) in the README is the shorter
path; what follows is the image-based flow.

The pieces live in [`scripts/image/`](../scripts/image/):

| File | Runs where | What |
|---|---|---|
| `build.sh` | Proxmox host | Downloads the MicroOS cloud image, provisions a build VM, turns it into a template |
| `provision.sh` | inside the build VM | System packages (docker, tailscale, firewalld rules, bun), split across a transactional-update reboot |
| `firstboot.sh` | inside every instance, first boot | Tailnet join, clone, install, app unit |
| `bootstrap.sh` | inside every instance, from the clone | The user-level install steps (`setup.sh` + infra + build) |
| `talaria.service` | inside every instance | The app under systemd |
| `talaria-firstboot.service`, `talaria-provision.service` | image units | Drive the two scripts above |

The image carries **no Talaria checkout and no credentials**. Every instance
clones the repo at its own first boot, so instances always install what is
current and the image only goes stale when the base OS or system packages do.

## Build the template

Once, on the Proxmox host (as root, from a repo checkout):

```bash
pvesm set local --content snippets    # one-time; build.sh refuses to guess this for you
git clone https://github.com/outcrop-labs/talaria
cd talaria && sudo ./scripts/image/build.sh
```

Defaults: VMID 9000, `local-lvm` storage, `vmbr0` bridge, 4 cores / 16 GB /
100 GB — override with `--vmid --storage --bridge --cores --memory --disk`, and
`--sshkeys ~/.ssh/id_ed25519.pub` to log into the build VM while it runs.

Inside, the VM adds Tailscale's signed openSUSE repo, installs docker + compose
v2 + tailscale + firewalld into a new transactional snapshot, reboots into it,
then configures everything else and powers off. `build.sh` waits for that
poweroff (up to 45 minutes — package install plus image pre-pulls), then runs
`qm template`. If provisioning hangs or fails, the script says so, leaves the VM
**running** for inspection, and prints the `qm terminal` command — a failed
build never quietly becomes a template.

Before templating, the VM is scrubbed into a clean slate: cloud-init state,
machine-id and SSH host keys are reset (every clone applies its *own* snippet
and gets its *own* identities), the journal is vacuumed, and the hostname is
neutralized.

Sizing: 4 vCPU / 8–16 GB / 100 GB is comfortable. 8 GB is the practical floor —
the first boot runs a vite build next to Postgres, Qdrant and the embeddings
model.

## Spin an instance

```bash
qm clone 9000 301 --name tal-eu-1 --full
qm set 301 --sshkeys ~/.ssh/id_ed25519.pub
# optional but usual — per-instance env vars, next section:
qm set 301 --cicustom user=local:snippets/tal-eu-1.yml
qm start 301
```

First boot (watch `journalctl -u talaria-firstboot -f` via ssh, or give it
10–20 minutes untouched):

1. If `TS_AUTHKEY` is set: joins your tailnet as `TALARIA_HOSTNAME` (or the VM
   hostname), then removes the spent key from the env file — auth keys are
   single-use. Failure here is non-fatal: the box comes up LAN-only.
2. Clones `TALARIA_REPO` at `TALARIA_REF` (defaults: this repo, `main`) to
   `~/talaria`.
3. Runs `scripts/setup.sh` + infra containers + the production build — the
   same steps as the manual runbook, with one addition: a re-entrancy check
   that repairs a half-installed `node_modules`, because `setup.sh` skips
   installs whose output directory merely exists and an interrupted first boot
   would otherwise poison every retry.
4. Installs and starts `talaria.service`, then writes the done-marker.

The unit retries transient failures (network settling, docker warming up);
every step is idempotent, so retries converge. When it's done, Talaria answers
on `http://<vm-ip>:5273`. The generated admin credentials are in the firstboot
journal and in `ui/.env` on the box.

Re-run an instance's install (e.g. after pointing `TALARIA_REF` at a new
release): delete `/var/lib/talaria/firstboot-done` and
`systemctl start talaria-firstboot`. It never deletes the checkout — uploads,
the rendered fleet and stack state live inside it and are not in git.

## Environment variables, set at the Proxmox level

The mechanism is a cloud-init snippet per instance — a small YAML file in
Proxmox's snippet storage that cloud-init writes to `/etc/talaria.env` on the
instance:

```yaml
#cloud-config
write_files:
  - path: /etc/talaria.env
    permissions: '0600'
    content: |
      TS_AUTHKEY=tskey-auth-xxxxx-yyyyy     # single-use; stripped after join
      TALARIA_HOSTNAME=tal-eu-1
      TALARIA_REF=v0.3.1
      # Anything below reaches the app process verbatim. Real environment
      # wins over ui/.env (server-entry.js skips keys already in process.env),
      # so this file is the override channel:
      AUTH_ADMIN_EMAILS=you@yourco.com
      TALARIA_SECRET_KEY=...                # pin to survive rebuilds/restores
```

Four variables steer the bootstrap itself:

| Var | Default | Effect |
|---|---|---|
| `TS_AUTHKEY` | — | Join the tailnet on first boot. Use short-lived/ephemeral keys; the spent key is removed automatically. |
| `TALARIA_HOSTNAME` | VM hostname | Tailscale node name. (The app itself doesn't read it.) |
| `TALARIA_REPO` | `https://github.com/outcrop-labs/talaria.git` | Clone source — a fork, a mirror, a private repo with a deploy key. |
| `TALARIA_REF` | `main` | Branch, tag or commit sha to install. |

Everything else passes straight through to the app (`EnvironmentFile=` in
`talaria.service`). The full app-side inventory is [`ui/.env.example`](../ui/.env.example):
`AUTH_*` (password users, Google OAuth, admin designations), `TALARIA_SECRET_KEY`
(the encryption root — set it here if you ever want to restore a database onto a
rebuilt instance), `SEARXNG_URL`, `TALARIA_S3_*`, `TALARIA_TRUST_PROXY`, …

Treat the snippet store as secret-bearing: it lives on Proxmox storage in
plain text. Prefer ephemeral Tailscale keys, and consider pre-setting
`AUTH_USERS`/`AUTH_ADMIN_EMAILS` here rather than fishing generated credentials
out of the firstboot journal.

## Firewall posture

Configured in the image, from `scripts/image/provision.sh`:

| Source | Ports | Why |
|---|---|---|
| LAN bridge | 5273/tcp (the web UI), ssh, 41641/udp (Tailscale direct) | Chosen posture: UI on LAN **and** tailnet. If your bridge is routable beyond your LAN, scope 5273 with a source-CIDR rich rule instead. |
| Docker bridges (`172.16.0.0/12`) | 5273, 5280 | Agents reach the app and the MCP toolkit container→host — the INPUT-chain rule from [`AGENT-NETWORKING.md`](./AGENT-NETWORKING.md); without it agents hang, they don't error. |
| `tailscale0` | trusted | The tailnet is an authenticated private overlay; the app and SSH are reachable from it wholesale. |

One standing tax of app-on-host + firewalld: **any later `firewall-cmd --reload`
by an operator re-breaks docker's iptables rules until docker restarts** (`systemctl
restart docker`). If agents suddenly go toolless after firewall work, that is
why — restart docker, then the affected agents (an agent connects its MCP
servers at startup and does not retry).

## Updating an instance

The in-app updater is disabled (`TALARIA_UPDATER=off`): its restart helper
would fight systemd, which owns restarts on an image instance. Updates are a
one-liner on the box:

```bash
cd ~/talaria && git pull && bun install && bun run build && sudo systemctl restart talaria
```

The restart applies any new migrations before the server listens again — expect
a minute or two of downtime on a schema-changing release, not a bug.

## TLS

Default posture is plain http on LAN + tailnet, so `talaria.service` sets
`COOKIE_SECURE=0`: production would otherwise mark session cookies `Secure`,
which browsers refuse over http, and login would silently break
(`ui/src/server/auth/session.ts`). When you front an instance with a TLS
proxy (Caddy/nginx, or Proxmox's own), remove that override and restart —
and put the real origin in the proxy, since OAuth callbacks and share links
derive from it.

## Where things live (instance)

| Path | What |
|---|---|
| `/home/talaria/talaria` | The checkout — app, uploads (`ui/.uploads/`), rendered fleet, `ui/.env` |
| `/home/talaria/.bun` | Bun (survives transactional updates; `/home` is unsnapshotted) |
| `/etc/talaria.env` | Proxmox-injected env (0600; systemd reads it as root) |
| `/etc/talaria/*.sh` | The firstboot/provision scripts the image shipped |
| `/var/lib/talaria/` | Done-markers; delete `firstboot-done` to re-install |
| `/var/lib/docker` | Infra containers + volumes (Postgres, Redis, Qdrant, MinIO) |

A note on privilege: the app user `talaria` is the cloud-init user, which on
this image base typically carries passwordless sudo — convenient for ops over
ssh, but it means the account the app runs as can become root. For stricter
instances, strip it (`rm /etc/sudoers.d/*cloud-init*`, `gpasswd -d talaria
wheel`) and do privileged ops over the Proxmox console instead.

**Back up `TALARIA_SECRET_KEY` somewhere a snapshot isn't** — off the Proxmox
host entirely. Every stored secret is sealed with it; a restored database
without it cannot read its own secrets ([`ENCRYPTION.md`](./ENCRYPTION.md)).
Instance backups otherwise: [`scripts/backup.sh`](../scripts/backup.sh) and
[`BACKUPS.md`](./BACKUPS.md).
