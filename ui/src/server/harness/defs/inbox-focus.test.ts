import { describe, expect, it } from 'vitest'
import { NO_TOOLS } from '@/server/harness/define'
import type { Finding, GuardConfig } from '@/server/guardrails'
import { runHarness, type HarnessDeps, type HarnessRunRow, type TransportRequest } from '@/server/harness/run'
import type { Capability } from '@/server/harness/capability'
import { validateCommandObject } from '@/server/inbox-focus-policy'
import {
  allowedFocusActionIds,
  inboxBriefHarness,
  inboxCommandHarness,
  type FocusCommandInput,
} from '@/server/harness/defs/inbox-focus'

// AUDIT 1.8 is a DECISION, and this file is where it is held still. Widening
// hands a model that has proved `tool-select` the item's own action list instead
// of the single action three regexes matched. The thing that must remain true
// either way is that `validateCommandObject` — not the schema, not the prompt —
// decides what the model was allowed to say, and that its allowlist can never
// contain an action the card does not carry.
//
// Everything below runs against RECORDED REPLIES through injected deps: no
// database, no gateway, no fleet.

const ACTIONS = [
  { id: 'approve_task', label: 'Approve', risk: 'confirmation' as const, confirmationRequired: true, reversible: false },
  { id: 'request_changes', label: 'Request changes', risk: 'reversible' as const, confirmationRequired: false, reversible: true },
]

const commandInput = (over: Partial<FocusCommandInput> = {}): FocusCommandInput => ({
  item: {
    key: 'task:t1',
    question: 'Approve the completed work for "Ledger migration"?',
    sourceHref: '/boards/platform/t1',
    evidence: [{ label: 'Source', text: 'Dex reports the migration is done.' }],
    metadata: { status: 'review' },
    actions: ACTIONS,
  },
  instruction: 'approve it',
  history: [],
  mode: 'normal',
  deterministicActionId: null,
  role: 'orchestrator',
  specialist: null,
  ...over,
})

interface World {
  replies?: string[]
  facts?: Partial<Record<Capability, boolean>>
}

interface Recorder {
  requests: TransportRequest[]
  runs: HarnessRunRow[]
  deps: Partial<HarnessDeps>
}

const CONFIG: GuardConfig = { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }

function world(w: World = {}): Recorder {
  const requests: TransportRequest[] = []
  const runs: HarnessRunRow[] = []
  const replies = w.replies ?? ['{"message":"Ready to approve.","actionId":"approve_task"}']
  const facts: Partial<Record<Capability, { value: boolean; source: 'probe' }>> = {}
  // `source: 'probe'` is what the widening gate requires — a vendor's declared
  // claim must never hand this harness the item's whole action list.
  for (const [cap, value] of Object.entries(w.facts ?? {})) facts[cap as Capability] = { value, source: 'probe' }
  let clock = 0
  return {
    requests,
    runs,
    deps: {
      resolveModel: async () => ({ model: 'pl-main', step: 'pin' }),
      routing: async (model) => ({ endpoints: ['spark'], upstreamModel: model }),
      missingCapabilities: async (_key, required) => required.filter((cap) => facts[cap]?.value === false),
      capabilities: async () => facts,
      transport: async (req) => {
        requests.push(req)
        return {
          kind: 'gateway',
          text: replies[Math.min(requests.length - 1, replies.length - 1)] ?? '',
          toolNames: [],
          usage: null,
          contractDropped: false,
        }
      },
      guardConfig: async () => CONFIG,
      guardText: async () => [] as Finding[],
      recordFindings: async () => {},
      recordRun: async (row) => {
        runs.push(row)
      },
      now: () => (clock += 3),
    },
  }
}

/** What `requestFocusCommand` does, minus the deadline plumbing: run, then gate
 *  the value on the allowlist derived from the SAME function `render` used. */
async function commandTurn(input: FocusCommandInput, r: Recorder) {
  const result = await runHarness(inboxCommandHarness, input, { caller: 'test:inbox', model: 'pl-main', deps: r.deps })
  const turn = result.value ? validateCommandObject(result.value, new Set(allowedFocusActionIds(input, result.widened))) : null
  return { result, turn }
}

const WIDE = { 'tool-select': true, 'instruction-following': true } as const

// ── The allowlist itself ─────────────────────────────────────────────────────

