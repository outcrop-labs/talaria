- **W12c — one home for the sidecar blocks, and the argv that proves it.**
  `docker/sidecars.compose.yml` holds the six sidecars (postgres, redis, qdrant,
  embeddings, minio, searxng: image, restart, volumes, env defaults,
  healthchecks) and deliberately holds NO `ports`, `networks` or
  `container_name` — those stay per-stack (prod owns the network, devbox owns
  the loopback ports, every stack owns its container names). The three stack
  files consume it with the fragment **first**
  (`-f docker/sidecars.compose.yml -f <stack>.yml`): compose merges by service
  name with the later file winning, and the reverse order would hand every
  devbox a second TEI + SearXNG. The cli builds those argvs through
  `stackComposeFiles` / `composeFileArgs`.
  **The proof is a diff, not an opinion**: `docker compose config` for all three
  stack files, with the fragment, is BYTE-IDENTICAL to the pre-change
  single-file spec (same 8/6/5 services, same published ports, same network
  names, same volume names — so a running instance's volumes are neither
  recreated nor lost), and `bun talaria deploy status` really ran
  `docker compose -f docker/sidecars.compose.yml -f docker/compose.yml ps`
  against docker (exit 0). A bare `docker compose -f docker/compose.yml config`
  now FAILS FAST (`service "qdrant" has neither an image nor a build context`)
  instead of silently starting a stack with no sidecars — kept on purpose. The
  one-shot migration for an operator who sets `COMPOSE_FILE` is documented in
  `docs/CONTAINER.md`: an explicit `-f` beats the env var, so the env list must
  name the fragment first. `cd cli && bun test` 195/0, cli `tsc --noEmit` clean.
  The box template's need for `BOX_*` env is pre-existing (it was never runnable
  bare).
