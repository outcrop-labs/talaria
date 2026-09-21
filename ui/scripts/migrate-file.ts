// One migration pass from an arbitrary pg.ts module — the building block of
// CI's upgrade check: apply origin/main's array against a scratch database
// with this, then run this PR's array against the SAME database. Any throw —
// including the append-only checksum guard that refuses to boot a deployed
// instance whose ledger disagrees mid-array — exits non-zero with the error,
// which is exactly the failure the fleet would otherwise hit first.
//
//   DATABASE_URL=postgres://talaria:talaria@127.0.0.1:55442/talaria \
//   TALARIA_SECRET_KEY=ci-scratch-key \
//     bun run migrate-file src/server/db/pg.main-upgrade.ts
//
// Own process per module on purpose: migrate() memoizes its promise on
// globalThis, so importing two pg.ts variants into one process would have
// the second call return the first module's result.
import { resolve } from 'node:path'

const target = process.argv[2]
if (!target) {
  console.error('usage: bun run migrate-file <path-to-pg.ts>')
  process.exit(2)
}

const mod = (await import(resolve(target))) as typeof import('../src/server/db/pg')
const { applied, total } = await mod.migrate()
console.log(`migrate-file ${target}: applied: ${applied}, total: ${total}`)
