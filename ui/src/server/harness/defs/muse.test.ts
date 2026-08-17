import { describe, expect, it } from 'vitest'
import { NO_TOOLS, type CheckResult } from '@/server/harness/define'
import { runHarness, type HarnessDeps, type TransportRequest } from '@/server/harness/run'
import { redactSecrets } from '@/server/guardrails'
import {
  MUSE_AGENT,
  MUSE_CRON,
  MUSE_TICKET,
  createStreamRedactor,
  looksLikeSchedule,
  museAgentHarness,
  museCronHarness,
  museDraftHarness,
  museTicketHarness,
  type AgentDraft,
  type MuseDraftInput,
  type MuseProseInput,
  type TicketMusePatch,
} from '@/server/harness/defs/muse'
import type { Capability } from '@/server/harness/capability'
import type { GuardConfig } from '@/server/guardrails'

// The Muse's three structured kinds. What these cases hold still is the thing
// the port was for: until now the object came back as a TEXT STREAM and the
// BROWSER pulled it apart with a greedy `/\{[\s\S]*\}/`, so a model that said
// anything at all around its answer produced `null`, which rendered as a button
// that did nothing when you clicked it. There was no repair turn, no guard pass
// and no record that it had happened.
//
// Nothing here touches a database, a gateway or a fleet: every edge the runner
// has is injected. The `agent` kind is exercised through its schema rather than
// through `runHarness` because its render reads the org profile — see the note
// on that case.

const GUARD: GuardConfig = { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }

interface World {
  replies?: string[]
  facts?: Partial<Record<Capability, boolean>>
}

function world(w: World = {}) {
  const requests: TransportRequest[] = []
  const replies = w.replies ?? ['{"name":"inbox-brief","schedule":"0 8 * * 1-5","prompt":"Summarize the inbox into a short brief and send it."}']
  const facts: Partial<Record<Capability, { value: boolean }>> = {}
  for (const [cap, value] of Object.entries(w.facts ?? {})) facts[cap as Capability] = { value }

  const deps: Partial<HarnessDeps> = {
    resolveModel: async () => ({ model: 'pl-main', step: 'preferred' }),
    routing: async (model) => ({ endpoints: ['spark'], upstreamModel: model }),
    missingCapabilities: async (_key, required) => required.filter((c) => facts[c]?.value === false),
    capabilities: async () => facts,
    transport: async (req) => {
      requests.push(req)
      return { kind: 'gateway', text: replies[Math.min(requests.length - 1, replies.length - 1)] ?? '', toolNames: [], usage: null, contractDropped: false }
    },
    guardConfig: async () => GUARD,
    guardText: async () => [],
    recordFindings: async () => {},
    recordRun: async () => {},
    now: () => 0,
  }
  return { requests, deps }
}

const run = <I, O>(def: Parameters<typeof runHarness<I, O>>[0], input: I, w: ReturnType<typeof world>) =>
  runHarness(def, input, { caller: 'platform:muse:test', userId: 'u1', deps: w.deps })

// ── cron ─────────────────────────────────────────────────────────────────────

const ASK: MuseDraftInput = { instruction: 'every weekday at 8am, summarize my inbox into a brief' }

