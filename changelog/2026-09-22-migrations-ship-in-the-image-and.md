- **Migrations ship in the image — and bad ones die before (or at) the
  door.** The `MIGRATIONS` array is now the stated single channel for schema
  changes AND one-time data operations (a backfill, a watermark reset, a
  repair): append a statement, redeploy, done — running psql by hand on a
  customer database is never part of a release, because a fix that lived only
  in a shell history did not ship. Two CI gates back the law: a new
  `migrations` workflow boots a scratch `postgres:16-alpine` (the exact image
  the sidecars run), replays the entire array from zero, runs the pass a
  second time requiring `applied: 0`, and diffs the resulting schema against
  a committed snapshot (`ui/src/server/db/schema.snapshot.sql`, regenerate
  with `bun run migrations:snapshot`) — so every schema-touching PR carries
  the real schema diff as its review artifact, and a statement with a syntax
  error or a bad column reference can no longer pass CI and explode only at
  customer boot. The invariant check adds a destructive-statement guard: any
  statement that can destroy or rewrite data (drop table/column/type,
  truncate, an unscoped delete or update, an `alter column … type`, a
  rename) fails CI unless it carries an inline `-- deliberate: <why>`
  comment — the marker is the second look, recorded next to the SQL it
  justifies. And a FAILED boot migration pass now reports `migrations: !ok`
  on `/api/healthz` (503), flipping the compose healthcheck unhealthy
  instead of a green container whose every table query 500s; slow-but-running
  passes stay non-fatal, because slow is not failed. The docs that claimed
  the array was frozen and sqlx owned new migrations — never implemented —
  now describe the array as the live single channel it has always been.
