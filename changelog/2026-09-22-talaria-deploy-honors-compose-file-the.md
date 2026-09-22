- **`talaria deploy` honors `COMPOSE_FILE` — the registry-image flow works
  through the wrappers, no build on the host.** Export the layered file list
  (`docker/compose.yml:docker/compose.registry.yml[:your-vm override]`) and
  every deploy leaf drops its explicit `-f` (docker's own precedence puts -f
  ABOVE the env, so honoring the env means stepping aside); registry mode
  pulls `talaria searxng-config` fail-fast and `up` runs WITHOUT `--build` —
  the override swaps the image but cannot remove the base's `build:` key, so
  a build would tag the checkout as the registry ref. `talaria service
  install` captures the env into the systemd unit, and the env-drift warning
  scans every file in the list (so `TALARIA_CHANNEL` and per-override knobs
  join the checked set). Verified: `bun talaria deploy up/update/down` argv
  under COMPOSE_FILE asserted in the CLI suite (pull-before-up ordering, no
  --build, die-on-pull-failure), unit text with and without the env, drift
  across a layered override, and the unset path byte-identical to before.