describe('the cron harness', () => {
  it('reads the draft out of the reply shape the client-side extractor died on', async () => {
    // Preamble, fenced object, then a sentence containing a brace. The greedy
    // span ran from the first `{` to the `{daily}` in the trailing prose and
    // threw, and `parseCronDraft` answered null: "could not turn that into a
    // job", every time, on the same model that had just answered correctly.
    const w = world({
      replies: [
        'Sure — here is the job:\n\n```json\n{"name":"inbox-brief","schedule":"0 8 * * 1-5","prompt":"Summarize the inbox and send a short brief."}\n```\n\nSet {daily} instead if you prefer weekends too.',
      ],
    })
    const res = await run(museCronHarness, ASK, w)
    expect(res.value).toEqual({ name: 'inbox-brief', schedule: '0 8 * * 1-5', prompt: 'Summarize the inbox and send a short brief.' })
    expect(res.schemaValid).toBe(true)
  })

  it('repairs a draft missing a field instead of giving up on the click', async () => {
    // The repair turn is the whole small-model story (audit 1.4), and this is
    // the flow where its absence was most visible: nothing anywhere in the tree
    // re-asked, so one dropped field cost the user the entire draft.
    const w = world({
      replies: ['{"name":"inbox-brief","prompt":"Summarize the inbox."}', '{"name":"inbox-brief","schedule":"every 24h","prompt":"Summarize the inbox."}'],
    })
    const res = await run(museCronHarness, ASK, w)
    expect(res.repairs).toBe(1)
    expect(res.value?.schedule).toBe('every 24h')
    expect(w.requests[1]?.messages.at(-1)?.content).toContain("missing required field 'schedule'")
  })

  it('asks for JSON at the protocol level and anchors it in the prompt', async () => {
    const w = world()
    await run(museCronHarness, ASK, w)
    expect(w.requests[0]?.jsonMode).toBe(true)
    expect(w.requests[0]?.messages.at(-1)?.content).toContain('exactly one JSON value')
  })

  it('refuses on a model MEASURED unable to produce JSON', async () => {
    // Muse used to degrade here — the form underneath is usable by hand, so a
    // shortcut that declines to try seemed worse than one that sometimes misses.
    // Under the narrowed fact (see `scoreJson`) this model does not "sometimes
    // miss": it returned no parseable object on any trial, so the shortcut would
    // miss every time and `onFailure: 'null'` leaves the form exactly as it would
    // have been. Refusing costs the user nothing and saves a call.
    const w = world({ facts: { json: false } })
    const res = await run(museCronHarness, ASK, w)
    expect(w.requests).toHaveLength(0)
    expect(res.error).toContain('cannot run harness "muse:cron"')
  })

  it('returns nothing rather than a half-draft, so the form keeps what it had', async () => {
    const w = world({ replies: ['I need to know what time of day you want this.', 'As I said, what time of day?'] })
    const res = await run(museCronHarness, ASK, w)
    expect(res.value).toBeNull()
    expect(res.escalate).toBe(false)
  })
})

describe('the cron contract', () => {
  const parse = (v: unknown) => MUSE_CRON.safeParse(v)

  it('trims, and treats a whitespace-only field as absent rather than present', () => {
    // `parseCronDraft` tested `!j.name || !j.schedule || !j.prompt`, so a field
    // of spaces failed the draft. Same outcome, now with a sentence the model
    // can act on instead of a silent null.
    expect(parse({ name: '  inbox-brief \n', schedule: '0 8 * * *', prompt: 'Do the thing.' }).data?.name).toBe('inbox-brief')
    expect(parse({ name: '   ', schedule: '0 8 * * *', prompt: 'x' }).success).toBe(false)
    expect(parse({ name: 'n', schedule: '0 8 * * *' }).success).toBe(false)
  })
})

describe('the schedule assertion', () => {
  // A schema constraint here would fail drafts the user could have used, so
  // this is measured rather than enforced — the builder opens anything it does
  // not recognize as a raw "custom" string.
  it('accepts the two shapes Hermes understands', () => {
    expect(looksLikeSchedule('0 8 * * 1-5')).toBe(true)
    expect(looksLikeSchedule('every 2h')).toBe(true)
    expect(looksLikeSchedule('30m')).toBe(true)
    expect(looksLikeSchedule('*/15 * * * *')).toBe(true)
  })

  it('rejects the prose a weak model answers with', () => {
    expect(looksLikeSchedule('every weekday morning')).toBe(false)
    expect(looksLikeSchedule('daily at 8am')).toBe(false)
  })
})

// ── agent ────────────────────────────────────────────────────────────────────

