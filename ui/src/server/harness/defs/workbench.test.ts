import { describe, expect, it } from 'vitest'
import { NO_TOOLS, type EvalContext } from '@/server/harness/define'
import { workbenchHeavyHarness, workbenchLightHarness, workbenchStandardHarness } from '@/server/harness/defs/workbench'
import { makeWorkbench, type WorkspaceFile } from '@/server/fitness/toolbox/hermes-tools'

// The three Workbench columns used to read "No harness in this install is bound
// to this, so a sweep can say nothing about a model for it." These are the
// harnesses that fill them, and what is asserted here is the half a sweep
// cannot: that the ORACLES are right, and that the fixtures fail the ways they
// are meant to fail. An oracle that accepts a broken repository would credit
// every model with a fix; one that rejects a real fix would fail every model.

const HARNESSES = [workbenchLightHarness, workbenchStandardHarness, workbenchHeavyHarness]

const call = (tool: string, args: Record<string, unknown> = {}) => ({ tool, args, result: {}, error: null })

/** A context standing in for a completed dry run. `failure` is what the
 *  fixture's oracle said about the files as the model left them. */
const ctx = (calls: EvalContext['calls'], failure: string | null = null): EvalContext => ({
  ...NO_TOOLS,
  calls,
  world: { failure },
  calledBefore: (a, b) => {
    const i = calls.findIndex((c) => c.tool === a)
    const j = calls.findIndex((c) => c.tool === b)
    return i !== -1 && j !== -1 && i < j
  },
})

const fixtureOf = (name: string) => {
  const found = (workbenchStandardHarness.evals ?? []).find((e) => e.name === name)
  if (!found) throw new Error(`no workbench fixture called "${name}"`)
  return found
}

/** Run a fixture's own workspace through its oracle with one file replaced —
 *  which is how a real fix reaches it. */
const oracleFor = (fixtureName: string) => {
  const fixture = fixtureOf(fixtureName)
  const workspace = workbenchStandardHarness.dryRun?.workspace?.(fixture.input)
  if (!workspace) throw new Error('the workbench harness declares no workspace')
  return (patch: Partial<Record<string, string>> = {}): string | null =>
    workspace.passes(workspace.files.map((f): WorkspaceFile => ({ ...f, content: patch[f.path] ?? f.content })))
}

describe('the three Workbench harnesses', () => {
  it('bind the three effort roles, which is what fills the columns', () => {
    // `score.ts` runs the real resolver over `model`, so naming the role here is
    // the whole mechanism: one harness declaring `code-standard` would bind one
    // column and leave the other two exactly as empty as they were.
    expect(HARNESSES.map((h) => h.model.role)).toEqual(['code-light', 'code-standard', 'code-heavy'])
    expect(HARNESSES.map((h) => h.id)).toEqual(['workbench:light', 'workbench:standard', 'workbench:heavy'])
  })

  it('declare the tool loop, and a workspace rather than a Talaria world', () => {
    for (const h of HARNESSES) {
      expect(h.tools, h.id).toBe('own')
      expect(h.dryRun?.workspace, `${h.id} has no workspace, so a dry run would hand it Talaria's toolkit`).toBeTypeOf('function')
      expect(h.dryRun?.tools, `${h.id} declares both surfaces`).toBeUndefined()
    }
  })

  it('run the SAME tasks at every effort, so the three columns are comparable', () => {
    const names = HARNESSES.map((h) => (h.evals ?? []).map((e) => e.name).join('|'))
    expect(new Set(names).size, 'the three efforts run different suites, so their scores cannot be compared').toBe(1)
  })

  it('refuse nothing — a weaker coder is reviewed, not blocked', () => {
    for (const h of HARNESSES) expect(h.floor.refuseBelow, h.id).toBe(false)
  })
})

