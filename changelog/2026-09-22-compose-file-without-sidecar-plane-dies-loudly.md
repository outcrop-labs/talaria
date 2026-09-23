- **A `COMPOSE_FILE` that omits the sidecar plane dies with the fix spelled
  out.** The deploy wrappers honor the operator's `COMPOSE_FILE` by dropping
  their own `-f` pair — the env list replaces both files — so the sidecar
  plane has to ride IN that list. Two customer VMs exported the registry flow
  without it (`docker/compose.yml:docker/compose.registry.yml:
  docker/compose.vm.yml`), and every `bun talaria deploy update` died in
  docker's per-service `service "postgres"/"searxng" has neither an image nor
  a build context` beside a `compose pull` — which the CLI's own die line
  then mislabeled as a pull-reachability problem. `deployCompose` and the
  `service install` guards (the unit bakes the env into `/etc`) now check the
  list first and die naming the fragment and the corrected export.
  Verified: `bun test` on the CLI suites (sidecars/deploy/service, 63 green);
  the real CLI from a checkout — the broken export dies before any docker
  argv with the fix spelled out, the legal list passes through to
  `compose ps` clean; `bun run check` green.