describe('the agent contract', () => {
  // Through the schema rather than the runner: `museAgentHarness.render` reads
  // the org profile to anchor the identity, and a unit test must not need a
  // database. Everything the client used to do lives in this schema now, so
  // this is where the coercion has to be held still.
  const parse = (v: unknown) => MUSE_AGENT.safeParse(v)
  const OK = { name: 'Release Manager', handle: 'releasemanager', department: 'release', role: 'Release Manager', soul: '# Release Manager\n## Who you are\n...' }

  it('coerces a hostile handle into the identifier alphabet', () => {
    // THE reason this could not stay on the client. A handle becomes a
    // container name and half of the fleet model id, and the create endpoint is
    // reachable without the browser that used to sanitize this.
    const out = parse({ ...OK, handle: '../../etc/passwd', department: 'Release Eng!' })
    expect(out.data?.handle).toBe('etcpasswd')
    expect(out.data?.department).toBe('releaseeng')
  })

  it('derives a missing handle from the name and a missing department from the handle', () => {
    const out = parse({ name: 'Release Manager', soul: 's' })
    expect(out.data?.handle).toBe('releasemanager')
    expect(out.data?.department).toBe('releasemanager')
  })

  it('falls back to the handle when the department coerces to nothing', () => {
    // An empty department would produce the fleet model id "remy-".
    expect(parse({ ...OK, department: '123' }).data?.department).toBe('releasemanager')
  })

  it('fails a handle too short to be an agent id, and says so in words a model can act on', () => {
    const out = parse({ ...OK, handle: '-' })
    expect(out.success).toBe(false)
    expect(out.error?.issues[0]?.message).toContain("'handle'")
  })

  it('kebabs skill names, drops the unusable ones, and caps the list at five', () => {
    const out = parse({
      ...OK,
      skills: [
        { name: 'Weekly Retro!', content: '# retro' },
        { name: '-', content: '# nope' }, // one character after coercion
        { name: 'no-content' }, // half a skill
        ...Array.from({ length: 6 }, (_, i) => ({ name: `skill-${i}`, content: 'x' })),
      ],
    })
    expect(out.data?.skills.map((s) => s.name)).toEqual(['weeklyretro', 'skill-0', 'skill-1', 'skill-2'])
  })

  it('requires a soul, because a soul is what an agent IS', () => {
    expect(parse({ name: 'Release Manager', handle: 'releasemanager' }).success).toBe(false)
    expect(parse({ name: 'Release Manager', soul: '   ' }).success).toBe(false)
  })

  it('bounds the free-text fields the way the client did', () => {
    const out = parse({ name: 'N'.repeat(200), handle: 'h'.repeat(200), role: 'R'.repeat(200), soul: 's' })
    expect(out.data?.name).toHaveLength(60)
    expect(out.data?.role).toHaveLength(80)
    expect(out.data?.handle).toHaveLength(30) // ident() caps too
  })
})

describe('the agent widening', () => {
  it('buys depth, never authority', () => {
    // The narrow branch is a real answer, not a degraded one: a 25-line skill
    // with the right steps in it is what the user edits anyway. What a capable
    // model earns is the ability to hold three full playbooks in one nested
    // object without truncating — which is `json-strict` and nothing else.
    expect(museAgentHarness.widen?.requires).toEqual(['json-strict'])
    // The floor refuses on `json` (derived — see `defineHarness`), which is a
    // different question from the widening: `json` is whether the model can
    // produce an object at all, `json-strict` is whether it can hold three
    // nested playbooks without truncating. Failing the second still earns the
    // narrow branch, which is a real answer.
    expect(museAgentHarness.floor.refuseBelow).toBe(true)
    expect(museAgentHarness.floor.capabilities).toContain('json')
  })
})

// ── ticket ───────────────────────────────────────────────────────────────────

const TICKET: MuseDraftInput = {
  instruction: 'make it urgent and due friday',
  context: 'now: 2026-03-03T09:00:00.000Z',
  current: JSON.stringify({ title: 'Ship the ledger migration', priority: 'medium', status: 'assigned' }),
}

