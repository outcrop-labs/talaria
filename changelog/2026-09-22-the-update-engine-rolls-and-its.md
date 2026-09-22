- **The update engine rolls — and its panel and schedule arrive with it.**
  One update, start to finish, split across two processes by design: the
  live container gates (mode, adoption, nothing in flight), takes the
  fleet-wide roll lock (a redis lease held by heartbeat and never released
  — the roller's last act stops its own container, so the TTL is what
  bounds a dead one), pulls the digest-pinned image, renders, brings the
  other slot up, health-gates it (the compose healthcheck IS the gate — an
  unhealthy replacement is removed with the old container never disturbed),
  moves the fleet alias, and only then records cutting-over and stops
  ITSELF; the new container's boot reconcile verifies the edge actually
  routes to it before the run lands done. Rollback restarts the kept old
  slot by the same choreography, and a run whose roller died mid-roll is
  closed as failed so no install spins forever. `/api/admin/updates`
  surfaces all of it (running version + slot, available, history, the
  auto-update toggle — default off) for an admin session or a per-instance
  machine key minted for the deploy script and stored only as a hash.
  The update-check job rides the scheduler table again (6h cadence; the
  hold is retired) as the hands-off half: resolve, record, and — only
  behind both the adoption and the toggle — roll. healthz now reports the
  image's version so a deploy gate can tell two instances apart. Still
  dormant everywhere: the engine acts only where an admin adopts it.