describe('allowedFocusActionIds', () => {
  it('is the single regex match when the model has not earned the wider surface', () => {
    expect(allowedFocusActionIds(commandInput({ deterministicActionId: 'approve_task' }), false)).toEqual(['approve_task'])
  })

  it('is empty when no regex matched — the model may only clarify', () => {
    expect(allowedFocusActionIds(commandInput(), false)).toEqual([])
  })

  it('is the ITEM’S OWN action list when widened, never anything beyond it', () => {
    expect(allowedFocusActionIds(commandInput(), true)).toEqual(['approve_task', 'request_changes'])
  })

  it('ignores a deterministic match that is not on the item', () => {
    // deterministicProposal already checks this. Restating it here means "never
    // outside the item's own actions" is true by construction in one place.
    expect(allowedFocusActionIds(commandInput({ deterministicActionId: 'delete' }), false)).toEqual([])
  })

  it('authorizes NOTHING in plan mode, widened or not', () => {
    // Plan mode is a choice the owner made. A capability does not overrule it.
    expect(allowedFocusActionIds(commandInput({ mode: 'plan' }), true)).toEqual([])
    expect(allowedFocusActionIds(commandInput({ mode: 'plan', deterministicActionId: 'approve_task' }), false)).toEqual([])
  })
})

// ── Widening, end to end ─────────────────────────────────────────────────────

describe('the widened command surface', () => {
  it('shows a widened model every action on the card', async () => {
    const r = world({ facts: WIDE })
    const { result, turn } = await commandTurn(commandInput(), r)

    expect(result.widened).toBe(true)
    expect(r.requests[0]?.messages[0]?.content).toContain('["approve_task","request_changes"]')
    // The regex matched nothing, and today that would have forced a
    // clarification. A model that earned the surface can act.
    expect(turn).toEqual({ kind: 'proposal', message: 'Ready to approve.', actionId: 'approve_task' })
  })

  it('shows an unproven model only what the regex matched', async () => {
    const r = world()
    const { result, turn } = await commandTurn(commandInput({ deterministicActionId: 'approve_task' }), r)

    expect(result.widened).toBe(false)
    expect(r.requests[0]?.messages[0]?.content).toContain('["approve_task"]')
    expect(turn?.actionId).toBe('approve_task')
  })

  it('does not widen on an unprobed model — unknown is not earned', async () => {
    const r = world({ facts: { 'tool-select': true } })
    const { result } = await commandTurn(commandInput(), r)
    // `instruction-following` is unknown, so the pair is not satisfied.
    expect(result.widened).toBe(false)
  })

  it('WIDENED, A MODEL STILL CANNOT PROPOSE AN ACTION THAT IS NOT ON THE ITEM', async () => {
    // The load-bearing assertion of audit 1.8. The model is fully widened, it
    // answers with a plausible-sounding action id, and it does not get it —
    // because the widened allowlist is the item's actions, not the model's
    // imagination. A rejected proposal is null, which is exactly the value
    // `runFocusCommand` falls through on.
    const r = world({ facts: WIDE, replies: ['{"message":"Deleting it now.","actionId":"delete_task"}'] })
    const { result, turn } = await commandTurn(commandInput(), r)

    expect(result.widened).toBe(true)
    expect(turn).toBeNull()
    // WHAT THIS USED TO ASSERT: `schemaValid: true` — "the SHAPE was fine; the
    // AUTHORITY was not". The shape WAS fine and the authority gate did reject
    // it, but the run row then reported a model that held its contract while its
    // caller threw the answer away, which is the one thing `harness_runs` must
    // never say. The allowlist is part of the contract now (`output.verify`), so
    // the model gets one repair turn naming the ids it may use, and a model that
    // still cannot stay inside them is recorded as having failed.
    expect(result.schemaValid).toBe(false)
    expect(result.repairs).toBe(1)
    expect(r.requests[1]?.messages.at(-1)?.content).toContain('["approve_task","request_changes"]')
  })

  it('UNWIDENED, the contract is the narrower list — which only `ctx.widened` can express', async () => {
    // `request_changes` IS on the item, so a verify written from `(value, input)`
    // alone would have to accept it. On the unwidened surface the model was shown
    // `["approve_task"]` and nothing else, and proposing anything else is the
    // model ignoring an explicit constraint — the exact `instruction-following`
    // failure this harness's `requires` names. Passing `render`'s own
    // `RenderContext` to `verify` is what makes the two lists the same list.
    const r = world({ replies: ['{"message":"Sending it back.","actionId":"request_changes"}'] })
    const { result, turn } = await commandTurn(commandInput({ deterministicActionId: 'approve_task' }), r)

    expect(result.widened).toBe(false)
    expect(result.schemaValid).toBe(false)
    expect(r.requests[1]?.messages.at(-1)?.content).toContain('["approve_task"]')
    expect(turn).toBeNull()
  })

  it('widening cannot rescue an action in plan mode', async () => {
    const r = world({ facts: WIDE, replies: ['{"message":"Approving.","actionId":"approve_task"}'] })
    const { result, turn } = await commandTurn(commandInput({ mode: 'plan' }), r)

    expect(result.widened).toBe(true)
    expect(r.requests[0]?.messages[0]?.content).toContain('[Response mode: plan.')
    expect(turn).toBeNull()
    // Plan mode is the owner's choice, so proposing anything at all is a failed
    // contract and is recorded as one — the repair says so in the model's own
    // terms rather than leaving the turn to degrade into a clarification.
    expect(result.schemaValid).toBe(false)
    expect(r.requests[1]?.messages.at(-1)?.content).toContain('plan mode')
  })

  it('a fleet persona is never widened, because nothing has probed it', async () => {
    // Capability facts are keyed 'endpoint:model' and a persona has no gateway
    // route, so the runner sees no keys and holds the deterministic surface.
    const r = world({ facts: WIDE })
    const result = await runHarness(inboxCommandHarness, commandInput(), {
      caller: 'test:inbox',
      model: 'penny-administrative-assistant',
      deps: {
        ...r.deps,
        routing: async () => {
          throw new Error('not a gateway model')
        },
      },
    })
    expect(result.widened).toBe(false)
  })
})