describe('the ticket patch contract', () => {
  const parse = (v: unknown) => MUSE_TICKET.safeParse(v)

  it('drops fields outside the allowlist instead of failing the patch', () => {
    // zod strips unrecognized keys, which is exactly `parseTicketPatch`'s
    // behavior: a model that helpfully invents `assignees` should lose the
    // field, not the whole edit.
    const out = parse({ priority: 'urgent', assignees: ['user:abc'], boardId: 'x' })
    expect(out.data).toEqual({ priority: 'urgent' })
  })

  it('names an out-of-vocabulary enum so the repair turn can fix it', () => {
    // `parseTicketPatch` accepted `priority: string` — any string at all — and
    // "P1" travelled on toward the save path.
    const out = parse({ priority: 'P1' })
    expect(out.success).toBe(false)
    const status = parse({ status: 'in-progress' })
    expect(status.success).toBe(false)
  })

  it('will not park a ticket in an off-board terminal state', () => {
    // 'failed' and 'cancelled' are legal statuses nothing on the board may move
    // work into. A natural-language edit is not where that power is acquired.
    expect(parse({ status: 'cancelled' }).success).toBe(false)
    expect(parse({ status: 'done' }).success).toBe(true)
  })

  it('keeps the error escape hatch exclusive of any edit', () => {
    expect(parse({ error: 'I cannot change assignees.', status: 'done' }).data).toEqual({ error: 'I cannot change assignees.' })
  })

  it('rejects an empty patch, which is not an answer', () => {
    expect(parse({}).success).toBe(false)
  })

  it('keeps null as a CLEAR and rejects it where clearing is meaningless', () => {
    expect(parse({ effort: null, dueDate: null, color: null }).success).toBe(true)
    expect(parse({ priority: null }).success).toBe(false)
  })

  it('holds dates to the shape the write path accepts, so the repair turn can fire', () => {
    // "make it due friday" is the single likeliest thing anyone types into this
    // bar, and every natural answer to it used to pass here and 400 at
    // PUT /api/tasks/:id — where the bar has already cleared its preview, so the
    // whole patch (including a perfectly good `priority`) vanished with nothing
    // shown. A date in the wrong format is exactly what one repair turn fixes.
    for (const d of ['2026-03-06', 'Friday', 'next friday', '03/06/2026', '2026-03-06 17:00', '2026-03-06T09:00']) {
      expect(parse({ priority: 'urgent', dueDate: d }).success, d).toBe(false)
    }
    expect(parse({ dueDate: '2026-03-06T00:00:00Z' }).success).toBe(true)
    expect(parse({ startDate: '2026-03-06T00:00:00.000Z' }).success).toBe(true)
  })

  it('holds every other field to the route’s bounds too, for the same reason', () => {
    // The dates were not the only place this schema was looser than `Patch` in
    // routes/api/tasks.$id.ts, and every gap has the identical shape: the
    // harness records a held contract, the PUT 400s, and the command bar
    // swallows it with the preview already cleared. A named issue buys a repair
    // turn; a 400 buys nothing.
    expect(parse({ title: '   ' }).success).toBe(false)
    expect(parse({ title: 'T'.repeat(301) }).success).toBe(false)
    expect(parse({ description: 'd'.repeat(20_001) }).success).toBe(false)
    expect(parse({ estimatedHours: -1 }).success).toBe(false)
    expect(parse({ estimatedHours: 1_000 }).success).toBe(false)
    expect(parse({ tags: [''] }).success).toBe(false)
    expect(parse({ tags: ['t'.repeat(41)] }).success).toBe(false)
    expect(parse({ tags: Array.from({ length: 21 }, (_, i) => `t${i}`) }).success).toBe(false)
    // And still accepts everything the route does.
    expect(parse({ title: 'Ship it', estimatedHours: 8, tags: ['launch'] }).success).toBe(true)
    expect(parse({ tags: [] }).success).toBe(true)
  })
})

describe('the ticket harness', () => {
  it('parses a patch the model wrapped in prose', async () => {
    const w = world({ replies: ['I will set both:\n\n{"priority":"urgent","dueDate":"2026-03-06T17:00:00.000Z"}\n\nAnything else?'] })
    const res = await run(museTicketHarness, TICKET, w)
    expect(res.value).toEqual({ priority: 'urgent', dueDate: '2026-03-06T17:00:00.000Z' })
  })

  // ── the date anchor: what the schema cannot say ───────────────────────────
  //
  // `z.string().datetime()` matches `Patch` in routes/api/tasks.$id.ts character
  // for character, which is everything a module constant can say about a date —
  // it is built at import time and the ticket's clock arrives with the run. So
  // the FORMAT bug is closed and the ANCHOR bug is not, and the anchor bug is
  // the worse of the two: a malformed date at least 400s, while a well-formed
  // one worked out from the model's own training cutoff is accepted by the
  // route, written to the board, and shows up as a ticket that has been overdue
  // for two years. Nothing errors anywhere.

  const stale = '{"priority":"urgent","dueDate":"2024-03-08T17:00:00.000Z"}'
  const right = '{"priority":"urgent","dueDate":"2026-03-06T17:00:00.000Z"}'

  it('does not fire on a date worked out from the time it was given', async () => {
    const res = await run(museTicketHarness, TICKET, world({ replies: [right] }))
    expect(res.value).toEqual({ priority: 'urgent', dueDate: '2026-03-06T17:00:00.000Z' })
    expect(res.repairs).toBe(0)
    expect(res.schemaValid).toBe(true)
  })

  it('does not fire on a date the user genuinely backdated', async () => {
    // "it was due last week" is a real instruction and lands well inside the
    // year of tolerance. The check names a stale clock, not a past date.
    const w = world({ replies: ['{"dueDate":"2026-02-24T17:00:00.000Z"}'] })
    const res = await run(museTicketHarness, TICKET, w)
    expect(res.schemaValid).toBe(true)
    expect(res.repairs).toBe(0)
  })

  it('fails a date worked out from the model’s own idea of today, then repairs it', async () => {
    const w = world({ replies: [stale, right] })
    const res = await run(museTicketHarness, TICKET, w)
    expect(res.repairs).toBe(1)
    expect(res.value).toEqual({ priority: 'urgent', dueDate: '2026-03-06T17:00:00.000Z' })
    const repair = w.requests[1]?.messages.at(-1)?.content ?? ''
    // Quotes the clock it was handed and says what to do with it.
    expect(repair).toContain('you set dueDate to 2024-03-08T17:00:00.000Z')
    expect(repair).toContain('2026-03-03T09:00:00.000Z')
  })

  it('is an honest contract failure when the repair does not take', async () => {
    const w = world({ replies: [stale, stale] })
    const res = await run(museTicketHarness, TICKET, w)
    expect(res.value).toBeNull()
    expect(res.schemaValid).toBe(false)
    expect(res.error).toContain('more than a year before the current time')
  })

  it('declines when the caller stated no clock, rather than grading against one the model never saw', async () => {
    const w = world({ replies: [stale] })
    const res = await run(museTicketHarness, { instruction: 'make it urgent and due friday' }, w)
    expect(res.schemaValid).toBe(true)
    expect(res.value).toEqual({ priority: 'urgent', dueDate: '2024-03-08T17:00:00.000Z' })
  })

  it('passes the refusal through as a value rather than as a failure', async () => {
    // "I cannot do that" is a correct answer to "assign this to Dana", and it
    // has to reach the user as a sentence instead of as a parse failure.
    const w = world({ replies: ['{"error":"I can only change ticket fields, not assignees."}'] })
    const res = await run(museTicketHarness, TICKET, w)
    expect(res.value).toEqual({ error: 'I can only change ticket fields, not assignees.' })
    expect(res.schemaValid).toBe(true)
  })
})

