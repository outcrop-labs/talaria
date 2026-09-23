- **A golden-image deploy for Proxmox.** `scripts/image/` builds an openSUSE
  MicroOS template with the system half of an install baked in — Docker +
  compose v2 (podman stays unused), Tailscale, firewalld rules for the
  agent→app path from AGENT-NETWORKING.md, Bun, the guest agent — and no
  Talaria in it at all: every instance cloned from the template installs
  *current* Talaria on its own first boot (clone → setup.sh → infra → build
  → `talaria.service`), so the image can't ship a stale app. Per-instance
  configuration rides cloud-init snippets (`qm set --cicustom` →
  `/etc/talaria.env`): a handful of vars steer the bootstrap (tailnet key,
  instance hostname, repo, ref) and everything else reaches the app process
  verbatim, winning over `ui/.env` the way the environment always has.
  `TALARIA_HOSTNAME` names the whole instance — system hostname via
  hostnamectl plus a hosts(5) entry, not just the Tailscale node name —
  because the cicustom snippet replaces the user-data that would otherwise
  carry one, and every clone would answer to the template's neutral name.
  The install is
  re-entrant — first boot retries converge instead of wedging on a
  half-installed `node_modules` (setup.sh's skip-if-exists can't heal that
  on its own), the app unit gates on Postgres readiness rather than losing
  the boot race to a cached failed migration, and a re-run never deletes the
  checkout, where uploads and fleet state live outside git. systemd owns
  restarts, so the in-app updater stands down (`TALARIA_UPDATER=off`) and
  updates are a one-liner. The SearXNG settings render moved from dev.sh
  into a shared `scripts/render-searxng.sh` so the bootstrap and the dev
  loop mount the same file. `build.sh` assumes nothing about the host it
  runs on: it prompts for the VM-disk storage (from live `pvesm` output)
  and the snippet storage (offering to enable snippets on `local` by
  *merging* content types — `pvesm set --content` replaces the list), and
  when the host has no SSH public keys it generates and names a keypair to
  inject; the same choices exist as flags for non-interactive runs, and
  `--dry-run` prints the resolved plan. Full runbook:
  docs/SELF-HOSTING.md. Verified repo-side: `bash -n` on every new script,
  the generated cloud-init snippet round-trips its four embedded files
  byte-for-byte under YAML literal-block indentation, and the build.sh
  resolution/prompt/keygen flows pass a stubbed-host test matrix
  (`pvesm`/`qm` stubs, pty-driven prompts, dry-run assertions); the image
  build itself runs on the Proxmox host per SELF-HOSTING.md (not
  exercisable from this tree).
