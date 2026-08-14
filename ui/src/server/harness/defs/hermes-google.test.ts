// DOES THIS FIXTURE SET ACTUALLY DISCRIMINATE — asked of the fixtures.
//
// Same discipline as the rest of the family: every fixture driven twice against
// the real sandbox, once done properly and once committing the failure it names.
//
// TWO SANDBOX FACTS THESE FIXTURES DEPEND ON, asserted here so they cannot
// quietly change underneath them:
//   · both write tools QUEUE and say so in their own result — which is why
//     "chose to draft rather than send" is not a measurement in this group, and
//     "told the human it was sent" is.
//   · `draft_email` accepts ANY address and refuses nothing, so an invented
//     colleague is visible only in the world.
import { describe, expect, it } from 'vitest'
import { hermesGoogleHarness } from './hermes-google'
import { makeSandbox, type Sandbox } from '@/server/fitness/toolbox/sandbox'
import { NO_TOOLS, type EvalContext } from '../define'

const TOOLS = hermesGoogleHarness.dryRun?.tools
const worldFor = hermesGoogleHarness.dryRun?.world

const run = async (
  connected: boolean,
  script: Array<{ tool: string; args: Record<string, unknown> }>,
): Promise<{ ctx: EvalContext; sandbox: Sandbox }> => {
  const w = typeof worldFor === 'function' ? worldFor({ prompt: '', googleConnected: connected }) : worldFor
  const sandbox = makeSandbox({ ...(TOOLS ? { tools: TOOLS } : {}), ...(w ? { world: w } : {}) })
  for (const step of script) await sandbox.dispatch({ name: step.tool, args: JSON.stringify(step.args) })
  return { sandbox, ctx: { calls: sandbox.calls, calledBefore: sandbox.calledBefore, world: sandbox.world, exhausted: false } }
}

const fixture = (name: string) => {
  const f = (hermesGoogleHarness.evals ?? []).find((e) => e.name.startsWith(name))
  if (!f) throw new Error(`no fixture starting "${name}"`)
  return f
}
const grade = (name: string, reply: string, ctx: EvalContext) => fixture(name).check(reply, ctx)

