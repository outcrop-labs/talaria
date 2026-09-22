- **`talaria` — one TypeScript CLI for everything the bash scripts did.**
  ~1,600 lines of shell across eleven scripts are now a zero-dependency
  `cli/` package run directly under bun (`bun talaria …`), with a flat
  tree: `setup`, `dev`, `worktree`, `reset`, `box`, `deploy`,
  `backup`/`restore`. Every ported script is deleted in the same commit
  that replaces it and every doc reference rewritten — no shims, no dual
  sources of truth. The CLI is dependency-injected end to end
  (`run(ctx, args)` with exec/pipe/readLine/env/isTTY/now behind one Ctx
  interface), so all 126 of its tests run against planted process
  behavior with no mocks. Safety rails survive structurally rather than
  by careful quoting: reset's SQL runs as `psql -c` argv with
  `ON_ERROR_STOP=1` (the bash's stdin-swallowing `read` trap is
  impossible now), restore streams `gunzip | psql` (dumps never sit in a
  buffer), and secrets land only in 0600 files. Two latent bash bugs the
  port's live round-trips surfaced are fixed: worktree creation died on
  container-name conflicts with the main stack whenever the main stack
  was up (its compose up is now scoped to the two services a worktree
  actually owns), and `dev` inside a worktree silently adopted the MAIN
  stack's containers under the default compose project (it now aims at
  `talaria-wt-<name>` with ports lifted from the worktree's own ui/.env).
  `deploy` wraps exactly the commands CONTAINER.md documents and prints
  each one before running it — a test reads the doc at runtime and
  asserts the argv is character-for-character identical, so the doc and
  the CLI cannot drift; it also resolves `DOCKER_GID` from the socket and
  warns when a shell export is missing from `docker/.env` (the drift trap
  the doc warns about, detected before it bites). `setup` now also installs
  a bare `talaria` command on your PATH — a two-line sh shim in bun's bin
  dir that runs the invoking checkout's CLI from anywhere (re-running
  setup elsewhere repoints it; `TALARIA_BIN_DIR` overrides the location),
  with a warning when the target dir isn't on PATH. Legacy checkouts:
  re-run `bun talaria setup` to repoint the `git wt` alias at the CLI.
  The runtime-coupled scripts (`update-restart.mjs`, `chassis.template.yml`,
  `skills/`, the smoke/invariant checks) stay where they are — CI and the
  prod image call them by path.
