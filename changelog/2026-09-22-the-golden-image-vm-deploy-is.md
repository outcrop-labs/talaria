- **The golden-image (VM) deploy is retired.** `scripts/image/` — the Proxmox
  build/bootstrap/firstboot scripts and their systemd units — and
  `docs/SELF-HOSTING.md`, which documented only them, are deleted. The
  container deploy (docs/CONTAINER.md) is the self-hosting path: same app,
  same env-var contract, a compose file instead of a VM template, and it
  doesn't need a Proxmox host to provision one. References repointed
  (README self-hosting pointer, doc index, CONTAINER.md intro).
