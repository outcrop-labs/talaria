#!/usr/bin/env node
// run-self-tests — every stdlib self-test of the repo's scripts, found rather
// than listed. Run by `bun run check`.
//
// WHY DISCOVERY. The check chain used to name each `*.test.mjs` by hand, so a
// new test file gated nothing until someone remembered to add it to
// package.json. Now a file named `scripts/<x>.test.mjs` or
// `scripts/hooks/<x>.test.mjs` runs on the next check with no other edit.
//
// THE CONTRACT for a test file here: stdlib only (`bun run check` needs no
// install), no network, exit 0 on a pass and non-zero on a failure, saying
// what failed. No test ever sees a model key: each runs with
// JUDGE_OPENROUTER_API_KEY, JUDGE_CALIBRATION_OPENROUTER_API_KEY and
// OPENROUTER_API_KEY stripped and JUDGE_REVIEW_OFFLINE=1, so the reviewer
// (scripts/judge-review.mjs) refuses the network even if a test forgot to. Each runs with this process's own runtime (bun under `bun run
// check`), from the repo root, with inherited output, one after another, in
// sorted order. Not recursive: fixtures directories hold no tests.

import { spawnSync } from 'node:child_process'
import { readdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const DIRS = ['scripts', 'scripts/hooks']

const files = DIRS.flatMap((dir) =>
  readdirSync(join(ROOT, dir), { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith('.test.mjs'))
    .map((entry) => `${dir}/${entry.name}`),
).sort()

if (files.length === 0) {
  console.error('self-tests: none found under scripts/ — not a pass')
  process.exit(1)
}

/** This process's environment without a model key, and offline. */
export function childEnv(env = process.env) {
  const out = { ...env }
  for (const k of ['JUDGE_OPENROUTER_API_KEY', 'JUDGE_CALIBRATION_OPENROUTER_API_KEY', 'OPENROUTER_API_KEY']) delete out[k]
  return { ...out, JUDGE_REVIEW_OFFLINE: '1' }
}

const failed = []
for (const file of files) {
  const r = spawnSync(process.execPath, [join(ROOT, file)], { cwd: ROOT, stdio: 'inherit', env: childEnv() })
  if (r.error || r.status !== 0) {
    failed.push(r.error ? `${file} (${r.error.message})` : `${file} (exit ${r.status ?? r.signal})`)
  }
}

if (failed.length) {
  console.error(`self-tests: ${failed.length} of ${files.length} failed:\n${failed.map((f) => `  ${f}`).join('\n')}`)
  process.exit(1)
}
console.log(`self-tests: ${files.length} ok`)
