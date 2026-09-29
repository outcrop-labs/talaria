- **`talaria dev` boots from a clean clone again: minio can't take the stack
  down, and a fresh database migrates.** Two independent faults, both of which
  ended in a dev stack that looked started and wasn't.

  **One unpullable sidecar aborted the two services the app cannot boot
  without.** `dev.ts` already knows this rule and states it twice — "a single
  `up` resolves every image before creating any container, so one
  unpullable/broken image would abort postgres/redis/qdrant too" — which is
  why searxng and embeddings each get their own non-fatal `up`. minio was the
  one sidecar still inside the fatal one, and its image is our own GHCR mirror
  of a dead upstream (TALA-20): a machine can pull it only once that mirror has
  run and its package is public. Until then `bun talaria dev` died at `✗ dev
  infra failed to start` with the registry's bare `denied` as the only clue,
  and postgres never started. minio now comes up in its own pass and a failure
  warns — naming the mirror and the `docker login ghcr.io` a private package
  needs — instead of ending the run. Uploads degrade; the app boots.

  **A fresh dev database never migrated.** The pass used to be lazy: it fired
  on the first table-backed `db()` call, and before the api cutover the TS
  server served the tables, so the first page hit migrated. Post-cutover every
  table query is the Rust api's (which owns no DDL by design) and nothing in
  the boot path touches a table. `server-entry.ts` grew an explicit boot pass
  for precisely that — and `server-entry.ts` is the PROD wrapper, which never
  runs under `vite dev`. So `talaria dev` on a new machine came up with zero
  tables: `/claim` 500'd, every api call answered `relation "…" does not
  exist`, and the api's scheduler logged it once a tick, forever. The dev
  middleware now runs the same pass at `configureServer`, off the same
  `migrate` export prod uses, recording a failure on the same globalThis
  channel `/api/healthz` reads — so a broken schema fails the probe in dev the
  way it does in prod. It is not awaited by requests, deliberately: `talaria
  dev` starts vite and then the api, and the server's own table queries already
  await the cached promise inside `ensureMigrated()`.

  Verified: `bun run check` green; `bun test cli/src/cmd/dev.test.ts` 17/17,
  including a new case that plants a `denied` on minio's `up` and asserts
  postgres/redis/qdrant came up in their own pass, that the failure is a
  warning, and that the run still reached the app. The migration half was
  driven against a real empty database (`create database migrate_probe`, a
  vite dev pointed at it, no request made): first boot logged `migrations →
  applied 386 statement(s)` and the database went from 0 to 120 tables; a
  second boot logged `schema already current (386 statements, 595ms)`.

  Still owed, and not code: the minio mirror's one-time dispatch. Its own
  workflow header says so — dispatch `minio-mirror.yml` by hand, then flip
  `talaria-minio` and `talaria-mc` to public — and until that happens the
  pinned refs answer `NAME_UNKNOWN` for everyone, including customer installs.
  This change means that failure no longer takes the database with it.