describe('the task oracles', () => {
  it('paginate: rejects the bug, accepts any one-based fix', () => {
    const oracle = oracleFor('fixes an off-by-one the test spells out — the suite goes green')
    expect(oracle()).toContain('one-based')
    // Two spellings of the same fix, because a benchmark that demands one exact
    // diff measures obedience rather than capability.
    const a = 'export function pageRange(page, perPage, total) {\n  const start = (page - 1) * perPage\n  const end = Math.min(start + perPage, total)\n  return { start, end }\n}\n'
    const b = 'export function pageRange(page, perPage, total) {\n  const start = perPage * (page - 1)\n  const end = Math.min(start + perPage, total)\n  return { start, end }\n}\n'
    expect(oracle({ 'src/paginate.js': a })).toBeNull()
    expect(oracle({ 'src/paginate.js': b })).toBeNull()
    // And it still checks the behaviour that already worked.
    const noClamp = 'export function pageRange(page, perPage, total) {\n  const start = (page - 1) * perPage\n  return { start, end: start + perPage }\n}\n'
    expect(oracle({ 'src/paginate.js': noClamp })).toContain('clamped')
  })

  it('invoice: accepts the fix at the cause and REJECTS it at the symptom', () => {
    // The whole point of this task. Patching the caller leaves every other
    // consumer of money() still truncating, so the defect is still there.
    const oracle = oracleFor('fixes the defect, not the file it was pointed at — the suite goes green')
    expect(oracle()).toContain('truncates')

    const fixedMoney = '// Every monetary value in the system passes through here.\nexport function money(n) {\n  return Math.round(n * 100) / 100\n}\n'
    expect(oracle({ 'src/money.js': fixedMoney })).toBeNull()

    const patchedCaller =
      "import { money } from './money.js'\n\nexport function invoiceTotal(lines) {\n  let sum = 0\n  for (const line of lines) sum += Math.round(line.unitPrice * line.quantity * 100) / 100\n  return sum\n}\n"
    expect(oracle({ 'src/invoice.js': patchedCaller })).toContain('truncates')
    // Even WITH money() fixed, rounding in the caller as well is flagged: it is
    // the shape of a symptom patch and the next reader has to disprove it.
    expect(oracle({ 'src/money.js': fixedMoney, 'src/invoice.js': patchedCaller })).toContain('every other caller')
  })

  it('slug: accepts an accent fold and rejects one that breaks a passing test', () => {
    const oracle = oracleFor('fixes the reported bug without breaking what already worked — the suite goes green')
    expect(oracle()).toContain('accents')

    const good =
      "export function slug(title) {\n  return title\n    .normalize('NFKD')\n    .replace(/[\\u0300-\\u036f]/g, '')\n    .toLowerCase()\n    .replace(/[^a-z0-9]+/g, '-')\n    .replace(/^-|-$/g, '')\n}\n"
    expect(oracle({ 'src/slug.js': good })).toBeNull()

    // Folds the accents and drops the trim — the regression this task exists for.
    const lostTrim =
      "export function slug(title) {\n  return title\n    .normalize('NFKD')\n    .replace(/[\\u0300-\\u036f]/g, '')\n    .toLowerCase()\n    .replace(/[^a-z0-9]+/g, '-')\n}\n"
    expect(oracle({ 'src/slug.js': lostTrim })).toContain('trim')
  })

  it('an empty file is a failure, not a pass', () => {
    // `write_file` replaces a whole file; a model that writes nothing must not
    // score a green suite for having deleted the defect along with the code.
    for (const [name, path] of [
      ['fixes an off-by-one the test spells out — the suite goes green', 'src/paginate.js'],
      ['fixes the defect, not the file it was pointed at — the suite goes green', 'src/money.js'],
      ['fixes the reported bug without breaking what already worked — the suite goes green', 'src/slug.js'],
    ] as const) {
      expect(oracleFor(name)({ [path]: '' }), name).toContain('empty or missing')
    }
  })
})