// ── One structured-output strategy, both transports (audit 1.3) ──────────────

describe('the unified structured request', () => {
  it('asks for JSON at the protocol level and anchors it in the prompt', async () => {
    const r = world()
    await commandTurn(commandInput(), r)
    const req = r.requests[0]!
    expect(req.jsonMode).toBe(true)
    expect(req.temperature).toBe(0.1)
    expect(req.messages.at(-1)?.content).toContain('exactly one JSON value')
  })

  it('repairs a reply the model buried in prose rather than losing the turn', async () => {
    // The exact shape that broke indexOf('{')/lastIndexOf('}'): an object, then
    // an explanation containing a brace. Nothing in the tree re-asked before.
    const r = world({
      replies: [
        'Here you go:\n\n{"message":"Ready to approve.","actionId":"approve"}\n\nUse {actionId} to run it.',
        '{"message":"Ready to approve.","actionId":"approve_task"}',
      ],
    })
    const { result, turn } = await commandTurn(commandInput({ deterministicActionId: 'approve_task' }), r)
    // The first reply PARSED — the extractor handles the trailing brace prose,
    // which is the thing this fixture was written for. It then failed the
    // allowlist half of the contract on `approve` vs `approve_task`, which is
    // the likeliest small-model mistake on this harness and now the one it gets
    // a repair turn for. It used to be a silent `schemaValid: true` and a turn
    // the caller dropped.
    expect(result.repairs).toBe(1)
    expect(result.schemaValid).toBe(true)
    expect(turn?.actionId).toBe('approve_task')
  })

  it('re-asks once when the shape is wrong, with the concrete field named', async () => {
    const r = world({ replies: ['{"actionId":"approve_task"}', '{"message":"Ready to approve.","actionId":"approve_task"}'] })
    const { result, turn } = await commandTurn(commandInput({ deterministicActionId: 'approve_task' }), r)

    expect(result.repairs).toBe(1)
    expect(r.requests[1]?.messages.at(-1)?.content).toContain("'message'")
    expect(turn?.actionId).toBe('approve_task')
  })

  it('fails a reply proposal with no text to post, which earns it a repair turn', async () => {
    const r = world({
      replies: [
        '{"message":"Sent.","actionId":"reply"}',
        '{"message":"I prepared this reply.","actionId":"reply","payload":{"message":"On track for today."}}',
      ],
    })
    const input = commandInput({
      item: {
        key: 'channel:c1',
        question: 'Reply in #platform?',
        sourceHref: '/comms/platform',
        evidence: [],
        metadata: {},
        actions: [{ id: 'reply', label: 'Reply', risk: 'confirmation', confirmationRequired: true, reversible: false }],
      },
      instruction: 'reply that we are on track',
      deterministicActionId: 'reply',
    })
    const { result, turn } = await commandTurn(input, r)

    expect(result.repairs).toBe(1)
    // The repair turn names the missing thing, which is the difference between
    // a small model fixing its reply and rewriting it from scratch.
    expect(r.requests[1]?.messages.at(-1)?.content).toContain('payload.message')
    expect(turn).toEqual({ kind: 'proposal', message: 'I prepared this reply.', actionId: 'reply', payload: { message: 'On track for today.' } })
  })

  it('returns null so the caller keeps its own fallback, rather than throwing', async () => {
    // Fire-and-forget is the property that must survive the port:
    // `runFocusCommand` falls through to the specialist, then the deterministic
    // proposal, then a clarification. None of that happens if this throws.
    const r = world({ replies: ['I am not sure what you mean.', 'Still not sure.'] })
    const { result, turn } = await commandTurn(commandInput(), r)
    expect(result.value).toBeNull()
    expect(turn).toBeNull()
    expect(r.runs[0]?.schemaValid).toBe(false)
  })
})

