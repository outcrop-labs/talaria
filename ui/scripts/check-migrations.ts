// Migration replay — CI's proof that the MIGRATIONS array actually runs, plus
// the schema-snapshot ratchet that puts a real schema diff in every PR.
//
//   DATABASE_URL=postgres://talaria:talaria@127.0.0.1:55441/talaria \
//   TALARIA_SECRET_KEY=ci-dummy bun run migrations:check
//
// What it does, in order:
//   1. Runs the full migration pass (migrate()) and asserts schema_migrations
//      carries exactly one row per statement — the array is the whole schema.
//   2. pg_dump --schema-only from PG_DUMP (default: inside the talaria-mig-pg
//      container, so the dump tool is always the server's own version — CI and
//      prod both run postgres:16-alpine).
//   3. Diffs the normalized dump against src/server/db/schema.snapshot.sql.
//      Drift fails with the regen command; `--update-snapshot` rewrites it.
//
// The second-run check ("applied: 0") is a second PROCESS, not a second call —
// migrate() memoizes its promise on globalThis, so an in-process re-run proves
// nothing. CI runs this script twice for exactly that reason.
//
// TALARIA_SECRET_KEY is required even for a schema-only replay: the migration
// pass ends by loading the secretbox (initSecretbox), which throws without it.
import { execFileSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { migrate, getSql } from '../src/server/db/pg'

const HERE = dirname(new URL(import.meta.url).pathname)
const SNAPSHOT = join(HERE, '../src/server/db/schema.snapshot.sql')
const UPDATE = process.argv.includes('--update-snapshot')
// Anything pg_dump prints that starts with "--" is a version header or section
// marker whose minor version floats with the postgres image — keeping it would
// make every 16.x bump look like schema drift. Lines starting with "\" are psql
// meta-commands (\restrict and friends): the 2025 pg_dump security backport
// emits a RANDOM restrict token per dump, so they can never be part of a
// stable comparison.
const normalize = (dump: string) =>
  dump
    .split('\n')
    .filter((l) => l.trim() !== '' && !l.trim().startsWith('--') && !l.trim().startsWith('\\'))
    .join('\n')
    .trim() + '\n'

const { applied, total } = await migrate()
const sql = getSql()
const [{ count }] = (await sql`select count(*)::int as count from schema_migrations`) as unknown as Array<{ count: number }>
await sql.end({ timeout: 5 })
if (count !== total) {
  console.error(`[migrations] schema_migrations has ${count} rows but the array has ${total} — a statement ran without being recorded`)
  process.exit(1)
}

// The default assumes CI's container (talaria-mig-pg); locally, point PG_DUMP
// at the dev sidecar: PG_DUMP='docker exec talaria-postgres-dev pg_dump -U talaria'
const dumpCmd = process.env.PG_DUMP ?? 'docker exec talaria-mig-pg pg_dump -U talaria'
const flags = ['--schema-only', '--no-owner', '--no-privileges', '--no-comments', 'talaria']
const dump = execFileSync('/bin/sh', ['-c', `${dumpCmd} ${flags.join(' ')} 2>/dev/null`], {
  encoding: 'utf8',
  maxBuffer: 64 * 1024 * 1024,
})

const got = normalize(dump)
if (UPDATE) {
  const header = [
    '-- Talaria schema snapshot — the schema a fresh database reaches after the',
    '-- full MIGRATIONS array in ui/src/server/db/pg.ts runs. Regenerate after an',
    '-- intentional migration: cd ui && bun run migrations:snapshot',
    '-- (then commit the diff — it is the PR\'s schema change, in review form.)',
    '',
  ].join('\n')
  writeFileSync(SNAPSHOT, header + got)
  console.log(`[migrations] applied: ${applied}, total: ${total}; snapshot updated`)
} else if (got !== normalize(readFileSync(SNAPSHOT, 'utf8'))) {
  console.error('[migrations] schema snapshot drifted — the MIGRATIONS array and the committed snapshot disagree.')
  console.error('              if the migration change is intentional, run `cd ui && bun run migrations:snapshot`')
  console.error('              and commit the diff as the PR\'s schema change.')
  process.exit(1)
} else {
  console.log(`[migrations] applied: ${applied}, total: ${total}; schema matches snapshot`)
}
