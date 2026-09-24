- **Observability shows the host, not just the containers.** A new Host tab
  (and an Overview doorway) reads CPU and load, memory, swap, disk per mount
  fullest-first, and the top processes. The api reads host `/proc` and
  `statvfs` — on the host in dev, and through read-only `/host/proc` plus an
  rslave `/host` bind in the container deploy. A container without those
  mounts says so instead of reporting its own cgroup. Filling disks and
  memory pressure also land on Alerts. Verified: `bun run api:check` and
  `bun run verify`; worktree stack `GET /api/host` 200 on the api and through
  the UI proxy, load within a point of `/proc/loadavg`, root disk 77% on
  `/dev/mapper/fedora-root` against `df`'s 78%, top processes were host
  chrome/omp/cargo, and the Host tab rendered those numbers fullest-first.
