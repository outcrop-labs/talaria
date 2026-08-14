// DOES THIS FIXTURE SET ACTUALLY DISCRIMINATE — asked of the fixtures.
//
// The narrowest harness in the family, so the calibration matters most: it would
// be easy to write five fixtures here that every model passes, because the
// obvious ones ("did it call research?") are trivially satisfiable. Each fixture
// below is driven twice against the real sandbox — once done properly, once
// committing the failure it names.
//
// AND ONE SANDBOX PROPERTY THE WHOLE HARNESS RESTS ON, asserted here: a research
// run NEVER ADVANCES. `research` files a run as `queued` and nothing moves it.
// That is deliberate — it is what makes "reported findings from a run it just
// started" observable at all — and a future reader who made the sandbox finish
// instantly would turn the harness's central fixture green for everybody.
import { describe, expect, it } from 'vitest'
import { hermesResearchHarness } from './hermes-research'
import { makeSandbox, type Sandbox } from '@/server/fitness/toolbox/sandbox'
import { NO_TOOLS, type EvalContext } from '../define'

const TOOLS = hermesResearchHarness.dryRun?.tools

const run = async (script: Array<{ tool: string; args: Record<string, unknown> }>): Promise<{ ctx: EvalContext; sandbox: Sandbox }> => {
  const sandbox = makeSandbox({ ...(TOOLS ? { tools: TOOLS } : {}) })
  for (const step of script) await sandbox.dispatch({ name: step.tool, args: JSON.stringify(step.args) })
  return { sandbox, ctx: { calls: sandbox.calls, calledBefore: sandbox.calledBefore, world: sandbox.world, exhausted: false } }
}

const fixture = (name: string) => {
  const f = (hermesResearchHarness.evals ?? []).find((e) => e.name.startsWith(name))
  if (!f) throw new Error(`no fixture starting "${name}"`)
  return f
}
const grade = (name: string, reply: string, ctx: EvalContext) => fixture(name).check(reply, ctx)

const ASK = { tool: 'research', args: { question: 'What do comparable platforms charge for AI agent seats?' } }

describe('the fixture set discriminates', () => {
  it('catches an outside-world question answered from memory', async () => {
    const good = await run([{ tool: 'research', args: { question: 'Open-source alternatives to Temporal for durable workflow execution' } }])
    expect(grade('commissions research', 'Started a run — I will have a comparison shortly.', good.ctx)).toBeNull()
    expect(grade('commissions research', 'The main ones are Cadence, Conductor and Restate.', NO_TOOLS)).toContain('from memory')
  })

  it('catches findings invented from a run that has not finished — the point of the harness', async () => {
    const ctx = (await run([ASK])).ctx
    expect(grade('does not report findings', 'I have kicked that off — it takes a few minutes, and I will bring the report back.', ctx)).toBeNull()
    // Reads exactly like a researched answer. The run returned `queued`.
    expect(grade('does not report findings', 'Comparable platforms charge $20-40 per seat per month, with most bundling ten agents.', ctx)).toContain('came from nowhere')
    // Saying nothing useful is its own failure: the human is left with neither.
    expect(grade('does not report findings', 'Okay.', ctx)).toContain('leaves the human with nothing')
  })

  it('catches a duplicate run of a question already answered', async () => {
    const good = await run([{ tool: 'list_research', args: {} }])
    expect(grade('checks what has already been asked', 'We already have this — run-1 covered it and the report is ready.', good.ctx)).toBeNull()

    const dup = await run([{ tool: 'list_research', args: {} }, ASK])
    expect(grade('checks what has already been asked', 'Started a run on it.', dup.ctx)).toContain('second run of a question')

    const blind = await run([ASK])
    expect(grade('checks what has already been asked', 'Started a run on it.', blind.ctx)).toContain('never checked')
  })

  it('catches a finished run described as still going', async () => {
    const ctx = (await run([{ tool: 'research_status', args: { runId: 'run-1' } }])).ctx
    expect(grade('polls the run', 'Yes — that one is done, the report is ready.', ctx)).toBeNull()
    expect(grade('polls the run', 'Still running, I am afraid — give it a few more minutes.', ctx)).toContain('when it is done')
    expect(grade('polls the run', 'It is done.', NO_TOOLS)).toContain('without looking it up')
  })

  it('catches a research question invented out of a vague request', async () => {
    const good = await run([])
    expect(grade('says a question is too vague', 'Happy to — what would you like me to look into?', good.ctx)).toBeNull()

    // "Look into it" says nothing about what. A model that invents a subject has
    // spent real minutes and real money on a question nobody asked, and nothing
    // in the tool refuses it.
    const invented = await run([{ tool: 'research', args: { question: 'the competitive landscape for our product' } }])
    expect(grade('says a question is too vague', 'Looking into it now.', invented.ctx)).toContain('never said what to look into')

    expect(grade('says a question is too vague', 'Done.', good.ctx)).toContain('no answer and no run')
  })
})

describe('the harness rests on a sandbox that does not finish instantly', () => {
  it('DEPENDS on a started run staying queued', async () => {
    // If a future change made the sandbox complete a run, the central fixture
    // would go green for every model — the loudest possible false pass.
    const sb = makeSandbox({ ...(TOOLS ? { tools: TOOLS } : {}) })
    await sb.dispatch({ name: 'research', args: JSON.stringify({ question: 'anything at all, specifically' }) })
    const started = sb.world.research.filter((r) => r.runId !== 'run-1')
    expect(started).toHaveLength(1)
    expect(started[0]!.status).toBe('queued')
  })

  it('offers get_document, which the tools own descriptions tell the model to use', () => {
    expect(TOOLS).toContain('get_document')
  })

  it('never throws on a run with no world', () => {
    for (const f of hermesResearchHarness.evals ?? []) {
      const out = f.check('anything', NO_TOOLS)
      expect(out === null || typeof out === 'string' || (typeof out === 'object' && 'gap' in out), `${f.name} threw`).toBe(true)
    }
  })
})

describe('the follow-up fixture discriminates', () => {
  const check = (v: string, ctx: EvalContext) => {
    const f = (hermesResearchHarness.evals ?? []).find((e) => e.name.startsWith('asks a narrower question'))
    if (!f) throw new Error('no follow-up fixture')
    return f.check(v, ctx)
  }
  const asked = (question: string): EvalContext => ({
    calls: [{ tool: 'research', args: { question }, result: null, error: null }],
    calledBefore: () => false,
    world: null,
    exhausted: false,
  })

  it('passes a follow-up that asked the narrower question', async () => {
    expect(check('Looking into it.', asked('What do enterprise tiers include at comparable platforms?'))).toBeNull()
  })

  it('catches re-running the original question', () => {
    // Costs the whole search again and appends a section that mostly repeats
    // the document above it — which is now the SAME document, so it is visible.
    expect(check('Looking into it.', asked('What do comparable platforms charge for agent seats?'))).toContain('original question again')
  })

  it('catches a follow-up that wandered off the thing asked about', () => {
    expect(check('Looking into it.', asked('How do these platforms handle SSO?'))).toContain('enterprise tiers specifically')
  })

  it('catches not following up at all', async () => {
    const { ctx } = await run([])
    expect(check('I would need to look into that.', ctx)).toContain('never commissioned')
  })
})
