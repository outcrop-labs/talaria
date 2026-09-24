#!/usr/bin/env node
// stop-check — the stop gate, as a library any harness can call.
//
// WHY. `bun run check` is the repo's seconds-fast, dependency-free gate —
// invariants, doc links, generated-reference drift — and the one excuse for
// skipping it has always been "I forgot". A stop gate removes the excuse:
// the same chain CI runs, run at the moment an agent claims to be done.
// The same moment runs the artifact sweep (scripts/cleanup-sweep.mjs --gate).
// Disk pressure and stale dev artifacts are not a CI invariant — a fresh
// runner has no one's worktrees — so they stay out of `bun run check` and
// live here, where the agent who left them is about to claim done.
//
// THE CONTRACT (see README.md in this directory — the load-bearing part; any
// harness that can run a command and read an exit code can wire this):
//   stdin  — the caller's event payload, whatever shape it carries. Tolerated,
//            never read: harnesses disagree on the shape; the gate does not care.
//   exit 0 — pass, silent. Nothing to say.
//   exit 2 — block: the reason is on stderr. Claude Code feeds stderr back to
//            the model; a git hook shows it to the pusher; CI fails the job.
//   else   — the gate could not run (bun missing, timeout). A NON-BLOCKING
//            error: never treat it as a pass, never treat it as a block.
//
// WHY IT ALWAYS RUNS. No diff fast path, no surface matching: the check is
// seconds and zero-dependency, and a skip list is a second place where each
// checker's scope rots. The gate reads the whole tree on purpose — parallel
// sessions share working trees, so a failure may predate your change.

import { spawnSync } from 'node:child_process'
import { resolve } from 'node:path'

const ROOT = resolve(import.meta.dirname, '..', '..')

const run = spawnSync('bun', ['run', 'check'], {
  cwd: ROOT,
  encoding: 'utf8',
  // Below the harnesses' own hook timeouts, so a wedged check exits here as a
  // non-blocking error instead of being killed from the outside.
  timeout: 50_000,
})

if (run.error) {
  console.error(`stop gate could not run — not a pass: ${run.error.message}`)
  process.exit(1)
}

if (run.status === null) {
  console.error('stop gate could not run — not a pass: bun run check timed out')
  process.exit(1)
}

if (run.status !== 0) {

console.error('stop gate FAILED — bun run check is red. Fix it before claiming done.')
if (run.stdout) process.stderr.write(run.stdout)
if (run.stderr) process.stderr.write(run.stderr)
console.error(
  'The gate runs on the whole tree: if this failure predates your change (parallel ' +
    'sessions share working trees), verify that and say so in the PR.'
)
process.exit(2)
}

const sweep = spawnSync(process.execPath, [resolve(ROOT, 'scripts', 'cleanup-sweep.mjs'), '--gate'], {
  cwd: ROOT,
  encoding: 'utf8',
  timeout: 20_000,
})

if (sweep.error || sweep.status === null) {
  console.error(
    `stop gate could not run the artifact sweep — not a pass: ${sweep.error?.message ?? 'timed out'}`,
  )
  process.exit(1)
}

if (sweep.status === 2) {
  console.error('stop gate FAILED — dev artifacts or disk pressure. Clean up before claiming done.')
  if (sweep.stdout) process.stderr.write(sweep.stdout)
  if (sweep.stderr) process.stderr.write(sweep.stderr)
  console.error('The command is `bun talaria cleanup --apply`. The convention is the cleanup skill.')
  process.exit(2)
}

if (sweep.status !== 0) {
  console.error('stop gate could not run the artifact sweep — not a pass.')
  if (sweep.stderr) process.stderr.write(sweep.stderr)
  process.exit(1)
}

process.exit(0)
