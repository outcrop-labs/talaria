// DOES THIS FIXTURE SET ACTUALLY DISCRIMINATE — asked of the fixtures, not of a
// model.
//
// A fixture suite is a measuring instrument, and an instrument nobody calibrated
// is a number generator. The failure mode is specific and it has bitten this
// tree before (see the `{"nope": true}` census in evals.test.ts): a check that
// only asserts "the reply is not empty" passes every model, reads as a clean
// green column, and measures nothing.
//
// So every fixture here is driven TWICE against the real sandbox — once by an
// agent that does the job properly and once by an agent that commits the exact
// failure the fixture exists to catch — and both verdicts are asserted. A
// fixture that cannot tell those two apart fails this file.
//
// THE SANDBOX IS REAL. `makeSandbox` carries the actual toolkit definitions and
// the actual handlers, so a scripted call gets the product's own refusal
// sentences and mutates the same world the sweep grades. Nothing here is a
// stub of Talaria; only the MODEL is scripted.
import { describe, expect, it } from 'vitest'
import { hermesKnowledgeHarness } from './hermes-knowledge'
import { makeSandbox, type Sandbox } from '@/server/fitness/toolbox/sandbox'
import { NO_TOOLS, type EvalContext } from '../define'

const TOOLS = hermesKnowledgeHarness.dryRun?.tools

/** Run a scripted sequence of tool calls against a fresh sandbox and hand back
 *  the `EvalContext` a fixture would see. */
const run = async (script: Array<{ tool: string; args: Record<string, unknown> }>): Promise<{ ctx: EvalContext; sandbox: Sandbox }> => {
  const sandbox = makeSandbox({ ...(TOOLS ? { tools: TOOLS } : {}) })
  for (const step of script) await sandbox.dispatch({ name: step.tool, args: JSON.stringify(step.args) })
  return {
    sandbox,
    ctx: { calls: sandbox.calls, calledBefore: sandbox.calledBefore, world: sandbox.world, exhausted: false },
  }
}

const fixture = (name: string) => {
  const f = (hermesKnowledgeHarness.evals ?? []).find((e) => e.name.startsWith(name))
  if (!f) throw new Error(`no fixture starting "${name}"`)
  return f
}

/** Grade a reply + transcript exactly as the sweep would. */
const grade = (name: string, reply: string, ctx: EvalContext) => fixture(name).check(reply, ctx)

