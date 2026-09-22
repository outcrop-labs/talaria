- **A harness of choice installs once, not per box**: every box mounts the shared tools layer
  (`../devboxes/shared/tools` → `/work/tools`, its `bin` first on PATH). `talaria box install
  <name> '<cmd>'` installs into it from any box — flock-held, `NPM_CONFIG_PREFIX` pointed at the
  layer (the image's own global prefix is root-owned; plain `npm i -g` in a box was always an
  EACCES) — and the result is usable from every box, survives `box rm`, and rides into boxes
  created later. `--setup` hooks get the same tools env, so recreate-with-same-`--setup` is a
  no-op, not a re-download.