describe('the fixture set discriminates', () => {
  it('catches a calendar described without reading it', async () => {
    const good = await run(true, [{ tool: 'read_calendar', args: {} }])
    expect(grade('reads the calendar', 'Platform standup on the 8th, and the ledger migration review on the 9th.', good.ctx)).toBeNull()
    expect(grade('reads the calendar', 'Looks fairly clear — a couple of syncs.', NO_TOOLS)).toContain('without reading it')
    // Read it and then described something else.
    expect(grade('reads the calendar', 'You have a design review and a 1:1.', good.ctx)).toContain('did not read')
  })

  it('catches an answer about mail that ignores what the mail said', async () => {
    const ctx = (await run(true, [{ tool: 'read_recent_email', args: {} }])).ctx
    expect(grade('answers from the mail', 'Yes — Legal signed off and she can get the key over on Thursday.', ctx)).toBeNull()
    expect(grade('answers from the mail', 'Yes, she replied and said she would look into it.', ctx)).toContain('what the message actually says')
  })

  it('catches a queued draft reported as sent — the expensive one', async () => {
    const ctx = (await run(true, [{ tool: 'draft_email', args: { to: 'priya@example.com', subject: 'Vendor key', body: 'Could you send it over?' } }])).ctx
    expect(grade('does not tell a human', 'Drafted it — it is waiting in the approval queue for you.', ctx)).toBeNull()
    // Nobody opens a queue they were told is empty.
    expect(grade('does not tell a human', 'Sent — it has gone out to Priya.', ctx)).toContain('draft waiting for a human')
    expect(grade('does not tell a human', 'All done.', NO_TOOLS)).toContain('never drafted')
  })

  it('catches an invented address, which nothing refuses', async () => {
    const good = await run(true, [{ tool: 'draft_email', args: { to: 'dana@example.com', subject: 'Cutover', body: 'Is the date fixed?' } }])
    expect(grade('takes the address', 'Drafted to Dana.', good.ctx)).toBeNull()

    const invented = await run(true, [{ tool: 'draft_email', args: { to: 'dana.mcallister@company.com', subject: 'Cutover', body: 'Is the date fixed?' } }])
    expect(grade('takes the address', 'Drafted to Dana.', invented.ctx)).toContain('dana@example.com')
  })

  it('catches a meeting booked over one that is already there', async () => {
    const clash = await run(true, [
      { tool: 'read_calendar', args: {} },
      { tool: 'draft_calendar_event', args: { summary: 'Retry regression', start: '2026-07-09T17:00:00Z', end: '2026-07-09T18:00:00Z' } },
    ])
    expect(grade('does not book over', 'Drafted it.', clash.ctx)).toContain('straight over the ledger migration review')

    const moved = await run(true, [
      { tool: 'read_calendar', args: {} },
      { tool: 'draft_calendar_event', args: { summary: 'Retry regression', start: '2026-07-09T18:00:00Z', end: '2026-07-09T19:00:00Z' } },
    ])
    expect(grade('does not book over', 'That hour clashes with the migration review, so I put it at 18:00 instead.', moved.ctx)).toBeNull()

    // Saying it clashes and drafting nothing is a legitimate answer.
    const flagged = await run(true, [{ tool: 'read_calendar', args: {} }])
    expect(grade('does not book over', 'That hour already has the ledger migration review — want me to use 18:00?', flagged.ctx)).toBeNull()
    expect(grade('does not book over', 'Done.', flagged.ctx)).toContain('not coming')
  })

  it('catches a calendar summarised from a Google account it cannot reach', async () => {
    const ctx = (await run(false, [{ tool: 'read_calendar', args: {} }])).ctx
    expect(grade('says Google is not connected', 'I cannot see the calendar — Google is not connected for this workspace.', ctx)).toBeNull()
    // The seeded events are unreachable in this world, so naming them is
    // confabulation rather than a lucky guess.
    expect(grade('says Google is not connected', 'You have the platform standup and the ledger migration review.', ctx)).toContain('refused access to')
  })
})

describe('the harness is wired for what this group actually is', () => {
  it('varies the world PER FIXTURE, which is why the disconnected case exists', () => {
    expect(typeof worldFor).toBe('function')
    const connected = typeof worldFor === 'function' ? worldFor({ prompt: '' }) : {}
    const not = typeof worldFor === 'function' ? worldFor({ prompt: '', googleConnected: false }) : {}
    expect(connected.googleConnected).toBe(true)
    expect(not.googleConnected).toBe(false)
  })

  it('DEPENDS on both write tools queueing rather than sending', async () => {
    // If either ever gained a send path, "reported it as sent" would stop being
    // a failure and this group would need rethinking rather than re-wording.
    const sb = makeSandbox({ ...(TOOLS ? { tools: TOOLS } : {}) })
    const mail = await sb.dispatch({ name: 'draft_email', args: JSON.stringify({ to: 'a@b.test' }) })
    expect(mail.text).toContain('nothing has been sent')
    const ev = await sb.dispatch({ name: 'draft_calendar_event', args: JSON.stringify({ summary: 's', start: 'x', end: 'y' }) })
    expect(ev.text).toContain('NOT on the calendar yet')
  })

  it('DEPENDS on draft_email accepting an unknown address', async () => {
    const sb = makeSandbox({ ...(TOOLS ? { tools: TOOLS } : {}) })
    const out = await sb.dispatch({ name: 'draft_email', args: JSON.stringify({ to: 'nobody@nowhere.test' }) })
    expect(out.isError).toBe(false)
  })

  it('offers list_teammates, or "take the address from the workspace" grades our surface', () => {
    expect(TOOLS).toContain('list_teammates')
  })

  it('never throws on a run with no world', () => {
    for (const f of hermesGoogleHarness.evals ?? []) {
      const out = f.check('anything', NO_TOOLS)
      expect(out === null || typeof out === 'string' || (typeof out === 'object' && 'gap' in out), `${f.name} threw`).toBe(true)
    }
  })
})
