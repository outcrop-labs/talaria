// THE WIRING, PROVEN. Every other test here asserts that a run CHECKPOINTS
// correctly; none of them can tell you whether anything ever reads those
// checkpoints back on a real instance, because that depends entirely on which
// modules the server graph loads. This file is that assertion.
//
// It matters more than usual here: the symptom of the wiring being wrong is
// silence — a cold instance finds a row of a kind it never imported, correctly
// declines to touch it, and nothing anywhere says so.
//
// Claims 3 and 4 are read off the SOURCE TREE rather than by importing the
// route graph, deliberately: importing every route in a unit test would drag
// Postgres, Redis, Qdrant and the gateway in behind it, and the property under
// test is a static fact about the import graph rather than a runtime one. The
// search finds the importer wherever it is, so MOVING the side-effect import
// from one route to another is fine and DELETING it is not.
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

// The import under test. Side-effect only — this is the line a production
// instance runs, and everything below asks what it did.
import '@/server/runs/boot'

import { runDefinitions } from '@/server/runs/define'
import { RECLAIM_JOB, RECLAIM_JOB_SPEC } from '@/server/runs/reclaim'
import { schedulerStatus } from '@/server/scheduler'

const HERE = new URL('.', import.meta.url).pathname
const SERVER = join(HERE, '..')
const ROUTES = join(SERVER, '..', 'routes', 'api')

/** Every .ts under a directory, ignoring tests. A plain walk rather than
 *  `readdirSync(recursive)` so this does not depend on a Node minor. */
function sources(dir: string): string[] {
  if (!existsSync(dir)) return []
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const full = join(dir, e.name)
    if (e.isDirectory()) return sources(full)
    return e.name.endsWith('.ts') && !e.name.includes('.test.') ? [full] : []
  })
}

describe('server/runs/boot.ts', () => {
  it('registers the reclaim sweep with the scheduler', () => {
    // Without this the whole substrate is inert: checkpoints get written, and
    // no process ever re-enters the step that follows them.
    expect(schedulerStatus().map((j) => j.name)).toContain(RECLAIM_JOB)
    expect(RECLAIM_JOB_SPEC.name).toBe('run-reclaim')
  })

  it('names EVERY module in defs/, so a new kind cannot be silently omitted', () => {
    // The failure this catches is the one nobody notices: a definition lands,
    // its module is imported only by whatever surface starts it, and a cold
    // instance that finds one of its rows leaves it alone forever
    // (`drive()` returns `no-definition` and does the right thing, quietly).
    //
    // Both sides are read fresh, so a kind that lands in defs/ without its line
    // here fails this test rather than going quietly unregistered.
    const modules = sources(join(HERE, 'defs'))
      .map((f) => f.slice(f.lastIndexOf('/') + 1).replace(/\.ts$/, ''))
      .sort()
    const boot = readFileSync(join(HERE, 'boot.ts'), 'utf8')
    const imported = [...boot.matchAll(/^import '\.\/defs\/([\w-]+)'$/gm)].map((m) => m[1]).sort()
    expect(imported).toEqual(modules)
  })

  it('is reachable from the route graph — the only way it reaches production', () => {
    const importers = sources(ROUTES).filter((f) => readFileSync(f, 'utf8').includes("'@/server/runs/boot'"))
    expect(importers.length, 'no route imports @/server/runs/boot — run-reclaim will never arm').toBeGreaterThan(0)
  })

  it('is in REQUIRED_JOBS, so a boot that loses the import is a loud error', () => {
    // REQUIRED_JOBS is module-private (it is the scheduler's own boot check), so
    // this reads the declaration. Worth pinning in source form precisely because
    // the failure mode it guards — durability quietly not happening — has no
    // other symptom.
    const scheduler = readFileSync(join(SERVER, 'scheduler.ts'), 'utf8')
    const start = scheduler.indexOf('const REQUIRED_JOBS')
    const required = scheduler.slice(start, scheduler.indexOf('\n]', start))
    expect(required).toContain("'run-reclaim'")
  })

  it('every registered kind states an audience and a step budget', () => {
    // A definition missing either is one the runtime cannot park or cannot time
    // out, and both are silent until the day they matter.
    for (const def of runDefinitions()) {
      expect(typeof def.audience, `${def.kind} has no audience`).toBe('function')
      expect(def.maxStepMs, `${def.kind} has no maxStepMs`).toBeGreaterThan(0)
    }
  })

  it('registers the kinds anyone has meant to add, and no others', () => {
    // Named explicitly rather than counted, so adding a kind without meaning
    // to shows up here as a failing test rather than as a behaviour change
    // nobody reviewed.
    expect(runDefinitions().map((d) => d.kind).sort()).toEqual(['plan-draft', 'rag-backfill', 'rag-reindex', 'research', 'work-session'])
  })
})