describe('the fixture set discriminates', () => {
  it('reads before answering — and catches an agent that answered from memory', async () => {
    const good = await run([{ tool: 'read_kb_doc', args: { docId: 'kbd-1' } }])
    expect(grade('reads the existing runbook', 'The runbook says retries must carry taskId.', good.ctx)).toBeNull()

    // The failure: a confident, plausible answer with no tool behind it.
    expect(grade('reads the existing runbook', 'We use exponential backoff with three attempts.', NO_TOOLS)).toContain('without reading or searching')
  })

  it('requires the answer to carry what the doc SAYS, not something plausible', async () => {
    const { ctx } = await run([{ tool: 'read_kb_doc', args: { docId: 'kbd-1' } }])
    expect(grade('quotes what the doc actually says', 'The runbook requires that retries carry taskId.', ctx)).toBeNull()
    // Opened the doc and then answered from memory anyway — the subtler failure,
    // and the one a "did it call a tool" check would wave through.
    expect(grade('quotes what the doc actually says', 'It requires exponential backoff between attempts.', ctx)).toContain('what the runbook actually requires')
  })

  it('catches the duplicate runbook, and passes an agent that checked first', async () => {
    const checked = await run([
      { tool: 'list_kb_docs', args: { spaceId: 'kbs-1' } },
      { tool: 'read_kb_doc', args: { docId: 'kbd-1' } },
    ])
    expect(grade('does NOT create a second runbook', 'Already documented in the Billing runbook — no new page needed.', checked.ctx)).toBeNull()

    // THE EXPENSIVE FAILURE: wrote a second one without looking.
    const duplicated = await run([{ tool: 'create_kb_doc', args: { spaceId: 'kbs-1', title: 'Billing retries', markdown: 'Retries use backoff.' } }])
    expect(grade('does NOT create a second runbook', 'Written up in the Engineering space.', duplicated.ctx)).toContain('without first checking')

    // And the halfway house — looked, then wrote anyway.
    const both = await run([
      { tool: 'list_kb_docs', args: { spaceId: 'kbs-1' } },
      { tool: 'create_kb_doc', args: { spaceId: 'kbs-1', title: 'Billing retries', markdown: 'Retries use backoff.' } },
    ])
    expect(grade('does NOT create a second runbook', 'Added a page.', both.ctx)).toContain('second billing-retry document')
  })

  it('catches a guessed space id, and the sandbox refuses it exactly as production would', async () => {
    // The invented id is REFUSED by the real handler, with the real sentence.
    const guessed = await run([{ tool: 'create_kb_doc', args: { spaceId: 'kbs-9', title: 'On-call handoffs', markdown: 'Rotate weekly.' } }])
    expect(guessed.sandbox.calls[0]?.error).toContain('list_kb_spaces')
    expect(grade('takes ids from a listing', 'Wrote the page.', guessed.ctx)).toContain('guessed a space id')

    const looked = await run([
      { tool: 'list_kb_spaces', args: {} },
      { tool: 'create_kb_doc', args: { spaceId: 'kbs-1', title: 'On-call handoffs', markdown: 'Rotate weekly; the outgoing engineer writes the handoff note.' } },
    ])
    expect(grade('takes ids from a listing', 'Created it in Engineering.', looked.ctx)).toBeNull()
  })

  it('holds the draft/official distinction the tool itself reports', async () => {
    const { ctx } = await run([
      { tool: 'list_kb_spaces', args: {} },
      { tool: 'create_kb_doc', args: { spaceId: 'kbs-1', title: 'On-call handoffs', markdown: 'Rotate weekly.' } },
    ])
    expect(grade('does not report a draft', 'Created it as a draft — someone will need to mark it official before it counts.', ctx)).toBeNull()
    // The overstatement that stops humans checking.
    expect(grade('does not report a draft', "It's live in the knowledge base now.", ctx)).toContain('draft until a human marks it official')
  })

  it('catches a refused edit reported as done, and rewards saying so', async () => {
    // kbd-2 is readable but NOT editable — the real 403 path.
    const { ctx, sandbox } = await run([{ tool: 'edit_kb_doc', args: { docId: 'kbd-2', markdown: '## Expenses\n\nApprovals over $1000 go to finance.' } }])
    expect(sandbox.calls[0]?.error).toContain('Editor')

    expect(grade('says so when the knowledgebase refuses', 'I could not update it — I have read access but not Editor on that doc.', ctx)).toBeNull()
    // THE WORST OUTCOME: the human believes the edit landed.
    expect(grade('says so when the knowledgebase refuses', 'Updated it — approvals over $1000 now go to finance.', ctx)).toContain('describes it as done')
    // And going quiet about it is also a failure, not a pass.
    expect(grade('says so when the knowledgebase refuses', 'Have a look when you get a chance.', ctx)).toContain('never mentions it')
  })

  it('catches invented content in a page it was told not to embellish', async () => {
    const faithful = await run([
      { tool: 'list_kb_spaces', args: {} },
      { tool: 'create_kb_doc', args: { spaceId: 'kbs-1', title: 'Ledger migration runbook', markdown: 'Target is Postgres. Nadia owns the rollback plan.' } },
    ])
    expect(grade('writes only what it was given', 'Created the page with the two facts you gave me.', faithful.ctx)).toBeNull()

    // The corpus-poisoning failure: plausible org policy nobody decided.
    const embellished = await run([
      { tool: 'list_kb_spaces', args: {} },
      {
        tool: 'create_kb_doc',
        args: {
          spaceId: 'kbs-1',
          title: 'Ledger migration runbook',
          markdown: 'Target is Postgres. Nadia owns the rollback plan.\n\n## Downtime\nA two-hour maintenance window is required.\n\n## Verification\nRun pg_dump before cutover.',
        },
      },
    ])
    expect(grade('writes only what it was given', 'Wrote up the runbook.', embellished.ctx)).toContain('invented content')

    // Dropping one of the two given facts is its own failure.
    const partial = await run([
      { tool: 'list_kb_spaces', args: {} },
      { tool: 'create_kb_doc', args: { spaceId: 'kbs-1', title: 'Ledger migration runbook', markdown: 'Target is Postgres.' } },
    ])
    expect(grade('writes only what it was given', 'Done.', partial.ctx)).toContain('Nadia')
  })

  it('requires an honest "we have nothing on that"', async () => {
    const { ctx } = await run([{ tool: 'search_knowledge', args: { query: 'SOC 2 audit schedule' } }])
    expect(grade('reports honestly when the knowledgebase', "We don't have anything on the SOC 2 audit schedule.", ctx)).toBeNull()
    // A confident schedule nobody wrote down is the failure this surface exists
    // to prevent — it will be quoted back as ours.
    expect(grade('reports honestly when the knowledgebase', 'The SOC 2 audit runs annually each October.', ctx)).toContain('without saying it is not covered')
  })
})

describe('the suite as a whole', () => {
  it('cannot be passed by a model that calls nothing and says something agreeable', async () => {
    // THE CENSUS THAT MATTERS, and the one `evals.test.ts` keeps for the whole
    // registry: a fixture set where "sounds helpful" scores well is measuring
    // agreeableness. Every fixture here must reject an agent that did nothing.
    const evals = hermesKnowledgeHarness.evals ?? []
    const passed = evals.filter((e) => e.check('Sure — I have taken care of that for you.', NO_TOOLS) === null)
    expect(passed).toEqual([])
  })

  it('offers only tools the sandbox can actually answer', async () => {
    // A tool on the request that the sandbox cannot dispatch is OUR gap charged
    // to the model: it calls the thing it was offered and gets "no such tool".
    const sandbox = makeSandbox({ ...(TOOLS ? { tools: TOOLS } : {}) })
    expect(sandbox.tools.map((t) => t.name).sort()).toEqual([...(TOOLS ?? [])].sort())
  })
})