// ── The brief ────────────────────────────────────────────────────────────────

describe('the brief', () => {
  const briefInput = {
    sourceType: 'task' as const,
    evidence: [{ label: 'Source', text: 'The deploy failed on step 3.' }],
    metadata: { status: 'failed' },
    actions: ACTIONS,
  }

  it('parses a brief a small model buried in prose', async () => {
    const r = world({
      replies: [
        'Sure!\n```json\n{"question":"Retry or reassign the deploy?","recommendation":"Open the task and inspect step 3.","recommendedActionId":null}\n```\nHope that helps — see {task}.',
      ],
    })
    const res = await runHarness(inboxBriefHarness, briefInput, { caller: 'test:inbox', model: 'pl-main', deps: r.deps })
    expect(res.value?.question).toBe('Retry or reassign the deploy?')
    expect(res.repairs).toBe(0)
  })

  it('never widens — a brief has the full action list already', async () => {
    const r = world({ facts: WIDE, replies: ['{"question":"q?","recommendation":"r","recommendedActionId":"approve_task"}'] })
    const res = await runHarness(inboxBriefHarness, briefInput, { caller: 'test:inbox', model: 'pl-main', deps: r.deps })
    expect(res.widened).toBe(false)
  })

  it('refuses on a model MEASURED unable to produce JSON, rather than feeding prose to a parser', async () => {
    // This harness used to degrade here, and the argument was that refusing
    // would take the Inbox away from a self-host whose only model failed one
    // probe. That argument was answered by narrowing the fact rather than by
    // softening the floor: `json: false` no longer means "this endpoint ignores
    // response_format" (see `scoreJson`), it means the model did not return a
    // parseable object on ANY trial. A model that produces JSON from the prompt
    // alone now scores TRUE and still runs.
    //
    // What is left is a model that genuinely cannot do the thing this harness is
    // made of. Sending it anyway spent a call to hand prose to a JSON parser and
    // recorded the wreckage as the model's failure; `onFailure: 'null'` means
    // the card keeps its own deterministic question either way, so the only
    // difference is the wasted call and the missing reason.
    const r = world({ facts: { json: false }, replies: ['{"question":"q?","recommendation":"r"}'] })
    const res = await runHarness(inboxBriefHarness, briefInput, { caller: 'test:inbox', model: 'pl-main', deps: r.deps })
    expect(r.requests).toHaveLength(0)
    expect(res.error).toContain('cannot run harness "inbox-brief"')
    expect(res.value).toBeNull()
  })
})

// ── The fixtures the fitness suite replays ───────────────────────────────────

describe('eval fixtures', () => {
  it('every command fixture accepts a null actionId and rejects an invented one', () => {
    for (const fixture of inboxCommandHarness.evals ?? []) {
      expect(fixture.check({ message: 'ok', actionId: null }, NO_TOOLS)).toBeNull()
      const invented = fixture.check({ message: 'ok', actionId: 'rm_-rf' }, NO_TOOLS)
      expect(invented, `${fixture.name} accepted an invented action id`).not.toBeNull()
    }
  })

  it('every fixture asserts something a machine can check without a second model', () => {
    for (const def of [inboxBriefHarness, inboxCommandHarness]) {
      expect(def.evals?.length, `${def.id} shipped no fixtures`).toBeGreaterThan(0)
    }
  })
})
