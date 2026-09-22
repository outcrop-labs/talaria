- **A `COMPOSE_FILE` that omits the sidecar plane is repaired, not fatal.**
  The die that met the fragment-less exports (2026-09-21's two customer VMs)
  was legible but left every existing installation unable to update or deploy
  until a human re-exported on each host — and re-ran `talaria service
  install`, since the unit bakes the env into /etc. `composeFileEnv` now
  prepends `docker/sidecars.compose.yml` to the list — the plane is half the
  project, not an operator override, and prepend preserves the operator's own
  layering exactly — warns once with the corrected export spelled out, and
  writes the repaired list back into the env, so the docker child (which
  reads `COMPOSE_FILE` from the inherited environment), the printed
  equivalent, and the captured systemd unit all carry it; the write-back is
  also the once-only gate. Verified: the CLI suites (`cd cli && bun test` —
  sidecars/deploy/service flipped from die-expectations to repair-expectations,
  including the incident command `deploy update` running through to the up,
  and the unit capturing `Environment=COMPOSE_FILE=` with the fragment
  prepended); the real CLI from the checkout with the incident export
  (`docker/compose.yml:docker/compose.registry.yml:docker/compose.vm.yml`)
  against a PATH-stubbed docker — `deploy status` and the full `deploy
  update` hand docker the repaired env, argv carries no `-f`; `bun run
  verify` green.