// ── the eval fixtures ────────────────────────────────────────────────────────

describe('the eval fixtures', () => {
  // BY NAME, NOT BY INDEX. These used to be `[0]` and `[1]` against suites of
  // exactly two, so every one of them silently re-pointed at a different
  // fixture the moment the suites grew — and the failures read as "the muse
  // check is wrong" rather than "this test is holding the wrong fixture".
  const named = <T,>(evals: ReadonlyArray<{ name: string; check: (v: T, ctx: typeof NO_TOOLS) => CheckResult }> | undefined, name: string) => {
    const found = evals?.find((e) => e.name === name)
    if (!found) throw new Error(`no fixture called "${name}"`)
    return found.check
  }

  it('are deterministic assertions, so the fitness suite needs no second model', () => {
    // WHAT THIS ACTUALLY ASSERTS is that nothing in a fixture reaches for a
    // model — it is a plain function over a value. It used to assert a fixture
    // COUNT, which measured nothing and broke every time a suite grew.
    for (const h of [museCronHarness, museAgentHarness, museTicketHarness, museDraftHarness]) {
      expect(h.evals?.length ?? 0).toBeGreaterThanOrEqual(8)
      for (const e of h.evals ?? []) expect(typeof e.check).toBe('function')
    }
  })

  it('score a good cron draft clean and a prose schedule dirty', () => {
    const good = { name: 'inbox-brief', schedule: '0 8 * * 1-5', prompt: 'Summarize the inbox into a brief and send it to me.' }
    expect(named(museCronHarness.evals, 'a weekday morning brief')(good, NO_TOOLS)).toBeNull()
    expect(named(museCronHarness.evals, 'a weekday morning brief')({ ...good, schedule: 'every weekday morning' }, NO_TOOLS)).toContain('neither a 5-field cron')
    expect(named(museCronHarness.evals, 'a weekday morning brief')({ ...good, name: 'Inbox Brief' }, NO_TOOLS)).toContain('not kebab-case')
  })

  it('catch a soul that skipped a required heading', () => {
    const draft: AgentDraft = {
      name: 'Release Manager',
      handle: 'releasemanager',
      department: 'release',
      role: 'Release Manager',
      soul: '# Release Manager\n## Who you are\nx\n## How you work\ny',
      skills: [],
    }
    expect(named(museAgentHarness.evals, 'a release manager')(draft, NO_TOOLS)).toContain('## Voice & personality')
  })

  it('catch the ticket edit that did more than it was asked to', () => {
    // The assertion the audit named: a model that helpfully rewrites the title
    // or moves the ticket has done something the user did not sanction, and
    // this is where that shows up as a red cell rather than as a surprise.
    const asked: TicketMusePatch = { priority: 'urgent', dueDate: '2026-03-06T17:00:00.000Z' }
    expect(named(museTicketHarness.evals, 'two fields, named')(asked, NO_TOOLS)).toBeNull()
    expect(named(museTicketHarness.evals, 'two fields, named')({ ...asked, status: 'in_progress', title: 'Ship it' }, NO_TOOLS)).toContain('did not ask for')
  })

  it('score the date anchor through the same function the contract enforces', () => {
    // The fixture grades against the clock its own input puts in the prompt, and
    // it calls `dateAnchorIssue` to do it — one function, so the offline score
    // and `harness_runs.schema_valid` cannot come to disagree about one reply,
    // which is the defect this whole round is about.
    expect(named(museTicketHarness.evals, 'two fields, named')({ priority: 'urgent', dueDate: '2024-03-08T17:00:00.000Z' }, NO_TOOLS)).toContain('more than a year before')
  })

  it('catch a model that invents a patch for an instruction it cannot carry out', () => {
    expect(named(museTicketHarness.evals, 'outside the fields it may change')({ error: 'I cannot change assignees.' }, NO_TOOLS)).toBeNull()
    expect(named(museTicketHarness.evals, 'outside the fields it may change')({ status: 'in_progress' }, NO_TOOLS)).toContain('invented a patch')
  })
})

