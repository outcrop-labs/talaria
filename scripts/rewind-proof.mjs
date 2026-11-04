#!/usr/bin/env node
// Prints one run's `runs` row and its `run_checkpoints` ledger as a stable
// table, for before-and-after evidence: two prints of a finished run are
// identical, and a diff of two prints is what changed.
//
//   node scripts/rewind-proof.mjs --stack <checkout> --run <run id>
//
// Reads only. A database older than the ledger prints the run and says so, which
// is how a trunk stack compares against a head stack.

import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { parseArgs } from 'node:util'

const RUN_FIELDS = ['id', 'kind', 'state', 'phase', 'attempt', 'crashes', 'checkpoint_seq', 'subject_id', 'error']
const LEDGER_COLUMNS = [
  'seq', 'kind', 'turn', 'step_no', 'status', 'title', 'tool', 'tool_call_id', 'job_id',
  'workspace_commit', 'workspace_tree', 'restorable', 'unrestorable_reason', 'created_at',
]
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

const die = (line) => {
  process.stderr.write(`[proof] ${line}\n`)
  process.exit(1)
}

const { values } = parseArgs({ options: { stack: { type: 'string' }, run: { type: 'string' } } })
if (!values.stack || !UUID.test(values.run ?? '')) die('usage: rewind-proof.mjs --stack <checkout> --run <run id>')

const env = readFileSync(join(resolve(values.stack), 'ui/.env'), 'utf8')
const name = env.match(/^TALARIA_WORKTREE=(.+)$/m)?.[1]
if (!name) die(`${values.stack} is not a worktree stack`)

const psql = (sql) =>
  execFileSync('docker', ['exec', `talaria-pg-${name}`, 'psql', '-U', 'talaria', '-d', 'talaria', '-tAc', sql], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  }).trim()

/** Postgres fixes jsonb's key order, so the digest is stable across prints. */
const digest = (value) =>
  value == null ? '-' : createHash('sha256').update(JSON.stringify(value)).digest('hex').slice(0, 12)
const cell = (value) => (value == null ? '-' : String(value).replaceAll(/\s+/g, ' ').slice(0, 60))

const runJson = psql(`select to_jsonb(r) from runs r where id = '${values.run}'`)
if (!runJson) die(`no run ${values.run} on stack ${name}`)
const run = JSON.parse(runJson)

const out = ['run']
for (const field of RUN_FIELDS) out.push(`  ${field.padEnd(15)}${cell(run[field])}`)
out.push(`  ${'checkpoint'.padEnd(15)}${digest(run.checkpoint)}`)

if (psql(`select to_regclass('public.run_checkpoints') is null`) === 't') {
  out.push('', 'ledger: absent, this database has no run_checkpoints table')
} else {
  const rows = JSON.parse(
    psql(
      `select coalesce(jsonb_agg(to_jsonb(c) order by c.seq), '[]') from run_checkpoints c where c.run_id = '${values.run}'`,
    ),
  )
  const header = [...LEDGER_COLUMNS, 'checkpoint']
  const table = [header, ...rows.map((r) => [...LEDGER_COLUMNS.map((c) => cell(r[c])), digest(r.checkpoint)])]
  const widths = header.map((_, i) => Math.max(...table.map((row) => row[i].length)))
  out.push('', `ledger: ${rows.length} rows`)
  for (const row of table) out.push(row.map((c, i) => c.padEnd(widths[i])).join('  ').trimEnd())
  const lastTurn = rows.filter((r) => r.kind === 'turn').at(-1)
  if (lastTurn) {
    const agrees = digest(lastTurn.checkpoint) === digest(run.checkpoint)
    out.push('', `runs.checkpoint equals the last turn row: ${agrees ? 'yes' : 'no'}`)
  }
}
process.stdout.write(`${out.join('\n')}\n`)
