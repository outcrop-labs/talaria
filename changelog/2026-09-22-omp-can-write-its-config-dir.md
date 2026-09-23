- **omp can write its config dir.** The shared harness directory
  `/opt/data/workbench/harness/pi` was coming out `root:root` `755`: Docker
  creates the parents of a bind mount into the state volume as root, before
  the entrypoint, and Hermes stage2 does not chown `workbench/` when
  `/opt/data` is already hermes-owned. omp (and Pi) write auth and sessions
  there — `PI_CODING_AGENT_DIR` relocates `~/.omp/agent` — so a non-root
  agent could not start the harness. The render now mounts an s6 cont-init
  hook that hands those directories to `hermes:hermes` mode `0775` on every
  boot, existing volumes included, without a recursive chown of the read-only
  policy files. Oh My Pi sets the same `PI_CODING_AGENT_DIR` as Pi, so the
  policy files are mounted once into the dir omp actually reads, not into
  `~/.omp` (which the old mounts also left root-owned, and which is not on
  the state volume). Verified: `bun run api:check` (fmt, clippy `-D warnings`,
  cargo test on the 1.97.1 pin), `bun run check`, ui and mcp typecheck, and
  the ui suite (1208 tests). In the hermes image a reproduced `root:root` `755`
  config dir that hermes could not write becomes writable after the hook, while a
  bind-mounted policy file keeps its host owner.