// ── the six prose kinds ──────────────────────────────────────────────────────

const DRAFT: MuseProseInput = { kind: 'skill', instruction: 'a playbook for triaging a failed deploy' }

describe('the prose draft harness', () => {
  it('hands back exactly what landed in the editor, with nothing cleaned off it', async () => {
    // The contract is deliberately the raw reply (see the note on `output`): the
    // six prose kinds STREAM, so a whole-text cleaner could only ever run after
    // every character was already in the user's editor. Scoring a cleaned string
    // the product never produces would make the fitness matrix a fiction.
    const reply = '```md\n# Triage a failed deploy\n\n1. Read the run log.\n```'
    const w = world({ replies: [reply] })
    const res = await run(museDraftHarness, DRAFT, w)
    expect(res.value).toBe(reply)
    expect(res.schemaValid).toBe(true)
    // No JSON anchor and no response_format: this is prose, and asking a small
    // model for "exactly one JSON value" here would be actively harmful.
    expect(w.requests[0]?.jsonMode).toBe(false)
    expect(w.requests[0]?.messages.at(-1)?.content).not.toContain('exactly one JSON value')
    expect(w.requests[0]?.temperature).toBe(0.4)
  })

  it('records the run rather than the value when the model returns nothing', async () => {
    // The failure the audit cared about is the invisible one. A draft that comes
    // back empty leaves the editor untouched (onFailure: 'null') AND leaves a
    // harness_runs row saying so, which is the whole difference from before.
    const w = world({ replies: [''] })
    const res = await run(museDraftHarness, DRAFT, w)
    expect(res.value).toBeNull()
    expect(res.schemaValid).toBe(false)
    expect(res.error).toContain('returned nothing')
  })

  it('guards a drafted document for credentials and redacts the value it returns', async () => {
    // THE audit finding, in one case (1.5, the Muse row): a drafted soul
    // carrying a credential was neither flagged nor redacted, and a soul is
    // rendered into an agent's context on every run.
    const key = `sk-ant-api03-${'A'.repeat(40)}`
    const w = world({ replies: [`# Deploy triage\n\nCall the API with ${key} and read the log.`] })
    const res = await run(museDraftHarness, DRAFT, w)
    expect(res.findings.map((f) => f.check)).toContain('secret_leak')
    expect(res.value).not.toContain(key)
    expect(res.value).toContain('[redacted Anthropic key]')
  })

  it('declares the two content rules and nothing that would fire on every draft', () => {
    // `zero_tool_claim` on a drafted soul that says "when you have created the
    // ticket, say so" would file a finding against every agent design and poison
    // the per-model confabulation rate the fitness page reads.
    expect(museDraftHarness.guard?.rules).toEqual(['secret_leak', 'pii_leak'])
    expect(museDraftHarness.guard?.redact).toBe(true)
  })

  it('never refuses, and never widens', () => {
    // The editor underneath is fully usable by hand, so a shortcut that declines
    // to try is worse than one that sometimes needs tidying up after. And the
    // model that DRAFTS a soul is not the model that runs the agent — see the
    // argument on the definition.
    expect(museDraftHarness.floor.capabilities).toEqual([])
    expect(museDraftHarness.floor.refuseBelow).toBe(false)
    expect(museDraftHarness.widen).toBeUndefined()
  })
})