describe('the workbench fixtures', () => {
  const green = fixtureOf('fixes an off-by-one the test spells out — the suite goes green').check
  const careful = fixtureOf('fixes an off-by-one the test spells out — reads a file before replacing it').check
  const verified = fixtureOf('fixes an off-by-one the test spells out — runs the tests before it calls it done').check

  const CLEAN = [call('list_files'), call('read_file', { path: 'src/paginate.js' }), call('write_file', { path: 'src/paginate.js' }), call('run_tests')]

  it('all pass a run that read, fixed and verified', () => {
    for (const check of [green, careful, verified]) expect(check('fixed the off-by-one', ctx(CLEAN))).toBeNull()
  })

  it('all fail a model that answered in prose — which is the whole point', () => {
    // The census in `fitness/evals.test.ts` enforces this too; asserting it here
    // as well is cheap and names the reason.
    for (const check of [green, careful, verified]) expect(check('The bug is that pages are one-based.', ctx([]))).toContain('no tool at all')
  })

  it('catch the three failures they exist for, independently', () => {
    // FIXED but not carefully: replaced a file it never read.
    const blind = [call('list_files'), call('write_file', { path: 'src/paginate.js' }), call('run_tests')]
    expect(green('done', ctx(blind))).toBeNull()
    expect(careful('done', ctx(blind))).toContain('without reading it first')

    // CAREFUL but not fixed: the oracle still reports a failure.
    expect(green('done', ctx(CLEAN, 'pageRange(1, 20, 100) returned { start: 20 }'))).toContain('still red')
    expect(careful('done', ctx(CLEAN, 'pageRange(1, 20, 100) returned { start: 20 }'))).toBeNull()

    // FIXED and careful but never verified.
    const unverified = [call('read_file', { path: 'src/paginate.js' }), call('write_file', { path: 'src/paginate.js' })]
    expect(green('done', ctx(unverified))).toBeNull()
    expect(verified('done', ctx(unverified))).toContain('never ran the tests')
  })

  it('catches a run that verified and then edited again', () => {
    // A green tick on a state the model then changed is the same failure as not
    // having checked at all.
    const stale = [call('read_file', { path: 'src/paginate.js' }), call('write_file', { path: 'src/paginate.js' }), call('run_tests'), call('write_file', { path: 'src/paginate.js' })]
    expect(verified('done', ctx(stale))).toContain('never verified')
  })

  it('counts a search hit as having seen the file', () => {
    // Locating a one-line fix with `search` and editing from the hit is how a
    // developer actually works; demanding a full read would fail correct runs.
    const searched = [call('search', { query: 'pageRange' }), call('write_file', { path: 'src/paginate.js' }), call('run_tests')]
    expect(careful('done', ctx(searched))).toBeNull()
  })
})

describe('the workspace sandbox', () => {
  const workspace = { files: [{ path: 'a.js', content: 'let x = 1\n' }], passes: (f: readonly WorkspaceFile[]) => (f[0]?.content.includes('2') ? null : 'x is still 1') }

  it('reads, writes and re-runs the oracle against what it wrote', async () => {
    const wb = makeWorkbench(workspace)
    expect(wb.green()).toBe(false)
    expect(JSON.parse((await wb.dispatch({ name: 'run_tests', args: '{}' })).text)).toMatchObject({ passed: false, failure: 'x is still 1' })
    await wb.dispatch({ name: 'write_file', args: JSON.stringify({ path: 'a.js', content: 'let x = 2\n' }) })
    expect(JSON.parse((await wb.dispatch({ name: 'run_tests', args: '{}' })).text)).toMatchObject({ passed: true })
    expect(wb.green()).toBe(true)
    expect(wb.world.failure).toBeNull()
  })

  it('names the tree when a path is wrong, so a bad guess costs one turn', async () => {
    const wb = makeWorkbench(workspace)
    const out = await wb.dispatch({ name: 'read_file', args: JSON.stringify({ path: 'nope.js' }) })
    expect(out.isError).toBe(true)
    expect(out.text).toContain('a.js')
  })

  it('refuses a write with no content rather than emptying the file', async () => {
    const wb = makeWorkbench(workspace)
    const out = await wb.dispatch({ name: 'write_file', args: JSON.stringify({ path: 'a.js' }) })
    expect(out.isError).toBe(true)
    expect(wb.files[0]?.content).toBe('let x = 1\n')
  })

  it('is isolated — two workbenches cannot see each other', async () => {
    const a = makeWorkbench(workspace)
    const b = makeWorkbench(workspace)
    await a.dispatch({ name: 'write_file', args: JSON.stringify({ path: 'a.js', content: 'let x = 2\n' }) })
    expect(a.green()).toBe(true)
    expect(b.green()).toBe(false)
    expect(workspace.files[0]?.content).toBe('let x = 1\n')
  })
})
