#!/usr/bin/env node
// Prints one run's `runs` row, its `run_checkpoints` ledger, the attempts of
// its lineage and the rewinds that named it, as stable tables, for
// before-and-after evidence: two prints of a finished run are identical, and
// a diff of two prints is what changed.
//
//   node scripts/rewind-proof.mjs --stack <checkout> --run <run id>
//
// Reads only. A database older than the ledger prints the run and says so.

import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { parseArgs } from 'node:util'

const RUN_FIELDS = [
  'id', 'kind', 'state', 'phase', 'attempt', 'crashes', 'checkpoint_seq', 'subject_id', 'error', 'stage', 'turn',
  'lineage_id', 'attempt_no', 'rewound_from_run_id', 'rewound_from_seq', 'agent_run_id',
]
const ATTEMPT_COLUMNS = ['attempt_no', 'id', 'state', 'phase', 'turn', 'rewound_from_run_id', 'rewound_from_seq', 'job_id', 'branch', 'job_status']
const REWIND_COLUMNS = ['id', 'state', 'stage', 'seq', 'attempt_run_id', 'attempt_no', 'error', 'started_at', 'finished_at']
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

const envPath = join(resolve(values.stack), 'ui/.env')
if (!existsSync(envPath)) die(`${values.stack} has no ui/.env: cut it with \`bun talaria worktree <name>\``)
const env = readFileSync(envPath, 'utf8')
const name = env.match(/^TALARIA_WORKTREE=(.+)$/m)?.[1]
if (!name) die(`${values.stack} is not a worktree stack`)

const psql = (sql) =>
  execFileSync('docker', ['exec', `talaria-pg-${name}`, 'psql', '-U', 'talaria', '-d', 'talaria', '-tAc', sql], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  }).trim()

const cell = (value) => (value == null ? '-' : String(value).replaceAll(/\s+/g, ' ').slice(0, 60))

const table = (out, title, columns, rows) => {
  const grid = [columns, ...rows.map((r) => columns.map((c) => cell(r[c])))]
  const widths = columns.map((_, i) => Math.max(...grid.map((row) => row[i].length)))
  out.push('', `${title}: ${rows.length} rows`)
  for (const row of grid) out.push(row.map((c, i) => c.padEnd(widths[i])).join('  ').trimEnd())
}

const runJson = psql(
  `select (to_jsonb(r) - 'checkpoint') || jsonb_build_object('stage', r.checkpoint->'stage', 'turn', r.checkpoint->'turn')
   from runs r where id = '${values.run}'`,
)
if (!runJson) die(`no run ${values.run} on stack ${name}`)
const run = JSON.parse(runJson)

const out = ['run']
for (const field of RUN_FIELDS) out.push(`  ${field.padEnd(15)}${cell(run[field])}`)

if (psql(`select to_regclass('public.run_checkpoints') is null`) === 't') {
  out.push('', 'ledger: absent, this database has no run_checkpoints table')
} else {
  const rows = JSON.parse(
    psql(
      `select coalesce(jsonb_agg(to_jsonb(c) order by c.seq), '[]') from run_checkpoints c where c.run_id = '${values.run}'`,
    ),
  )
  table(out, 'ledger', LEDGER_COLUMNS, rows)
  const lastTurn = rows.filter((r) => r.kind === 'turn').at(-1)
  if (lastTurn) {
    const agrees = lastTurn.status === run.stage && lastTurn.turn === run.turn
    out.push('', `runs.checkpoint agrees with the last turn row on stage and turn: ${agrees ? 'yes' : 'no'}`)
  }
  if (psql(`select count(*) from information_schema.columns where table_name = 'runs' and column_name = 'lineage_id'`) === '0') {
    out.push('', 'attempts: absent, this database has no lineage columns')
  } else {
    const attempts = JSON.parse(
      psql(
        `select coalesce(jsonb_agg(a order by a.attempt_no, a.id), '[]') from (
           select r.attempt_no, r.id, r.state, r.phase, r.checkpoint->'turn' as turn,
                  r.rewound_from_run_id, r.rewound_from_seq,
                  j.id as job_id, j.branch, j.status as job_status
           from runs r
           left join lateral (select id, branch, status from workbench_jobs where run_id = r.id order by created_at desc limit 1) j on true
           where coalesce(r.lineage_id, r.id) = (select coalesce(lineage_id, id) from runs where id = '${values.run}')
         ) a`,
      ),
    )
    table(out, 'attempts in the lineage', ATTEMPT_COLUMNS, attempts)
    const rewinds = JSON.parse(
      psql(
        `select coalesce(jsonb_agg(w order by w.created_at), '[]') from (
           select id, state, checkpoint->>'stage' as stage, (input->>'seq')::bigint as seq,
                  coalesce(result->>'attemptRunId', checkpoint->'plan'->>'attemptRunId') as attempt_run_id,
                  coalesce(result->'attemptNo', checkpoint->'plan'->'attemptNo') as attempt_no,
                  error, started_at, finished_at, created_at
           from runs where kind = 'run-rewind'
             and (input->>'sourceRunId' = '${values.run}'
                  or result->>'attemptRunId' = '${values.run}'
                  or checkpoint->'plan'->>'attemptRunId' = '${values.run}')
         ) w`,
      ),
    )
    table(out, 'rewinds naming this run', REWIND_COLUMNS, rewinds)
  }
}
process.stdout.write(`${out.join('\n')}\n`)