describe('the prose eval fixtures', () => {
  describe('the template line count', () => {
    it('lets a big process come back as section names, which is what the prompt asks for', () => {
      // THE CONTRADICTION THIS FIXES. `SYSTEM.template` says both "whole template
      // under 25 lines" AND "if the request describes a big process, capture it as
      // section NAMES, not content". An incident runbook needs five sections; five
      // sections with a description line and placeholder bullets is 26 raw lines
      // and about ten lines of actual content. gemma produced exactly that — the
      // skeleton the prompt asked for — and was told it had written a document.
      const skeleton = ['Detection & Triage', 'Communication Plan', 'Mitigation Steps', 'Verification', 'Postmortem']
        .map((h) => `## ${h}\n_What this section covers._\n- \n- \n- \n`)
        .join('\n')

      expect(skeleton.trim().split('\n').length).toBeGreaterThan(25)
      expect(check(2, skeleton)).toBeNull()
    })

    it('still catches a model that writes the document instead of the shape', () => {
      const document = ['## Detection', '## Response', '## Review', ...Array.from({ length: 30 }, (_, i) => `- Step ${i}: do the thing carefully.`)].join('\n')
      expect(check(2, document)).toContain('a document rather than a skeleton')
    })
    })

  const check = (i: number, v: string): CheckResult => {
    const fixture = museDraftHarness.evals?.[i]
    if (!fixture) throw new Error(`no eval fixture at index ${i}`)
    return fixture.check(v, NO_TOOLS)
  }

  const SOUL = [
    '# Release Manager',
    '## Who you are',
    'You keep the deploy trains running for Northwind.',
    '## Voice & personality',
    'Dry, calm, allergic to drama.',
    '## How you work',
    '- Never start a deploy on a Friday without a named approver.',
    '- Keep humans in the loop: create and triage tickets, never assign or close them.',
  ].join('\n')

  it('pass a revision that made the change and kept everything else', () => {
    expect(check(0, SOUL)).toBeNull()
  })

  it('catch the revision that quietly dropped a section', () => {
    expect(check(0, SOUL.replace('## Voice & personality\nDry, calm, allergic to drama.\n', ''))).toContain('## Voice & personality')
  })

  it('catch the revision that dropped the guardrail it was not asked about', () => {
    expect(check(0, SOUL.replace('- Keep humans in the loop: create and triage tickets, never assign or close them.', '- Move fast.'))).toContain(
      'keep-humans-in-the-loop',
    )
  })

  it('catch the lead-in and the code fence, which are what a small model adds', () => {
    expect(check(0, `Here's the updated soul:\n\n${SOUL}`)).toContain('must BE the document')
    expect(check(0, '```md\n' + SOUL + '\n```')).toContain('code fence')
  })

  it('pass a template that stayed a skeleton', () => {
    const template = ['## Summary', '_What broke, in two sentences._', '## Steps to reproduce', '- ', '- ', '## Expected', '_What should have happened._'].join(
      '\n',
    )
    expect(check(1, template)).toBeNull()
  })

  it("catch the four ways a model breaks the template's hard rules", () => {
    // A `#` title is how a model that heard "document" instead of "template"
    // starts, and it trips the shape rule before the section rules are reached.
    expect(check(1, ['# Bug report', '## Summary', '_x_', '## Steps', '- ', '## Expected', '_y_'].join('\n'))).toContain('must BE the document')
    expect(check(1, ['## Summary', '_x_', '### Detail', 'y', '## Steps', '- ', '## Expected', '_z_'].join('\n'))).toContain('"##" only')
    // One section short of the range is within the margin (see `countProblem`);
    // a single-section "template" is the different kind of thing the rule is for.
    expect(check(1, ['## Summary', '_x_', '## Steps', '- '].join('\n'))).toBeNull()
    expect(check(1, ['## Summary', '_x_'].join('\n'))).toContain('1 section')
    // The overbuild rule: asked for a whole runbook, it wrote the runbook.
    expect(check(2, ['## Detection', ...Array.from({ length: 30 }, (_, i) => `- step ${i}`)].join('\n'))).toContain('under 25')
  })
})

// ── streaming redaction ──────────────────────────────────────────────────────

/** Feed `text` through the redactor in fixed-size chunks and collect every byte
 *  that was relayed, in order. `sizes` covers the boundaries that matter: one
 *  character at a time is the pathological case a real SSE stream approximates. */
const relay = (text: string, size: number): { out: string; beforeFlush: string } => {
  const r = createStreamRedactor(redactSecrets)
  let out = ''
  for (let i = 0; i < text.length; i += size) out += r.push(text.slice(i, i + size))
  const beforeFlush = out
  return { out: out + r.flush(), beforeFlush }
}

describe('the stream redactor', () => {
  const DOC = ['# Triage a failed deploy', '', 'Use this when a deploy train stops.', '', '1. Read the run log.', '2. Post in the channel.'].join('\n')

  it('relays a clean document unchanged, and relays most of it before the end', () => {
    for (const size of [1, 3, 17, 4096]) {
      const { out, beforeFlush } = relay(DOC, size)
      expect(out, `chunk size ${size}`).toBe(DOC)
      // The point of the whole design: this is a STREAM, not a buffer with a
      // guard on the end. Everything but the held tail is already gone.
      if (size < DOC.length) expect(beforeFlush.length, `chunk size ${size}`).toBeGreaterThan(DOC.length / 2)
    }
  })

  it('redacts a credential that arrived split across chunks, and never relays a byte of it', () => {
    const key = `sk-ant-api03-${'A'.repeat(40)}`
    const text = `Call the API with ${key} and read the log.\n\nThen post the result.`
    for (const size of [1, 5, 13, 4096]) {
      const { out } = relay(text, size)
      expect(out, `chunk size ${size}`).toBe(redactSecrets(text).text)
      expect(out).not.toContain('sk-ant-')
    }
  })

  it('holds a private key block until the stream ends, because that block spans newlines', () => {
    // The one pattern that is unbounded and multi-line: there is no cut inside
    // it that could ever be safe, so nothing after the BEGIN marker is relayed.
    const text = `Here is the deploy key:\n\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nAAAABG5vbmU=\n-----END OPENSSH PRIVATE KEY-----\n\nKeep it safe.`
    const { out, beforeFlush } = relay(text, 7)
    expect(beforeFlush).not.toContain('BEGIN')
    expect(beforeFlush).not.toContain('b3BlbnNzaC1rZXktdjEAAAAA')
    expect(out).toBe(redactSecrets(text).text)
    expect(out).toContain('[redacted Private key block]')
  })

  it('does not cut a card number in half at one of its own spaces', () => {
    // Every other pattern is a run of non-whitespace; the card pattern is the
    // exception, and a naive whitespace cut would relay "4111 1111" unredacted
    // and hold the rest.
    const text = 'Charge it to 4111 1111 1111 1111 and keep the receipt.'
    for (const size of [1, 2, 9]) {
      const { out } = relay(text, size)
      expect(out, `chunk size ${size}`).toBe(redactSecrets(text).text)
      expect(out).toContain('[redacted card number]')
      expect(out).not.toContain('4111')
    }
  })

  it('costs a whitespace-free run linear work, not quadratic', () => {
    // `onDelta` runs synchronously inside the transport's SSE read loop, so this
    // scan is the Node event loop. Restarting the backwards scan at the whole
    // buffer on every delta made a long run with no cut point in it — a base64
    // data URI, a minified bundle, a hex chain, all ordinary inside a drafted
    // document — O(n²): 20k characters measured ~0.9s and 80k ~13s of blocked
    // process, serving nobody. Doubling the input must roughly double the time,
    // not quadruple it; the ratio is asserted rather than a wall-clock bound so
    // this does not turn into a flaky machine-speed test.
    const blob = (n: number) => 'A'.repeat(n)
    const time = (n: number): number => {
      const r = createStreamRedactor(redactSecrets)
      const text = blob(n)
      const t0 = performance.now()
      for (let i = 0; i < text.length; i += 4) r.push(text.slice(i, i + 4))
      r.flush()
      return performance.now() - t0
    }
    time(20_000) // warm up, so the first JIT pass is not the measurement
    // BEST OF THREE, EACH SIDE. A ratio of two single samples is still a
    // wall-clock measurement wearing a disguise: on a loaded CI box one
    // scheduler preemption inside the small run inflates the ratio, and this
    // test went red at 8.2 on an otherwise green tree. The minimum is the
    // sample least contaminated by the machine, and taking it on both sides
    // keeps the comparison honest in both directions. The bound stays at 8 —
    // quadratic is ~16x for 4x the input, and nothing near 8 is linear noise.
    const best = (n: number): number => Math.min(time(n), time(n), time(n))
    const small = Math.max(best(20_000), 0.5)
    const large = best(80_000)
    expect(large / small).toBeLessThan(8)
  })

  it('still finds a cut point after a long uncuttable run', () => {
    // The scan watermark must not swallow whitespace that arrives later.
    const text = `${'A'.repeat(2_000)} then some ordinary prose to relay.`
    const { out } = relay(text, 4)
    expect(out).toBe(text)
  })

  it('holds back nothing but the tail once the stream is done', () => {
    const r = createStreamRedactor(redactSecrets)
    r.push('one two three four five six seven eight nine ten eleven twelve')
    expect(r.flush()).not.toBe('')
    expect(r.flush()).toBe('')
  })


})
